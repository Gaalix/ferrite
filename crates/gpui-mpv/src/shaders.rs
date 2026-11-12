use std::fs;
use std::path::{Path, PathBuf};

use crate::MpvError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DebandConfig {
    pub enabled: bool,
    pub iterations: u8,
    pub threshold: u16,
    pub range: u8,
    pub grain: u16,
}

impl Default for DebandConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            iterations: 1,
            threshold: 48,
            range: 16,
            grain: 32,
        }
    }
}

impl DebandConfig {
    pub const fn enabled_balanced() -> Self {
        Self {
            enabled: true,
            iterations: 2,
            threshold: 48,
            range: 16,
            grain: 32,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HdrBackendOptions {
    pub dolby_vision_reshape: bool,
    pub target_colorspace_hint: bool,
    pub hdr_compute_peak: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ShaderPreset {
    #[default]
    Off,
    KrigBilateral,
    FsrcnnxLine,
    Anime4kFast,
    Anime4kHq,
}

impl ShaderPreset {
    pub const ALL: [ShaderPreset; 5] = [
        Self::Off,
        Self::KrigBilateral,
        Self::FsrcnnxLine,
        Self::Anime4kFast,
        Self::Anime4kHq,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::KrigBilateral => "KrigBilateral Chroma",
            Self::FsrcnnxLine => "FSRCNNX Line Sharpen",
            Self::Anime4kFast => "Anime4K Fast",
            Self::Anime4kHq => "Anime4K HQ",
        }
    }

    pub const fn filename(self) -> Option<&'static str> {
        match self {
            Self::Off => None,
            Self::KrigBilateral => Some("krig_bilateral_chroma.glsl"),
            Self::FsrcnnxLine => Some("fsrcnnx_line_enhance.glsl"),
            Self::Anime4kFast => Some("anime4k_fast.glsl"),
            Self::Anime4kHq => Some("anime4k_hq.glsl"),
        }
    }

    pub const fn glsl_source(self) -> Option<&'static str> {
        match self {
            Self::Off => None,
            Self::KrigBilateral => Some(SHADER_KRIG_BILATERAL),
            Self::FsrcnnxLine => Some(SHADER_FSRCNNX_LINE),
            Self::Anime4kFast => Some(SHADER_ANIME4K_FAST),
            Self::Anime4kHq => Some(SHADER_ANIME4K_HQ),
        }
    }
}

pub fn default_shader_cache_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA").filter(|s| !s.is_empty()) {
        return PathBuf::from(local_app_data)
            .join("ferrite")
            .join("cache")
            .join("shaders");
    }

    if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME").filter(|s| !s.is_empty()) {
        PathBuf::from(xdg).join("ferrite").join("shaders")
    } else if let Some(home) = std::env::var_os("HOME").filter(|s| !s.is_empty()) {
        PathBuf::from(home)
            .join(".cache")
            .join("ferrite")
            .join("shaders")
    } else {
        std::env::temp_dir().join("ferrite").join("shaders")
    }
}

