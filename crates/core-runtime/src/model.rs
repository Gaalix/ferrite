use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Serialize};
use stremio_core::{
    models::{
        addon_details::AddonDetails,
        calendar::Calendar,
        catalog_with_filters::CatalogWithFilters,
        catalogs_with_extra::CatalogsWithExtra,
        continue_watching_preview::ContinueWatchingPreview,
        ctx::Ctx,
        data_export::DataExport,
        installed_addons_with_filters::InstalledAddonsWithFilters,
        library_with_filters::{ContinueWatchingFilter, LibraryWithFilters, NotRemovedFilter},
        link::Link,
        live_tv_continue_watching::LiveTvContinueWatching,
        live_tv_guide::LiveTvGuide,
        local_search::LocalSearch,
        meta_details::MetaDetails,
        player::Player,
        streaming_server::StreamingServer,
    },
    runtime::{
        msg::{Action, ActionLink, Msg},
        Effect, Effects, Model, Update, UpdateWithCtx,
    },
    types::{
        addon::Descriptor, api::LinkAuthKey, events::DismissedEventsBucket, library::LibraryBucket,
        notifications::NotificationsBucket, profile::Profile, resource::MetaItemPreview,
        search_history::SearchHistoryBucket, server_urls::ServerUrlsBucket, streams::StreamsBucket,
    },
};

use crate::env::{DesktopEnv, PersistedBuckets};

pub(crate) type ModelMutationFn =
    Box<dyn FnOnce(&mut DesktopModel) -> (Vec<Effect>, Vec<DesktopModelField>) + Send + 'static>;

#[derive(Clone, Default)]
pub(crate) struct ModelMutationQueue(Arc<Mutex<VecDeque<ModelMutationFn>>>);

impl std::fmt::Debug for ModelMutationQueue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ModelMutationQueue").finish()
    }
}

impl ModelMutationQueue {
    pub(crate) fn push(&self, f: ModelMutationFn) {
        if let Ok(mut guard) = self.0.lock() {
            guard.push_back(f);
        }
    }

    pub(crate) fn pop(&self) -> Option<ModelMutationFn> {
        self.0.lock().ok()?.pop_front()
    }
}

#[derive(Clone, Copy, Serialize, Deserialize, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum DesktopModelField {
    Ctx,
    AuthLink,
    DataExport,
    ContinueWatchingPreview,
    Board,
    Discover,
    Library,
    ContinueWatching,
    Calendar,
    LiveTvGuide,
    LiveTvContinueWatching,
    Search,
    LocalSearch,
    MetaDetails,
    RemoteAddons,
    InstalledAddons,
    AddonDetails,
    StreamingServer,
    Player,
}

#[derive(Clone, Debug, Serialize)]
pub struct DesktopModel {
    pub ctx: Ctx,
    pub auth_link: Link<LinkAuthKey>,
    pub data_export: DataExport,
    pub continue_watching_preview: ContinueWatchingPreview,
    pub board: CatalogsWithExtra,
    pub discover: CatalogWithFilters<MetaItemPreview>,
    pub library: LibraryWithFilters<NotRemovedFilter>,
    pub continue_watching: LibraryWithFilters<ContinueWatchingFilter>,
    pub calendar: Calendar,
    pub live_tv_guide: LiveTvGuide,
    pub live_tv_continue_watching: LiveTvContinueWatching,
    pub search: CatalogsWithExtra,
    pub local_search: LocalSearch,
    pub meta_details: MetaDetails,
    pub remote_addons: CatalogWithFilters<Descriptor>,
    pub installed_addons: InstalledAddonsWithFilters,
    pub addon_details: AddonDetails,
    pub streaming_server: StreamingServer,
    pub player: Player,
    #[serde(skip)]
    pub(crate) mutation_queue: ModelMutationQueue,
}

impl Model<DesktopEnv> for DesktopModel {
    type Field = DesktopModelField;

    fn update(&mut self, msg: &Msg) -> (Vec<Effect>, Vec<Self::Field>) {
        let mut effects = Vec::new();
        let mut fields = Vec::new();

        let ctx_effects = Update::<DesktopEnv>::update(&mut self.ctx, msg);
        if ctx_effects.has_changed {
            fields.push(DesktopModelField::Ctx);
        }
        effects.extend(ctx_effects);

        macro_rules! update_submodel {
            ($field_expr:expr, $variant:ident) => {{
                let sub_effects =
                    UpdateWithCtx::<DesktopEnv>::update(&mut $field_expr, msg, &self.ctx);
                if sub_effects.has_changed {
                    fields.push(DesktopModelField::$variant);
                }
                effects.extend(sub_effects);
            }};
        }

        update_submodel!(self.auth_link, AuthLink);
        update_submodel!(self.data_export, DataExport);
        update_submodel!(self.continue_watching_preview, ContinueWatchingPreview);
        update_submodel!(self.board, Board);
        update_submodel!(self.discover, Discover);
        update_submodel!(self.library, Library);
        update_submodel!(self.continue_watching, ContinueWatching);
        update_submodel!(self.calendar, Calendar);
        update_submodel!(self.live_tv_guide, LiveTvGuide);
        update_submodel!(self.live_tv_continue_watching, LiveTvContinueWatching);
        update_submodel!(self.search, Search);
        update_submodel!(self.local_search, LocalSearch);
        update_submodel!(self.meta_details, MetaDetails);
        update_submodel!(self.remote_addons, RemoteAddons);
        update_submodel!(self.installed_addons, InstalledAddons);
        update_submodel!(self.addon_details, AddonDetails);
        update_submodel!(self.streaming_server, StreamingServer);
        update_submodel!(self.player, Player);

        (effects, fields)
    }

