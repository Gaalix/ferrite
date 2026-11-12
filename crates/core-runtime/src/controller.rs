use std::{
    collections::HashMap,
    ops::Range,
    path::PathBuf,
    sync::{Arc, Mutex, RwLock, RwLockReadGuard},
};

use futures::StreamExt;
use stremio_core::{
    constants::{
        META_RESOURCE_NAME, PROFILE_STORAGE_KEY, SEARCH_EXTRA_NAME, STREAM_RESOURCE_NAME,
    },
    models::{
        addon_details::Selected as AddonDetailsSelected,
        calendar::Selected as CalendarSelected,
        catalog_with_filters::Selected as CatalogWithFiltersSelected,
        catalogs_with_extra::Selected as CatalogsWithExtraSelected,
        installed_addons_with_filters::{
            InstalledAddonsRequest, Selected as InstalledAddonsWithFiltersSelected,
        },
        library_with_filters::Selected as LibraryWithFiltersSelected,
        meta_details::Selected as MetaDetailsSelected,
        player::{Selected as PlayerSelected, VideoParams},
    },
    runtime::{
        msg::{
            Action, ActionCatalogsWithExtra, ActionCtx, ActionLink, ActionLoad, ActionPlayer,
            ActionSearch, ActionStreamingServer, Event, Internal, Msg,
        },
        Effect, Env, Runtime, RuntimeAction, RuntimeEvent,
    },
    types::{
        addon::{Descriptor, DescriptorFlags, ExtraValue, Manifest, ResourcePath},
        api::AuthRequest,
        profile::Settings as ProfileSettings,
        resource::{MetaItemPreview, Stream},
    },
};
use tokio::sync::broadcast;
use tracing::error;
use url::Url;

use crate::{
    addon_engine::{
        addon_configure_url, addon_has_debrid_configured, curated_addon_directory,
        detect_clipboard_addon_url, has_debrid_configured_addons, normalize_addon_url, redact_url,
        AddonCacheStats, AddonHttpEngine, AddonTelemetrySnapshot, CuratedAddonEntry,
        InstallAddonOutcome, MutedAddonEntry,
    },
    debrid::{
        DebridAccountStatus, DebridBaseUrls, DebridCacheStatus, LocalDebridResolver,
        ResolvedDebridStream,
    },
    env::{DesktopEnv, EnvConfig, SqliteStorage},
    error::CoreRuntimeError,
    model::{DesktopModel, DesktopModelField, ModelMutationQueue},
    prewarm::{
        convert_stream_to_playback_url, effective_streaming_server_url,
        evaluate_next_episode_prewarm, extract_player_series_id, inspect_next_episode_resolution,
        NextEpisodeResolution, NextEpisodeTarget, ResolvedNextEpisode,
    },
    private_config::{
        LocalDebridProvider, PrivateIntegrationsConfig, ProwlarrFallbackMode,
    },
    prowlarr::{ProwlarrClient, ProwlarrTestResult, RichStreamItem},
    track_prefs::{
        delete_series_track_pref, load_all_series_track_prefs, load_series_track_pref,
        store_series_track_pref, SeriesTrackPreference,
    },
};

const RUNTIME_CHANNEL_BUFFER: usize = 4096;
const BROADCAST_CHANNEL_CAPACITY: usize = 1024;
const DEFAULT_LOCAL_SEARCH_LIMIT: usize = 25;

pub const INITIAL_BOARD_SHELVES: usize = 6;

pub type DesktopRuntimeEvent = RuntimeEvent<DesktopEnv, DesktopModel>;

pub type DesktopRuntimeAction = RuntimeAction<DesktopEnv, DesktopModel>;

#[derive(Clone)]
pub struct CoreController {
    runtime: Runtime<DesktopEnv, DesktopModel>,
    event_tx: broadcast::Sender<Arc<DesktopRuntimeEvent>>,
    storage: SqliteStorage,
    addon_engine: Arc<AddonHttpEngine>,
    http_client: reqwest::Client,
    runtime_handle: tokio::runtime::Handle,
    mutation_queue: ModelMutationQueue,
    mutation_gate: Arc<Mutex<()>>,
    private_config: Arc<RwLock<PrivateIntegrationsConfig>>,
    private_config_path: PathBuf,
    debrid_resolver: Arc<LocalDebridResolver>,
    prowlarr_client: Arc<ProwlarrClient>,
}

#[derive(Clone, Copy, Debug, Default)]
struct AddonMutationSideEffects {
    has_auth: bool,
    board_loaded: bool,
    discover_needs_reload: bool,
}

impl std::fmt::Debug for CoreController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoreController")
            .field("subscribers", &self.event_tx.receiver_count())
            .finish()
    }
}

impl CoreController {
    pub async fn boot(config: EnvConfig) -> Result<Self, CoreRuntimeError> {
        DesktopEnv::init(config)?;
        Self::from_initialized_env().await
    }