pub fn materialize_shader_preset_in_dir(
    preset: ShaderPreset,
    cache_dir: &Path,
) -> Result<Option<PathBuf>, MpvError> {
    let (Some(filename), Some(source)) = (preset.filename(), preset.glsl_source()) else {
        return Ok(None);
    };

    fs::create_dir_all(cache_dir).map_err(|e| {
        MpvError::Property(format!(
            "failed to create shader cache dir {}: {e}",
            cache_dir.display()
        ))
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(cache_dir, fs::Permissions::from_mode(0o700));
    }

    let target_path = cache_dir.join(filename);
    let needs_write = match fs::read_to_string(&target_path) {
        Ok(existing) => existing != source,
        Err(_) => true,
    };

    if needs_write {
        fs::write(&target_path, source).map_err(|e| {
            MpvError::Property(format!(
                "failed to write GLSL shader {}: {e}",
                target_path.display()
            ))
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&target_path, fs::Permissions::from_mode(0o600));
        }
    }

    Ok(Some(target_path))
}

const SHADER_KRIG_BILATERAL: &str = r#"//!HOOK CHROMA
//!BIND HOOKED
//!BIND LUMA
//!DESC KrigBilateral Luma-Guided Chroma Reconstruction
//!WHEN CHROMA.w LUMA.w <

vec4 hook() {
    vec2 pos = HOOKED_pos;
    vec2 pt = HOOKED_pt;
    float luma_center = LUMA_tex(pos).x;
    vec4 accum = vec4(0.0);
    float w_total = 0.0;

    for (int dy = -1; dy <= 1; dy++) {
        for (int dx = -1; dx <= 1; dx++) {
            vec2 offset = vec2(float(dx), float(dy)) * pt;
            vec4 chroma_s = HOOKED_tex(pos + offset);
            float luma_s = LUMA_tex(pos + offset).x;
            float spatial_w = exp(-float(dx * dx + dy * dy) * 0.55);
            float range_w = exp(-abs(luma_s - luma_center) * 20.0);
            float w = spatial_w * range_w;
            accum += chroma_s * w;
            w_total += w;
        }
    }
    return accum / max(w_total, 1e-5);
}
"#;

const SHADER_FSRCNNX_LINE: &str = r#"//!HOOK MAIN
//!BIND HOOKED
//!DESC FSRCNNX-Style Edge-Directed Line Sharpening (Anti-Ringing)

float luma_of(vec3 c) {
    return dot(c, vec3(0.2126, 0.7152, 0.0722));
}

vec4 hook() {
    vec4 center = HOOKED_texOff(vec2(0.0, 0.0));
    vec3 n = HOOKED_texOff(vec2(0.0, -1.0)).rgb;
    vec3 s = HOOKED_texOff(vec2(0.0,  1.0)).rgb;
    vec3 w = HOOKED_texOff(vec2(-1.0, 0.0)).rgb;
    vec3 e = HOOKED_texOff(vec2( 1.0, 0.0)).rgb;
    vec3 nw = HOOKED_texOff(vec2(-1.0, -1.0)).rgb;
    vec3 ne = HOOKED_texOff(vec2( 1.0, -1.0)).rgb;
    vec3 sw = HOOKED_texOff(vec2(-1.0,  1.0)).rgb;
    vec3 se = HOOKED_texOff(vec2( 1.0,  1.0)).rgb;

    float gx = (luma_of(ne) + 2.0 * luma_of(e) + luma_of(se))
             - (luma_of(nw) + 2.0 * luma_of(w) + luma_of(sw));
    float gy = (luma_of(sw) + 2.0 * luma_of(s) + luma_of(se))
             - (luma_of(nw) + 2.0 * luma_of(n) + luma_of(ne));
    float edge_mag = clamp(length(vec2(gx, gy)) * 1.8, 0.0, 1.0);

    vec3 blur = (n + s + w + e) * 0.15 + (nw + ne + sw + se) * 0.10;
    float strength = mix(0.18, 0.55, edge_mag);
    vec3 sharpened = center.rgb + (center.rgb - blur) * strength;

    vec3 min_nb = min(min(min(n, s), min(w, e)), center.rgb);
    vec3 max_nb = max(max(max(n, s), max(w, e)), center.rgb);
    return vec4(clamp(sharpened, min_nb, max_nb), center.a);
}
"#;

const SHADER_ANIME4K_FAST: &str = r#"//!HOOK MAIN
//!BIND HOOKED
//!DESC Anime4K Fast Single-Pass Line Reconstruction

float get_luma(vec4 c) {
    return dot(c.rgb, vec3(0.299, 0.587, 0.114));
}

vec4 hook() {
    vec4 cc = HOOKED_texOff(vec2(0.0, 0.0));
    vec4 t  = HOOKED_texOff(vec2(0.0, -1.0));
    vec4 b  = HOOKED_texOff(vec2(0.0,  1.0));
    vec4 l  = HOOKED_texOff(vec2(-1.0, 0.0));
    vec4 r  = HOOKED_texOff(vec2( 1.0, 0.0));

    float lc = get_luma(cc);
    float lt = get_luma(t);
    float lb = get_luma(b);
    float ll = get_luma(l);
    float lr = get_luma(r);

    vec4 lightest = cc;
    float max_l = lc;
    if (lt > max_l) { max_l = lt; lightest = t; }
    if (lb > max_l) { max_l = lb; lightest = b; }
    if (ll > max_l) { max_l = ll; lightest = l; }
    if (lr > max_l) { max_l = lr; lightest = r; }

    vec4 darkest = cc;
    float min_l = lc;
    if (lt < min_l) { min_l = lt; darkest = t; }
    if (lb < min_l) { min_l = lb; darkest = b; }
    if (ll < min_l) { min_l = ll; darkest = l; }
    if (lr < min_l) { min_l = lr; darkest = r; }

    float avg_nb = 0.25 * (lt + lb + ll + lr);
    vec4 target = (lc < avg_nb) ? darkest : lightest;
    float edge = clamp(abs(max_l - min_l) * 1.6, 0.0, 1.0);
    return vec4(mix(cc.rgb, target.rgb, 0.32 * edge), cc.a);
}
"#;

const SHADER_ANIME4K_HQ: &str = r#"//!HOOK MAIN
//!BIND HOOKED
//!DESC Anime4K HQ Pass 1 - Surface Bilateral Denoise

vec4 hook() {
    vec4 cc = HOOKED_texOff(vec2(0.0, 0.0));
    vec3 sum = cc.rgb;
    float wsum = 1.0;

    for (int dy = -1; dy <= 1; dy++) {
        for (int dx = -1; dx <= 1; dx++) {
            if (dx == 0 && dy == 0) continue;
            vec3 sample_rgb = HOOKED_texOff(vec2(float(dx), float(dy))).rgb;
            vec3 diff = sample_rgb - cc.rgb;
            float w = exp(-dot(diff, diff) * 90.0) * 0.45;
            sum += sample_rgb * w;
            wsum += w;
        }
    }
    return vec4(sum / wsum, cc.a);
}

//!HOOK MAIN
//!BIND HOOKED
//!DESC Anime4K HQ Pass 2 - Directional Line Thinning & Crisp Restore

float a4k_luma(vec4 c) {
    return dot(c.rgb, vec3(0.299, 0.587, 0.114));
}

vec4 hook() {
    vec4 cc = HOOKED_texOff(vec2(0.0, 0.0));
    vec4 n  = HOOKED_texOff(vec2(0.0, -1.0));
    vec4 s  = HOOKED_texOff(vec2(0.0,  1.0));
    vec4 w  = HOOKED_texOff(vec2(-1.0, 0.0));
    vec4 e  = HOOKED_texOff(vec2( 1.0, 0.0));
    vec4 nw = HOOKED_texOff(vec2(-1.0, -1.0));
    vec4 ne = HOOKED_texOff(vec2( 1.0, -1.0));
    vec4 sw = HOOKED_texOff(vec2(-1.0,  1.0));
    vec4 se = HOOKED_texOff(vec2( 1.0,  1.0));

    float lc = a4k_luma(cc);
    float gx = (a4k_luma(ne) + 2.0 * a4k_luma(e) + a4k_luma(se))
             - (a4k_luma(nw) + 2.0 * a4k_luma(w) + a4k_luma(sw));
    float gy = (a4k_luma(sw) + 2.0 * a4k_luma(s) + a4k_luma(se))
             - (a4k_luma(nw) + 2.0 * a4k_luma(n) + a4k_luma(ne));

    vec2 grad = vec2(gx, gy);
    float mag = length(grad);
    if (mag < 0.015) {
        return cc;
    }

    vec2 dir = grad / mag;
    vec4 pos_step = HOOKED_tex(HOOKED_pos + dir * HOOKED_pt);
    vec4 neg_step = HOOKED_tex(HOOKED_pos - dir * HOOKED_pt);

    float l_pos = a4k_luma(pos_step);
    float l_neg = a4k_luma(neg_step);
    float mid = 0.5 * (l_pos + l_neg);

    vec4 pushed = (lc < mid)
        ? ((l_pos < l_neg) ? pos_step : neg_step)
        : ((l_pos > l_neg) ? pos_step : neg_step);

    float strength = clamp(mag * 0.55, 0.0, 0.48);
    return vec4(mix(cc.rgb, pushed.rgb, strength), cc.a);
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_experimental_options_default_to_off() {
        assert!(!DebandConfig::default().enabled);
        assert_eq!(ShaderPreset::default(), ShaderPreset::Off);
        let hdr = HdrBackendOptions::default();
        assert!(!hdr.dolby_vision_reshape);
        assert!(!hdr.target_colorspace_hint);
        assert!(!hdr.hdr_compute_peak);
    }

    #[test]
    fn shader_presets_materialize_valid_hook_glsl_files() {
        let tmp_dir =
            std::env::temp_dir().join(format!("ferrite-shader-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp_dir);

        assert!(materialize_shader_preset_in_dir(ShaderPreset::Off, &tmp_dir)
            .unwrap()
            .is_none());

        for preset in [
            ShaderPreset::KrigBilateral,
            ShaderPreset::FsrcnnxLine,
            ShaderPreset::Anime4kFast,
            ShaderPreset::Anime4kHq,
        ] {
            let path = materialize_shader_preset_in_dir(preset, &tmp_dir)
                .unwrap()
                .expect("expected materialized shader path");
            let contents = fs::read_to_string(&path).expect("read shader file");
            assert!(contents.contains("//!HOOK "));
            assert!(contents.contains("//!BIND HOOKED"));
            assert!(contents.contains("vec4 hook()"));
        }

        let _ = fs::remove_dir_all(&tmp_dir);
    }
}