    fn update_field(&mut self, msg: &Msg, field: &Self::Field) -> (Vec<Effect>, Vec<Self::Field>) {
        if matches!(
            (field, msg),
            (
                DesktopModelField::DataExport,
                Msg::Action(Action::Link(ActionLink::ReadData))
            )
        ) {
            if let Some(custom_mutation) = self.mutation_queue.pop() {
                return custom_mutation(self);
            }
        }

        macro_rules! update_single_field {
            ($field_expr:expr, $variant:ident) => {{
                let sub_effects =
                    UpdateWithCtx::<DesktopEnv>::update(&mut $field_expr, msg, &self.ctx);
                let changed = if sub_effects.has_changed {
                    vec![DesktopModelField::$variant]
                } else {
                    vec![]
                };
                (sub_effects.into_iter().collect(), changed)
            }};
        }

        match field {
            DesktopModelField::Ctx => {
                let ctx_effects = Update::<DesktopEnv>::update(&mut self.ctx, msg);
                let changed = if ctx_effects.has_changed {
                    vec![DesktopModelField::Ctx]
                } else {
                    vec![]
                };
                (ctx_effects.into_iter().collect(), changed)
            }
            DesktopModelField::AuthLink => update_single_field!(self.auth_link, AuthLink),
            DesktopModelField::DataExport => update_single_field!(self.data_export, DataExport),
            DesktopModelField::ContinueWatchingPreview => {
                update_single_field!(self.continue_watching_preview, ContinueWatchingPreview)
            }
            DesktopModelField::Board => update_single_field!(self.board, Board),
            DesktopModelField::Discover => update_single_field!(self.discover, Discover),
            DesktopModelField::Library => update_single_field!(self.library, Library),
            DesktopModelField::ContinueWatching => {
                update_single_field!(self.continue_watching, ContinueWatching)
            }
            DesktopModelField::Calendar => update_single_field!(self.calendar, Calendar),
            DesktopModelField::LiveTvGuide => update_single_field!(self.live_tv_guide, LiveTvGuide),
            DesktopModelField::LiveTvContinueWatching => {
                update_single_field!(self.live_tv_continue_watching, LiveTvContinueWatching)
            }
            DesktopModelField::Search => update_single_field!(self.search, Search),
            DesktopModelField::LocalSearch => update_single_field!(self.local_search, LocalSearch),
            DesktopModelField::MetaDetails => update_single_field!(self.meta_details, MetaDetails),
            DesktopModelField::RemoteAddons => {
                update_single_field!(self.remote_addons, RemoteAddons)
            }
            DesktopModelField::InstalledAddons => {
                update_single_field!(self.installed_addons, InstalledAddons)
            }
            DesktopModelField::AddonDetails => {
                update_single_field!(self.addon_details, AddonDetails)
            }
            DesktopModelField::StreamingServer => {
                update_single_field!(self.streaming_server, StreamingServer)
            }
            DesktopModelField::Player => update_single_field!(self.player, Player),
        }
    }
}

impl DesktopModel {
    pub fn new(
        profile: Profile,
        library: LibraryBucket,
        streams: StreamsBucket,
        server_urls: ServerUrlsBucket,
        notifications: NotificationsBucket,
        search_history: SearchHistoryBucket,
        dismissed_events: DismissedEventsBucket,
    ) -> (Self, Effects) {
        let (continue_watching_preview, continue_watching_preview_effects) =
            ContinueWatchingPreview::new(&library, &notifications);
        let (discover, discover_effects) = CatalogWithFilters::<MetaItemPreview>::new(&profile);
        let (library_model, library_effects) =
            LibraryWithFilters::<NotRemovedFilter>::new(&library, &notifications);
        let (continue_watching, continue_watching_effects) =
            LibraryWithFilters::<ContinueWatchingFilter>::new(&library, &notifications);
        let (remote_addons, remote_addons_effects) =
            CatalogWithFilters::<Descriptor>::new(&profile);
        let (installed_addons, installed_addons_effects) =
            InstalledAddonsWithFilters::new(&profile);
        let (streaming_server, streaming_server_effects) =
            StreamingServer::new::<DesktopEnv>(&profile);
        let (local_search, local_search_effects) = LocalSearch::new::<DesktopEnv>();

        let model = Self {
            ctx: Ctx::new(
                profile,
                library,
                streams,
                server_urls,
                notifications,
                search_history,
                dismissed_events,
            ),
            auth_link: Default::default(),
            data_export: Default::default(),
            continue_watching_preview,
            board: Default::default(),
            discover,
            library: library_model,
            continue_watching,
            calendar: Default::default(),
            live_tv_guide: Default::default(),
            live_tv_continue_watching: Default::default(),
            search: Default::default(),
            local_search,
            meta_details: Default::default(),
            remote_addons,
            installed_addons,
            addon_details: Default::default(),
            streaming_server,
            player: Player {
                collect_seek_logs: true,
                ..Default::default()
            },
            mutation_queue: ModelMutationQueue::default(),
        };

        (
            model,
            continue_watching_preview_effects
                .join(discover_effects)
                .join(library_effects)
                .join(continue_watching_effects)
                .join(remote_addons_effects)
                .join(installed_addons_effects)
                .join(streaming_server_effects)
                .join(local_search_effects),
        )
    }

    pub fn from_buckets(buckets: PersistedBuckets) -> (Self, Effects) {
        Self::new(
            buckets.profile,
            buckets.library,
            buckets.streams,
            buckets.server_urls,
            buckets.notifications,
            buckets.search_history,
            buckets.dismissed_events,
        )
    }
}

pub fn clone_model_field(field: &DesktopModelField) -> DesktopModelField {
    *field
}