    pub async fn from_initialized_env() -> Result<Self, CoreRuntimeError> {
        if !DesktopEnv::is_initialized() {
            return Err(CoreRuntimeError::NotInitialized);
        }

        let storage = DesktopEnv::sqlite_storage()?;
        let addon_engine = DesktopEnv::addon_engine()?;
        let http_client = DesktopEnv::http_client()?;
        let runtime_handle = DesktopEnv::runtime_handle()?;
        let private_config_path = DesktopEnv::private_config_path()?;
        let loaded_private_config =
            match PrivateIntegrationsConfig::load_from_path(&private_config_path) {
                Ok(cfg) => cfg,
                Err(err) => {
                    error!("Failed to load private integrations config: {err}");
                    PrivateIntegrationsConfig::default()
                }
            };
        let buckets = DesktopEnv::bootstrap_buckets().await?;
        let (model, effects) = DesktopModel::from_buckets(buckets);
        let mutation_queue = model.mutation_queue.clone();

        let (runtime, mut rx) = Runtime::<DesktopEnv, DesktopModel>::new(
            model,
            effects.into_iter().collect(),
            RUNTIME_CHANNEL_BUFFER,
        );

        let (event_tx, _) = broadcast::channel(BROADCAST_CHANNEL_CAPACITY);
        let pump_tx = event_tx.clone();

        DesktopEnv::exec_concurrent(async move {
            while let Some(event) = rx.next().await {
                let _ = pump_tx.send(Arc::new(event));
            }
        });

        Ok(Self {
            runtime,
            event_tx,
            storage,
            addon_engine,
            http_client,
            runtime_handle,
            mutation_queue,
            mutation_gate: Arc::new(Mutex::new(())),
            private_config: Arc::new(RwLock::new(loaded_private_config)),
            private_config_path,
            debrid_resolver: Arc::new(LocalDebridResolver::new()),
            prowlarr_client: Arc::new(ProwlarrClient::new()),
        })
    }

    fn mutate_model_sync<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut DesktopModel) -> (R, Vec<Effect>, Vec<DesktopModelField>) + Send + 'static,
    ) -> R {
        let _gate = self
            .mutation_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let out_slot: Arc<Mutex<Option<R>>> = Arc::new(Mutex::new(None));
        let slot_writer = Arc::clone(&out_slot);

        self.mutation_queue.push(Box::new(move |model| {
            let (output, effects, changed_fields) = f(model);
            if let Ok(mut guard) = slot_writer.lock() {
                *guard = Some(output);
            }
            (effects, changed_fields)
        }));

        self.dispatch(
            Action::Link(ActionLink::ReadData),
            Some(DesktopModelField::DataExport),
        );

        let output = out_slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
            .expect("synchronous model mutation must run during dispatch");
        output
    }

    fn apply_addon_side_effects(&self, side: AddonMutationSideEffects) {
        self.addon_engine.clear_cache();
        if side.has_auth {
            self.dispatch_action(Action::Ctx(ActionCtx::PushAddonsToAPI));
        }
        if side.board_loaded {
            self.load_board_range(0..INITIAL_BOARD_SHELVES);
        }
        if side.discover_needs_reload {
            self.load_discover(None);
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Arc<DesktopRuntimeEvent>> {
        self.event_tx.subscribe()
    }

    pub fn model(&self) -> RwLockReadGuard<'_, DesktopModel> {
        self.runtime
            .model()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn with_model<R>(&self, f: impl FnOnce(&DesktopModel) -> R) -> R {
        let guard = self.model();
        f(&guard)
    }

    pub fn runtime(&self) -> &Runtime<DesktopEnv, DesktopModel> {
        &self.runtime
    }

    pub fn dispatch(&self, action: Action, field: Option<DesktopModelField>) {
        self.runtime.dispatch(RuntimeAction { field, action });
    }

    pub fn dispatch_action(&self, action: Action) {
        self.dispatch(action, None);
    }

    pub fn login_with_password(&self, email: impl Into<String>, password: impl Into<String>) {
        self.dispatch_action(Action::Ctx(ActionCtx::Authenticate(AuthRequest::Login {
            email: email.into(),
            password: password.into(),
            facebook: false,
        })));
    }

    pub fn login_with_token(&self, token: impl Into<String>) {
        self.dispatch_action(Action::Ctx(ActionCtx::Authenticate(
            AuthRequest::LoginWithToken {
                token: token.into(),
            },
        )));
    }

    pub fn start_link_auth(&self) {
        self.dispatch(
            Action::Load(ActionLoad::Link),
            Some(DesktopModelField::AuthLink),
        );
    }

    pub fn poll_link_auth(&self) {
        self.dispatch(
            Action::Link(ActionLink::ReadData),
            Some(DesktopModelField::AuthLink),
        );
    }

    pub fn logout(&self) {
        self.dispatch_action(Action::Ctx(ActionCtx::Logout));
    }

    pub fn load_board(&self) {
        self.dispatch(
            Action::Load(ActionLoad::CatalogsWithExtra(CatalogsWithExtraSelected {
                r#type: None,
                extra: vec![],
            })),
            Some(DesktopModelField::Board),
        );
        self.dispatch(
            Action::CatalogsWithExtra(ActionCatalogsWithExtra::LoadRange(
                0..INITIAL_BOARD_SHELVES,
            )),
            Some(DesktopModelField::Board),
        );
    }

    pub fn load_board_range(&self, range: Range<usize>) {
        self.dispatch(
            Action::CatalogsWithExtra(ActionCatalogsWithExtra::LoadRange(range)),
            Some(DesktopModelField::Board),
        );
    }

    pub fn load_board_next_page(&self, catalog_index: usize) {
        self.dispatch(
            Action::CatalogsWithExtra(ActionCatalogsWithExtra::LoadNextPage(catalog_index)),
            Some(DesktopModelField::Board),
        );
    }

    pub fn load_discover(&self, selected: Option<CatalogWithFiltersSelected>) {
        self.dispatch(
            Action::Load(ActionLoad::CatalogWithFilters(selected)),
            Some(DesktopModelField::Discover),
        );
    }

    pub fn unload_discover(&self) {
        self.dispatch(Action::Unload, Some(DesktopModelField::Discover));
    }

    pub fn load_library(&self, selected: LibraryWithFiltersSelected) {
        self.dispatch(
            Action::Load(ActionLoad::LibraryWithFilters(selected)),
            Some(DesktopModelField::Library),
        );
    }

    pub fn load_continue_watching(&self, selected: LibraryWithFiltersSelected) {
        self.dispatch(
            Action::Load(ActionLoad::LibraryWithFilters(selected)),
            Some(DesktopModelField::ContinueWatching),
        );
    }

    pub fn load_calendar(&self, selected: Option<CalendarSelected>) {
        self.dispatch(
            Action::Load(ActionLoad::Calendar(selected)),
            Some(DesktopModelField::Calendar),
        );
    }

    pub fn load_local_search(&self) {
        self.dispatch(
            Action::Load(ActionLoad::LocalSearch),
            Some(DesktopModelField::LocalSearch),
        );
    }

    pub fn unload_local_search(&self) {
        self.dispatch(Action::Unload, Some(DesktopModelField::LocalSearch));
    }

    pub fn unload_search(&self) {
        self.dispatch(Action::Unload, Some(DesktopModelField::Search));
    }

    pub fn search(&self, query: impl Into<String>) {
        let query = query.into();
        let trimmed = query.trim();
        if trimmed.is_empty() {
            self.unload_search();
            return;
        }

        let search_query = trimmed.to_owned();
        self.dispatch(
            Action::Load(ActionLoad::CatalogsWithExtra(CatalogsWithExtraSelected {
                r#type: None,
                extra: vec![ExtraValue {
                    name: SEARCH_EXTRA_NAME.to_owned(),
                    value: search_query.clone(),
                }],
            })),
            Some(DesktopModelField::Search),
        );
        self.dispatch(
            Action::CatalogsWithExtra(ActionCatalogsWithExtra::LoadRange(0..usize::MAX)),
            Some(DesktopModelField::Search),
        );
        if self.with_model(|m| !m.local_search.current_records.is_empty()) {
            self.dispatch(
                Action::Search(ActionSearch::Search {
                    search_query,
                    max_results: DEFAULT_LOCAL_SEARCH_LIMIT,
                }),
                Some(DesktopModelField::LocalSearch),
            );
        }
    }

    pub fn toggle_library_item(&self, meta_preview: MetaItemPreview) {
        let is_in_library = self.with_model(|model| {
            model
                .ctx
                .library
                .items
                .get(&meta_preview.id)
                .is_some_and(|item| !item.removed)
        });

        if is_in_library {
            self.dispatch_action(Action::Ctx(ActionCtx::RemoveFromLibrary(meta_preview.id)));
        } else {
            self.dispatch_action(Action::Ctx(ActionCtx::AddToLibrary(meta_preview)));
        }
    }

    pub fn load_meta_details(
        &self,
        type_name: impl Into<String>,
        id: impl Into<String>,
        video_id: Option<&str>,
    ) {
        let type_name = type_name.into();
        let id = id.into();
        let stream_path = video_id.map(|vid| ResourcePath {
            resource: STREAM_RESOURCE_NAME.to_owned(),
            r#type: type_name.clone(),
            id: vid.to_owned(),
            extra: vec![],
        });
        let guess_stream = stream_path.is_none();

        self.dispatch(
            Action::Load(ActionLoad::MetaDetails(MetaDetailsSelected {
                meta_path: ResourcePath {
                    resource: META_RESOURCE_NAME.to_owned(),
                    r#type: type_name,
                    id,
                    extra: vec![],
                },
                stream_path,
                guess_stream,
            })),
            Some(DesktopModelField::MetaDetails),
        );
    }

    pub fn unload_meta_details(&self) {
        self.dispatch(Action::Unload, Some(DesktopModelField::MetaDetails));
    }

    pub fn load_player(&self, selected: PlayerSelected) {
        self.dispatch(
            Action::Load(ActionLoad::Player(Box::new(selected))),
            Some(DesktopModelField::Player),
        );
    }

    pub fn unload_player(&self) {
        self.dispatch(Action::Unload, Some(DesktopModelField::Player));
    }

    pub fn player_time_changed(&self, time_ms: u64, duration_ms: u64, device: impl Into<String>) {
        self.dispatch(
            Action::Player(ActionPlayer::TimeChanged {
                time: time_ms,
                duration: duration_ms,
                device: device.into(),
            }),
            Some(DesktopModelField::Player),
        );
    }

    pub fn player_seek(&self, time_ms: u64, duration_ms: u64, device: impl Into<String>) {
        self.dispatch(
            Action::Player(ActionPlayer::Seek {
                time: time_ms,
                duration: duration_ms,
                device: device.into(),
            }),
            Some(DesktopModelField::Player),
        );
    }

    pub fn player_video_params_changed(&self, video_params: Option<VideoParams>) {
        self.dispatch(
            Action::Player(ActionPlayer::VideoParamsChanged { video_params }),
            Some(DesktopModelField::Player),
        );
    }

    pub fn player_paused_changed(&self, paused: bool) {
        self.dispatch(
            Action::Player(ActionPlayer::PausedChanged { paused }),
            Some(DesktopModelField::Player),
        );
    }

    pub fn player_ended(&self) {
        self.dispatch(
            Action::Player(ActionPlayer::Ended),
            Some(DesktopModelField::Player),
        );
    }

    pub fn normalize_addon_url(raw: &str) -> Result<Url, String> {
        normalize_addon_url(raw)
    }

    pub fn addon_configure_url(transport_url: &Url) -> Option<Url> {
        addon_configure_url(transport_url)
    }

    pub fn detect_clipboard_addon_url(clipboard_text: &str) -> Option<Url> {
        detect_clipboard_addon_url(clipboard_text)
    }

    pub fn curated_addon_directory() -> &'static [CuratedAddonEntry] {
        curated_addon_directory()
    }

    pub fn addon_has_debrid_configured(descriptor: &Descriptor) -> bool {
        addon_has_debrid_configured(descriptor)
    }

    pub fn has_debrid_addon_configured(&self) -> bool {
        self.with_model(|model| has_debrid_configured_addons(&model.ctx.profile.addons))
    }

    pub fn install_addon(&self, descriptor: Descriptor) {
        let _ = self
            .storage
            .remove_muted_addon_by_url(&descriptor.transport_url);
        self.addon_engine.clear_cache();
        let board_loaded = self.with_model(|m| m.board.selected.is_some());
        self.dispatch_action(Action::Ctx(ActionCtx::InstallAddon(descriptor)));
        if board_loaded {
            self.load_board_range(0..INITIAL_BOARD_SHELVES);
        }
    }

    pub fn uninstall_addon(&self, descriptor: Descriptor) {
        let removed_muted = self
            .storage
            .remove_muted_addon_by_url(&descriptor.transport_url)
            .unwrap_or(false);
        self.addon_engine.clear_cache();
        let (was_active, board_loaded, discover_needs_reload) = self.with_model(|m| {
            let active = m
                .ctx
                .profile
                .addons
                .iter()
                .any(|a| a.transport_url == descriptor.transport_url);
            let discover_hit = m
                .discover
                .selected
                .as_ref()
                .is_some_and(|sel| sel.request.base == descriptor.transport_url);
            (active, m.board.selected.is_some(), discover_hit)
        });

        if was_active || !removed_muted {
            self.dispatch_action(Action::Ctx(ActionCtx::UninstallAddon(descriptor)));
        } else {
            let _ = self.event_tx.send(Arc::new(RuntimeEvent::NewState(vec![
                DesktopModelField::InstalledAddons,
            ])));
        }

        if board_loaded && was_active {
            self.load_board_range(0..INITIAL_BOARD_SHELVES);
        }
        if discover_needs_reload {
            self.load_discover(None);
        }
    }

    pub async fn install_or_replace_addon_by_url(
        &self,
        raw_url: &str,
    ) -> Result<InstallAddonOutcome, String> {
        let transport_url = normalize_addon_url(raw_url)?;
        let redacted_url = redact_url(&transport_url);

        self.addon_engine.reset_circuit(transport_url.as_str());
        self.addon_engine.invalidate_url(transport_url.as_str());

        let client = self.http_client.clone();
        let engine = Arc::clone(&self.addon_engine);
        let fetch_url = transport_url.to_string();

        let bytes = self
            .runtime_handle
            .spawn(async move {
                engine
                    .execute_bytes(
                        &client,
                        http::Method::GET,
                        fetch_url,
                        http::HeaderMap::new(),
                        None,
                        true,
                    )
                    .await
            })
            .await
            .map_err(|join_err| format!("Manifest fetch task failed: {join_err}"))?
            .map_err(|env_err| {
                format!("Failed to fetch manifest from {redacted_url}: {env_err}")
            })?;

        let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|err| {
            format!("Invalid Stremio addon manifest JSON at {redacted_url}: {err}")
        })?;

        if manifest.id.trim().is_empty() || manifest.name.trim().is_empty() {
            return Err(format!(
                "Manifest at {redacted_url} is missing required 'id' or 'name'"
            ));
        }

        let descriptor = Descriptor {
            manifest,
            transport_url,
            flags: DescriptorFlags::default(),
        };

        self.install_or_replace_addon_descriptor(descriptor)
    }

    pub fn install_or_replace_addon_descriptor(
        &self,
        mut descriptor: Descriptor,
    ) -> Result<InstallAddonOutcome, String> {
        if descriptor.manifest.behavior_hints.configuration_required {
            return Err(format!(
                "'{}' requires configuration in browser first — click Configure, then paste your personal manifest URL.",
                descriptor.manifest.name
            ));
        }

        let storage = self.storage.clone();
        let result = self.mutate_model_sync(move |model| {
            if model.ctx.profile.addons_locked {
                return (
                    Err("User addons are currently locked due to an account sync error".to_owned()),
                    vec![],
                    vec![],
                );
            }

            let existing_pos = model.ctx.profile.addons.iter().position(|existing| {
                existing.manifest.id == descriptor.manifest.id
                    || existing.transport_url == descriptor.transport_url
            });

            let (replaced_existing, was_muted, slot_index, core_event) =
                if let Some(pos) = existing_pos {
                    if model.ctx.profile.addons[pos].flags.protected || descriptor.flags.protected {
                        return (
                            Err(format!(
                                "'{}' is a protected official addon and cannot be replaced",
                                model.ctx.profile.addons[pos].manifest.name
                            )),
                            vec![],
                            vec![],
                        );
                    }

                    let old_transport_url = model.ctx.profile.addons[pos].transport_url.clone();
                    if old_transport_url == descriptor.transport_url {
                        descriptor.flags.official = model.ctx.profile.addons[pos].flags.official;
                    } else {
                        model
                            .ctx
                            .streams
                            .items
                            .retain(|_, item| item.stream_transport_url != old_transport_url);
                    }

                    model.ctx.profile.addons[pos] = descriptor.clone();

                    let mut idx = model.ctx.profile.addons.len();
                    while idx > pos + 1 {
                        idx -= 1;
                        let is_dup = model.ctx.profile.addons[idx].manifest.id
                            == descriptor.manifest.id
                            || model.ctx.profile.addons[idx].transport_url
                                == descriptor.transport_url;
                        if is_dup && !model.ctx.profile.addons[idx].flags.protected {
                            let removed = model.ctx.profile.addons.remove(idx);
                            model.ctx.streams.items.retain(|_, item| {
                                item.stream_transport_url != removed.transport_url
                            });
                        }
                    }

                    let _ = storage.take_muted_addon_by_url(&descriptor.transport_url);
                    let _ = storage.take_muted_addon_by_id(&descriptor.manifest.id);

                    (
                        true,
                        false,
                        pos,
                        Event::AddonUpgraded {
                            transport_url: descriptor.transport_url.clone(),
                            id: descriptor.manifest.id.clone(),
                        },
                    )
                } else {
                    let muted_match = storage
                        .take_muted_addon_by_url(&descriptor.transport_url)
                        .ok()
                        .flatten()
                        .or_else(|| {
                            storage
                                .take_muted_addon_by_id(&descriptor.manifest.id)
                                .ok()
                                .flatten()
                        });

                    if let Some(muted) = muted_match {
                        let insert_at = muted.preferred_index.min(model.ctx.profile.addons.len());
                        model
                            .ctx
                            .profile
                            .addons
                            .insert(insert_at, descriptor.clone());
                        (
                            true,
                            true,
                            insert_at,
                            Event::AddonInstalled {
                                transport_url: descriptor.transport_url.clone(),
                                id: descriptor.manifest.id.clone(),
                            },
                        )
                    } else {
                        let insert_at = model.ctx.profile.addons.len();
                        model.ctx.profile.addons.push(descriptor.clone());
                        (
                            false,
                            false,
                            insert_at,
                            Event::AddonInstalled {
                                transport_url: descriptor.transport_url.clone(),
                                id: descriptor.manifest.id.clone(),
                            },
                        )
                    }
                };

            persist_profile_snapshot(&storage, &model.ctx.profile);
            let side = compute_addon_side_effects(model);
            let outcome = InstallAddonOutcome {
                descriptor,
                replaced_existing,
                was_muted,
                slot_index,
            };

            let effects = vec![
                Effect::Msg(Box::new(Msg::Event(core_event))),
                Effect::Msg(Box::new(Msg::Internal(Internal::ProfileChanged))),
            ];

            (Ok((outcome, side)), effects, vec![DesktopModelField::Ctx])
        });

        match result {
            Ok((outcome, side)) => {
                self.apply_addon_side_effects(side);
                Ok(outcome)
            }
            Err(err) => Err(err),
        }
    }

    pub fn move_addon_up(&self, transport_url: &Url) -> bool {
        let target = transport_url.clone();
        let storage = self.storage.clone();
        let moved = self.mutate_model_sync(move |model| {
            if model.ctx.profile.addons_locked {
                return (None, vec![], vec![]);
            }
            let Some(idx) = model
                .ctx
                .profile
                .addons
                .iter()
                .position(|a| a.transport_url == target)
            else {
                return (None, vec![], vec![]);
            };
            if idx == 0 {
                return (None, vec![], vec![]);
            }

            model.ctx.profile.addons.swap(idx, idx - 1);
            persist_profile_snapshot(&storage, &model.ctx.profile);
            let side = compute_addon_side_effects(model);
            (
                Some(side),
                vec![Effect::Msg(Box::new(Msg::Internal(
                    Internal::ProfileChanged,
                )))],
                vec![DesktopModelField::Ctx],
            )
        });

        if let Some(side) = moved {
            self.apply_addon_side_effects(side);
            true
        } else {
            false
        }
    }

    pub fn move_addon_down(&self, transport_url: &Url) -> bool {
        let target = transport_url.clone();
        let storage = self.storage.clone();
        let moved = self.mutate_model_sync(move |model| {
            if model.ctx.profile.addons_locked {
                return (None, vec![], vec![]);
            }
            let len = model.ctx.profile.addons.len();
            let Some(idx) = model
                .ctx
                .profile
                .addons
                .iter()
                .position(|a| a.transport_url == target)
            else {
                return (None, vec![], vec![]);
            };
            if idx + 1 >= len {
                return (None, vec![], vec![]);
            }

            model.ctx.profile.addons.swap(idx, idx + 1);
            persist_profile_snapshot(&storage, &model.ctx.profile);
            let side = compute_addon_side_effects(model);
            (
                Some(side),
                vec![Effect::Msg(Box::new(Msg::Internal(
                    Internal::ProfileChanged,
                )))],
                vec![DesktopModelField::Ctx],
            )
        });

        if let Some(side) = moved {
            self.apply_addon_side_effects(side);
            true
        } else {
            false
        }
    }

    pub fn move_addon_to_top(&self, transport_url: &Url) -> bool {
        let target = transport_url.clone();
        let storage = self.storage.clone();
        let moved = self.mutate_model_sync(move |model| {
            if model.ctx.profile.addons_locked {
                return (None, vec![], vec![]);
            }
            let Some(idx) = model
                .ctx
                .profile
                .addons
                .iter()
                .position(|a| a.transport_url == target)
            else {
                return (None, vec![], vec![]);
            };
            if idx == 0 {
                return (None, vec![], vec![]);
            }

            let addon = model.ctx.profile.addons.remove(idx);
            model.ctx.profile.addons.insert(0, addon);
            persist_profile_snapshot(&storage, &model.ctx.profile);
            let side = compute_addon_side_effects(model);
            (
                Some(side),
                vec![Effect::Msg(Box::new(Msg::Internal(
                    Internal::ProfileChanged,
                )))],
                vec![DesktopModelField::Ctx],
            )
        });

        if let Some(side) = moved {
            self.apply_addon_side_effects(side);
            true
        } else {
            false
        }
    }

    pub fn set_addon_enabled(&self, transport_url: &Url, enabled: bool) -> Result<(), String> {
        let target = transport_url.clone();
        let redacted_target = redact_url(&target);
        let storage = self.storage.clone();

        let outcome = self.mutate_model_sync(move |model| {
            if model.ctx.profile.addons_locked {
                return (
                    Err("User addons are currently locked due to an account sync error".to_owned()),
                    vec![],
                    vec![],
                );
            }

            if !enabled {
                let Some(idx) = model
                    .ctx
                    .profile
                    .addons
                    .iter()
                    .position(|a| a.transport_url == target)
                else {
                    let already_muted = storage
                        .list_muted_addons()
                        .map(|list| list.iter().any(|m| m.descriptor.transport_url == target))
                        .unwrap_or(false);
                    return if already_muted {
                        (Ok(None), vec![], vec![])
                    } else {
                        (
                            Err(format!(
                                "Addon '{redacted_target}' is not currently installed"
                            )),
                            vec![],
                            vec![],
                        )
                    };
                };

                if model.ctx.profile.addons[idx].flags.protected {
                    return (
                        Err(format!(
                            "'{}' is a protected official addon and cannot be disabled",
                            model.ctx.profile.addons[idx].manifest.name
                        )),
                        vec![],
                        vec![],
                    );
                }

                let descriptor = model.ctx.profile.addons[idx].clone();
                let muted_entry = MutedAddonEntry {
                    descriptor: descriptor.clone(),
                    preferred_index: idx,
                };

                if let Err(err) = storage.upsert_muted_addon(&muted_entry) {
                    return (
                        Err(format!("Failed to persist muted addon state: {err}")),
                        vec![],
                        vec![],
                    );
                }

                model.ctx.profile.addons.remove(idx);
                model
                    .ctx
                    .streams
                    .items
                    .retain(|_, item| item.stream_transport_url != target);

                persist_profile_snapshot(&storage, &model.ctx.profile);
                let side = compute_addon_side_effects(model);
                (
                    Ok(Some(side)),
                    vec![
                        Effect::Msg(Box::new(Msg::Event(Event::AddonUninstalled {
                            transport_url: descriptor.transport_url,
                            id: descriptor.manifest.id,
                        }))),
                        Effect::Msg(Box::new(Msg::Internal(Internal::ProfileChanged))),
                    ],
                    vec![DesktopModelField::Ctx],
                )
            } else {
                if model
                    .ctx
                    .profile
                    .addons
                    .iter()
                    .any(|a| a.transport_url == target)
                {
                    let _ = storage.remove_muted_addon_by_url(&target);
                    return (Ok(None), vec![], vec![]);
                }

                let muted_opt = match storage.take_muted_addon_by_url(&target) {
                    Ok(v) => v,
                    Err(err) => {
                        return (
                            Err(format!("Failed to read muted addon from SQLite: {err}")),
                            vec![],
                            vec![],
                        );
                    }
                };

                let Some(muted) = muted_opt else {
                    return (
                        Err(format!("Muted addon '{redacted_target}' was not found")),
                        vec![],
                        vec![],
                    );
                };

                let descriptor = muted.descriptor;
                if let Some(existing_same_id) = model.ctx.profile.addons.iter().position(|a| {
                    a.manifest.id == descriptor.manifest.id && !a.flags.protected
                }) {
                    model.ctx.profile.addons[existing_same_id] = descriptor.clone();
                } else {
                    let insert_at = muted.preferred_index.min(model.ctx.profile.addons.len());
                    model.ctx.profile.addons.insert(insert_at, descriptor.clone());
                }

                persist_profile_snapshot(&storage, &model.ctx.profile);
                let side = compute_addon_side_effects(model);
                (
                    Ok(Some(side)),
                    vec![
                        Effect::Msg(Box::new(Msg::Event(Event::AddonInstalled {
                            transport_url: descriptor.transport_url,
                            id: descriptor.manifest.id,
                        }))),
                        Effect::Msg(Box::new(Msg::Internal(Internal::ProfileChanged))),
                    ],
                    vec![DesktopModelField::Ctx],
                )
            }
        });

        match outcome {
            Ok(Some(side)) => {
                self.apply_addon_side_effects(side);
                Ok(())
            }
            Ok(None) => Ok(()),
            Err(err) => Err(err),
        }
    }

    pub fn list_muted_addons(&self) -> Vec<MutedAddonEntry> {
        self.storage.list_muted_addons().unwrap_or_default()
    }

    pub fn remove_muted_addon(&self, transport_url: &Url) -> bool {
        let removed = self
            .storage
            .remove_muted_addon_by_url(transport_url)
            .unwrap_or(false);
        if removed {
            self.addon_engine.clear_cache();
            let _ = self.event_tx.send(Arc::new(RuntimeEvent::NewState(vec![
                DesktopModelField::InstalledAddons,
            ])));
        }
        removed
    }

    pub fn load_addon_details(&self, transport_url: Url) {
        self.dispatch(
            Action::Load(ActionLoad::AddonDetails(AddonDetailsSelected {
                transport_url,
            })),
            Some(DesktopModelField::AddonDetails),
        );
    }

    pub fn load_installed_addons(&self, r#type: Option<String>) {
        self.dispatch(
            Action::Load(ActionLoad::InstalledAddonsWithFilters(
                InstalledAddonsWithFiltersSelected {
                    request: InstalledAddonsRequest { r#type },
                },
            )),
            Some(DesktopModelField::InstalledAddons),
        );
    }

    pub fn load_remote_addons(&self, selected: Option<CatalogWithFiltersSelected>) {
        self.dispatch(
            Action::Load(ActionLoad::CatalogWithFilters(selected)),
            Some(DesktopModelField::RemoteAddons),
        );
    }

    pub fn load_default_remote_addons(&self) {
        self.load_remote_addons(None);
    }

    pub fn clear_addon_cache(&self) {
        self.addon_engine.clear_cache();
    }

    pub fn addon_cache_stats(&self) -> AddonCacheStats {
        self.addon_engine.cache_stats()
    }

    pub fn addon_telemetry(&self) -> HashMap<String, AddonTelemetrySnapshot> {
        let mut snapshots = self.addon_engine.telemetry_by_origin();
        let active_urls: Vec<Url> = self.with_model(|model| {
            model
                .ctx
                .profile
                .addons
                .iter()
                .map(|a| a.transport_url.clone())
                .collect()
        });
        for url in active_urls {
            let snap = self.addon_engine.telemetry_for_url(&url);
            snapshots
                .entry(snap.origin.clone())
                .or_insert_with(|| snap.clone());
            snapshots.insert(url.to_string(), snap);
        }
        for muted in self.list_muted_addons() {
            let url = muted.descriptor.transport_url;
            let snap = self.addon_engine.telemetry_for_url(&url);
            snapshots
                .entry(snap.origin.clone())
                .or_insert_with(|| snap.clone());
            snapshots.insert(url.to_string(), snap);
        }
        snapshots
    }

    pub fn addon_telemetry_for(&self, transport_url: &Url) -> AddonTelemetrySnapshot {
        self.addon_engine.telemetry_for_url(transport_url)
    }

    pub fn reset_addon_circuit(&self, transport_url_or_origin: &str) -> bool {
        self.addon_engine.reset_circuit(transport_url_or_origin)
    }

    pub fn update_profile_settings(&self, settings: ProfileSettings) {
        self.dispatch_action(Action::Ctx(ActionCtx::UpdateSettings(settings)));
    }

    pub fn reload_streaming_server(&self) {
        self.dispatch(
            Action::StreamingServer(ActionStreamingServer::Reload),
            Some(DesktopModelField::StreamingServer),
        );
    }

    pub fn active_player_series_id(&self) -> Option<String> {
        self.with_model(|model| extract_player_series_id(&model.player))
    }

    pub fn get_series_track_pref(&self, series_id: &str) -> Option<SeriesTrackPreference> {
        load_series_track_pref(&self.storage, series_id)
    }

    pub fn save_series_track_pref(&self, pref: SeriesTrackPreference) {
        if let Err(err) = store_series_track_pref(&self.storage, pref) {
            error!("Failed to save series track preference: {err:?}");
        }
    }

    pub fn remove_series_track_pref(&self, series_id: &str) -> bool {
        match delete_series_track_pref(&self.storage, series_id) {
            Ok(removed) => removed,
            Err(err) => {
                error!("Failed to remove series track preference: {err:?}");
                false
            }
        }
    }

    pub fn list_series_track_prefs(&self) -> HashMap<String, SeriesTrackPreference> {
        load_all_series_track_prefs(&self.storage)
    }

    pub fn should_trigger_next_episode_prewarm(
        &self,
        current_pos_secs: f64,
        duration_secs: f64,
    ) -> Option<NextEpisodeTarget> {
        self.with_model(|model| {
            evaluate_next_episode_prewarm(model, current_pos_secs, duration_secs)
        })
    }

    pub fn resolve_next_episode_playback_url(&self) -> Option<ResolvedNextEpisode> {
        let resolution = self.with_model(inspect_next_episode_resolution);
        match resolution {
            NextEpisodeResolution::Ready(resolved) => Some(*resolved),
            NextEpisodeResolution::NeedsStreamFetch {
                type_name,
                meta_id,
                video_id,
            } => {
                self.load_meta_details(type_name, meta_id, Some(&video_id));
                None
            }
            NextEpisodeResolution::Unavailable => None,
        }
    }

    pub fn resolve_stream_playback_url(&self, stream: &Stream) -> Option<Url> {
        self.with_model(|model| {
            let server_url = effective_streaming_server_url(model);
            convert_stream_to_playback_url(stream, server_url)
                .ok()
                .map(|(url, _, _)| url)
        })
    }

    pub fn private_integrations(&self) -> PrivateIntegrationsConfig {
        self.private_config
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    pub fn update_private_integrations(
        &self,
        next: PrivateIntegrationsConfig,
    ) -> Result<(), String> {
        let normalized = next.normalized();
        normalized.save_to_path(&self.private_config_path)?;
        if let Ok(mut guard) = self.private_config.write() {
            let key_or_provider_changed = guard.active_debrid != normalized.active_debrid
                || guard.real_debrid_api_key != normalized.real_debrid_api_key
                || guard.torbox_api_key != normalized.torbox_api_key
                || guard.alldebrid_api_key != normalized.alldebrid_api_key;
            *guard = normalized;
            if key_or_provider_changed {
                self.debrid_resolver.clear_caches();
            }
        }
        Ok(())
    }

    pub fn set_debrid_base_urls(&self, base_urls: DebridBaseUrls) {
        self.debrid_resolver.set_base_urls(base_urls);
    }

    pub fn has_local_debrid_configured(&self) -> bool {
        self.private_integrations().has_local_debrid_configured()
    }

    pub fn has_any_debrid_configured(&self) -> bool {
        self.has_local_debrid_configured() || self.has_debrid_addon_configured()
    }

    pub fn last_debrid_resolve_latency_ms(&self) -> Option<u64> {
        self.debrid_resolver.last_resolve_latency_ms()
    }

    pub async fn verify_local_debrid(
        &self,
        provider: LocalDebridProvider,
        override_key: Option<String>,
    ) -> Result<DebridAccountStatus, String> {
        let cfg = self.private_integrations();
        let api_key = override_key
            .map(|k| k.trim().to_owned())
            .filter(|k| !k.is_empty())
            .or_else(|| cfg.key_for_provider(provider).map(|k| k.expose().to_owned()))
            .unwrap_or_default();

        let resolver = Arc::clone(&self.debrid_resolver);
        self.runtime_handle
            .spawn(async move { resolver.verify_account(provider, &api_key).await })
            .await
            .map_err(|err| format!("Debrid verification task failed: {err}"))?
    }

    pub async fn test_prowlarr(
        &self,
        override_url: Option<String>,
        override_key: Option<String>,
    ) -> Result<ProwlarrTestResult, String> {
        let cfg = self.private_integrations();
        let url = override_url
            .map(|u| u.trim().to_owned())
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| cfg.prowlarr_url.clone());
        let api_key = override_key
            .map(|k| k.trim().to_owned())
            .filter(|k| !k.is_empty())
            .unwrap_or_else(|| cfg.prowlarr_api_key.expose().to_owned());
        let timeout_ms = cfg.prowlarr_timeout_ms;

        let client = Arc::clone(&self.prowlarr_client);
        self.runtime_handle
            .spawn(async move { client.test_connection(&url, &api_key, timeout_ms).await })
            .await
            .map_err(|err| format!("Prowlarr connection test task failed: {err}"))?
    }

    pub async fn check_local_debrid_cache(
        &self,
        info_hashes: Vec<[u8; 20]>,
    ) -> HashMap<[u8; 20], DebridCacheStatus> {
        let cfg = self.private_integrations();
        if !cfg.has_local_debrid_configured() || info_hashes.is_empty() {
            return HashMap::new();
        }
        let provider = cfg.active_debrid;
        let api_key = cfg
            .active_debrid_key()
            .map(|k| k.expose().to_owned())
            .unwrap_or_default();

        let resolver = Arc::clone(&self.debrid_resolver);
        self.runtime_handle
            .spawn(async move { resolver.check_cache(provider, &api_key, &info_hashes).await })
            .await
            .unwrap_or_default()
    }

    pub async fn resolve_torrent_via_local_debrid(
        &self,
        info_hash: [u8; 20],
        file_idx: Option<u32>,
        announce: Vec<String>,
        season_episode: Option<(u32, u32)>,
    ) -> Result<ResolvedDebridStream, String> {
        let cfg = self.private_integrations();
        if !cfg.has_local_debrid_configured() {
            return Err("No local Debrid provider is configured".to_owned());
        }
        if !cfg.direct_rust_unrestrict {
            return Err("Direct Rust Debrid unrestricting is disabled in Settings".to_owned());
        }

        let provider = cfg.active_debrid;
        let api_key = cfg
            .active_debrid_key()
            .map(|k| k.expose().to_owned())
            .unwrap_or_default();

        let resolver = Arc::clone(&self.debrid_resolver);
        self.runtime_handle
            .spawn(async move {
                resolver
                    .resolve_torrent(
                        provider,
                        &api_key,
                        info_hash,
                        file_idx,
                        &announce,
                        season_episode,
                    )
                    .await
            })
            .await
            .map_err(|err| format!("Debrid resolve task failed: {err}"))?
    }

    pub async fn fetch_prowlarr_streams(
        &self,
        content_type: &str,
        video_id: &str,
        title_hint: Option<&str>,
    ) -> Result<Vec<RichStreamItem>, String> {
        let cfg = self.private_integrations();
        if matches!(cfg.prowlarr_mode, ProwlarrFallbackMode::Off) {
            return Ok(Vec::new());
        }
        if cfg.prowlarr_url.trim().is_empty() {
            return Err("Prowlarr URL is not configured".to_owned());
        }

        let url = cfg.prowlarr_url.clone();
        let api_key = cfg.prowlarr_api_key.expose().to_owned();
        let timeout_ms = cfg.prowlarr_timeout_ms;
        let content_type = content_type.to_owned();
        let video_id = video_id.to_owned();
        let title_hint = title_hint.map(str::to_owned);

        let client = Arc::clone(&self.prowlarr_client);
        self.runtime_handle
            .spawn(async move {
                client
                    .search(
                        &url,
                        &api_key,
                        timeout_ms,
                        &content_type,
                        &video_id,
                        title_hint.as_deref(),
                    )
                    .await
            })
            .await
            .map_err(|err| format!("Prowlarr search task failed: {err}"))?
    }
}

fn persist_profile_snapshot(
    storage: &SqliteStorage,
    profile: &stremio_core::types::profile::Profile,
) {
    match serde_json::to_string(profile) {
        Ok(serialized) => {
            if let Err(err) = storage.set_raw(PROFILE_STORAGE_KEY, Some(&serialized)) {
                error!("Failed to synchronously persist profile snapshot: {err:?}");
            }
        }
        Err(err) => {
            error!("Failed to serialize profile snapshot: {err}");
        }
    }
}

fn compute_addon_side_effects(model: &DesktopModel) -> AddonMutationSideEffects {
    let has_auth = model.ctx.profile.auth_key().is_some();
    let board_loaded = model.board.selected.is_some();
    let discover_needs_reload = model.discover.selected.as_ref().is_some_and(|sel| {
        !model
            .ctx
            .profile
            .addons
            .iter()
            .any(|a| a.transport_url == sel.request.base)
    });

    AddonMutationSideEffects {
        has_auth,
        board_loaded,
        discover_needs_reload,
    }
}
