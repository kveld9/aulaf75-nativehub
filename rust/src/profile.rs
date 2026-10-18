//! Local lighting profiles management for AULA F75.
//!
//! Provides file-based persistence for lighting effect configurations,
//! colors, brightness levels, and per-key overrides without cloud or vendor dependencies.
//! Profiles are stored as human-readable key-value flat files under `$XDG_CONFIG_HOME/aula-f75/profiles/`.

use std::fs;
use std::path::{Path, PathBuf};

use crate::cli::parse_color_hex;
use crate::effects::{Effect, RGB};
use crate::proto;

/// Lighting profile configuration schema.
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    /// Human-readable profile name (used as filename `<name>.toml`).
    pub name: String,
    /// Effect name (e.g. solid, rainbow, wave, breathe, stars, off, custom).
    pub effect: String,
    /// Base color triplet `(R, G, B)` for effects requiring color (solid, breathe).
    pub color: Option<(u8, u8, u8)>,
    /// Global brightness factor clamped to `0.0..=1.0`.
    pub brightness: f32,
    /// Animation speed factor clamped to `0.25..=3.0` (default 1.0).
    pub speed: f32,
    /// Optional per-key RGB color overrides `[(key_name, (R, G, B))]`.
    pub keys: Vec<(String, (u8, u8, u8))>,
}

/// Execution parameters prepared from a profile for dispatch to hardware or a test runner.
#[derive(Debug, Clone, PartialEq)]
pub enum AppliedProfileParams {
    /// Host-rendered effect parameters
    Effect {
        effect: Effect,
        brightness: f32,
        seconds: Option<f64>,
    },
    /// Per-key static color overrides
    Keys {
        pairs: Vec<(String, (u8, u8, u8))>,
        seconds: Option<f64>,
    },
}

fn validate_profile_name(name: &str) -> Result<(), String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("Profile name cannot be empty".to_string());
    }
    if trimmed.contains('/') || trimmed.contains('\\') || trimmed.contains("..") {
        return Err(format!(
            "Profile name {:?} contains invalid path characters",
            name
        ));
    }
    Ok(())
}

fn validate_brightness(brightness: f32) -> Result<(), String> {
    if !(0.0..=1.0).contains(&brightness) || brightness.is_nan() {
        Err(format!(
            "Brightness must be between 0.0 and 1.0, got {}",
            brightness
        ))
    } else {
        Ok(())
    }
}

fn validate_speed(speed: f32) -> Result<(), String> {
    if !(0.25..=3.0).contains(&speed) || speed.is_nan() {
        Err(format!(
            "Speed must be between 0.25 and 3.0, got {}",
            speed
        ))
    } else {
        Ok(())
    }
}

fn validate_effect(
    effect: &str,
    color: Option<(u8, u8, u8)>,
    speed: f32,
    has_keys: bool,
) -> Result<(), String> {
    let eff_lower = effect.trim().to_ascii_lowercase();
    if eff_lower == "custom" {
        if !has_keys {
            return Err("Effect 'custom' requires at least one per-key override".to_string());
        }
        Ok(())
    } else {
        let rgb_opt = color.map(|c| RGB(c.0, c.1, c.2));
        Effect::from_name_with_speed(&eff_lower, rgb_opt, speed)?;
        Ok(())
    }
}

fn validate_key_overrides(keys: &[(String, (u8, u8, u8))]) -> Result<(), String> {
    for (key_name, _) in keys {
        if proto::slot_for_key(key_name).is_none() {
            return Err(format!("Unknown key name: {:?}", key_name));
        }
    }
    Ok(())
}

impl Profile {
    /// Construct a new Profile instance with default speed 1.0.
    pub fn new(
        name: impl Into<String>,
        effect: impl Into<String>,
        color: Option<(u8, u8, u8)>,
        brightness: f32,
        keys: Vec<(String, (u8, u8, u8))>,
    ) -> Self {
        Self {
            name: name.into(),
            effect: effect.into(),
            color,
            brightness,
            speed: 1.0,
            keys,
        }
    }

    /// Construct a new Profile instance with explicit speed.
    pub fn new_with_speed(
        name: impl Into<String>,
        effect: impl Into<String>,
        color: Option<(u8, u8, u8)>,
        brightness: f32,
        speed: f32,
        keys: Vec<(String, (u8, u8, u8))>,
    ) -> Self {
        Self {
            name: name.into(),
            effect: effect.into(),
            color,
            brightness,
            speed,
            keys,
        }
    }

    /// Builder helper to set speed.
    pub fn with_speed(mut self, speed: f32) -> Self {
        self.speed = speed;
        self
    }

    /// Validate profile integrity:
    /// - Name is non-empty and contains no directory separators or traversal
    /// - Brightness is within `0.0..=1.0` and not NaN
    /// - Speed is within `0.25..=3.0` and not NaN
    /// - Effect name is recognized (solid, rainbow, wave, breathe, stars, off, or custom)
    /// - Solid and breathe effects require a valid base color
    /// - Custom effect requires at least one per-key override
    /// - All per-key names resolve to physical slots in `proto::slot_for_key`
    pub fn validate(&self) -> Result<(), String> {
        validate_profile_name(&self.name)?;
        validate_brightness(self.brightness)?;
        validate_speed(self.speed)?;
        validate_effect(&self.effect, self.color, self.speed, !self.keys.is_empty())?;
        validate_key_overrides(&self.keys)?;
        Ok(())
    }

    /// Serialize the profile into a clean, human-readable flat `key = value` format.
    pub fn serialize_to_string(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("name = {}\n", self.name.trim()));
        out.push_str(&format!(
            "effect = {}\n",
            self.effect.trim().to_ascii_lowercase()
        ));
        if let Some(c) = self.color {
            out.push_str(&format!("color = {:02X}{:02X}{:02X}\n", c.0, c.1, c.2));
        }
        out.push_str(&format!("brightness = {:.2}\n", self.brightness));
        out.push_str(&format!("speed = {:.2}\n", self.speed));
        for (k, c) in &self.keys {
            out.push_str(&format!(
                "key.{} = {:02X}{:02X}{:02X}\n",
                k.trim().to_ascii_lowercase(),
                c.0,
                c.1,
                c.2
            ));
        }
        out
    }

    /// Assemble execution parameters without performing hardware I/O.
    pub fn assemble_params(&self, seconds: Option<f64>) -> Result<AppliedProfileParams, String> {
        self.validate()?;
        if !self.keys.is_empty() {
            let pairs: Vec<(String, (u8, u8, u8))> = self
                .keys
                .iter()
                .map(|(k, c)| {
                    let scaled = RGB(c.0, c.1, c.2).scale(self.brightness).to_tuple();
                    (k.clone(), scaled)
                })
                .collect();
            Ok(AppliedProfileParams::Keys { pairs, seconds })
        } else {
            let eff_lower = self.effect.trim().to_ascii_lowercase();
            let effect = Effect::from_name_with_speed(
                &eff_lower,
                self.color.map(|c| RGB(c.0, c.1, c.2)),
                self.speed,
            )?;
            Ok(AppliedProfileParams::Effect {
                effect,
                brightness: self.brightness,
                seconds,
            })
        }
    }

    /// Apply profile using an injectable runner function.
    pub fn apply_with_runner<F, R>(&self, seconds: Option<f64>, mut runner: F) -> Result<R, String>
    where
        F: FnMut(&AppliedProfileParams) -> Result<R, String>,
    {
        let params = self.assemble_params(seconds)?;
        runner(&params)
    }
}

/// Resolve the directory storing profile flat files.
///
/// Priority:
/// 1. `base_dir` parameter override (used for unit tests and custom paths)
/// 2. `AULA_F75_CONFIG` environment variable override
/// 3. `$XDG_CONFIG_HOME/aula-f75/profiles/`
/// 4. Fallback `$HOME/.config/aula-f75/profiles/`
pub fn resolve_profiles_dir(base_dir: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(dir) = base_dir {
        return Ok(dir.to_path_buf());
    }

    if let Ok(env_cfg) = std::env::var("AULA_F75_CONFIG") {
        let trimmed = env_cfg.trim();
        if !trimmed.is_empty() {
            let path = PathBuf::from(trimmed);
            if path.file_name().and_then(|s| s.to_str()) == Some("profiles") {
                return Ok(path);
            } else {
                return Ok(path.join("profiles"));
            }
        }
    }

    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        let trimmed = xdg.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed).join("aula-f75").join("profiles"));
        }
    }

    if let Ok(home) = std::env::var("HOME") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(home)
                .join(".config")
                .join("aula-f75")
                .join("profiles"));
        }
    }

    Err("Could not resolve config directory: HOME and XDG_CONFIG_HOME are unset".to_string())
}

#[derive(Default)]
struct ProfileBuilder {
    name: Option<String>,
    effect: Option<String>,
    color: Option<(u8, u8, u8)>,
    brightness: Option<f32>,
    speed: Option<f32>,
    keys: Vec<(String, (u8, u8, u8))>,
}

fn parse_color_field(v: &str, file_label: &str, line_num: usize) -> Result<(u8, u8, u8), String> {
    parse_color_hex(v).map_err(|e| format!("{}:{}: {}", file_label, line_num, e))
}

fn parse_brightness_field(v: &str, file_label: &str, line_num: usize) -> Result<f32, String> {
    v.parse::<f32>()
        .map_err(|e| format!("{}:{}: Invalid brightness {:?}: {}", file_label, line_num, v, e))
}

fn parse_speed_field(v: &str, file_label: &str, line_num: usize) -> Result<f32, String> {
    v.parse::<f32>()
        .map_err(|e| format!("{}:{}: Invalid speed {:?}: {}", file_label, line_num, v, e))
}

fn parse_key_override_field(
    key_suffix: &str,
    v: &str,
    file_label: &str,
    line_num: usize,
) -> Result<(String, (u8, u8, u8)), String> {
    let key_name = key_suffix.trim();
    if key_name.is_empty() {
        return Err(format!(
            "{}:{}: Empty key name after 'key.'",
            file_label, line_num
        ));
    }
    let rgb = parse_color_hex(v).map_err(|e| format!("{}:{}: {}", file_label, line_num, e))?;
    Ok((key_name.to_string(), rgb))
}

fn parse_kv_line<'a>(
    line: &'a str,
    line_num: usize,
    file_label: &str,
) -> Result<Option<(&'a str, &'a str)>, String> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return Ok(None);
    }

    let (k_raw, v_raw) = trimmed.split_once('=').ok_or_else(|| {
        format!(
            "{}:{}: Missing '=' in key=value pair: {:?}",
            file_label, line_num, line
        )
    })?;

    let k = k_raw.trim();
    if k.is_empty() {
        return Err(format!(
            "{}:{}: Empty key name in line {:?}",
            file_label, line_num, line
        ));
    }

    Ok(Some((k, v_raw.trim())))
}

fn parse_standard_field(
    builder: &mut ProfileBuilder,
    k: &str,
    v: &str,
    file_label: &str,
    line_num: usize,
) -> Result<bool, String> {
    if k.eq_ignore_ascii_case("name") {
        builder.name = Some(v.to_string());
        Ok(true)
    } else if k.eq_ignore_ascii_case("effect") {
        builder.effect = Some(v.to_string());
        Ok(true)
    } else if k.eq_ignore_ascii_case("color") {
        builder.color = Some(parse_color_field(v, file_label, line_num)?);
        Ok(true)
    } else if k.eq_ignore_ascii_case("brightness") {
        builder.brightness = Some(parse_brightness_field(v, file_label, line_num)?);
        Ok(true)
    } else if k.eq_ignore_ascii_case("speed") {
        builder.speed = Some(parse_speed_field(v, file_label, line_num)?);
        Ok(true)
    } else {
        Ok(false)
    }
}

fn parse_profile_entry(
    builder: &mut ProfileBuilder,
    k: &str,
    v: &str,
    file_label: &str,
    line_num: usize,
) -> Result<(), String> {
    if parse_standard_field(builder, k, v, file_label, line_num)? {
        return Ok(());
    }

    if let Some(key_suffix) = k.strip_prefix("key.") {
        let pair = parse_key_override_field(key_suffix, v, file_label, line_num)?;
        builder.keys.push(pair);
        return Ok(());
    }

    Err(format!(
        "{}:{}: Unknown configuration key {:?}",
        file_label, line_num, k
    ))
}

fn resolve_fallback_profile_name(file_label: &str) -> String {
    Path::new(file_label)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unnamed")
        .to_string()
}

fn build_profile(builder: ProfileBuilder, file_label: &str) -> Result<Profile, String> {
    let effect = builder
        .effect
        .ok_or_else(|| format!("{}: Missing required field 'effect'", file_label))?;

    let name = builder
        .name
        .unwrap_or_else(|| resolve_fallback_profile_name(file_label));

    let brightness = builder.brightness.unwrap_or(1.0);
    let speed = builder.speed.unwrap_or(1.0);

    let profile = Profile {
        name,
        effect,
        color: builder.color,
        brightness,
        speed,
        keys: builder.keys,
    };

    profile
        .validate()
        .map_err(|e| format!("{}: Validation error: {}", file_label, e))?;

    Ok(profile)
}

/// Parse a profile from a flat `key = value` text representation.
///
/// Parsing is strict: syntax errors or unknown keys return descriptive errors
/// indicating the originating file label and line number.
pub fn parse_from_str(content: &str, file_label: &str) -> Result<Profile, String> {
    let mut builder = ProfileBuilder::default();

    for (idx, line) in content.lines().enumerate() {
        let line_num = idx + 1;
        if let Some((k, v)) = parse_kv_line(line, line_num, file_label)? {
            parse_profile_entry(&mut builder, k, v, file_label, line_num)?;
        }
    }

    build_profile(builder, file_label)
}

/// Save a profile to disk under the resolved profiles directory as `<name>.toml`.
pub fn save_profile(profile: &Profile, base_dir: Option<&Path>) -> Result<PathBuf, String> {
    profile.validate()?;
    let dir = resolve_profiles_dir(base_dir)?;
    fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create profiles directory {}: {}", dir.display(), e))?;

    let file_path = dir.join(format!("{}.toml", profile.name.trim()));
    let content = profile.serialize_to_string();
    fs::write(&file_path, content)
        .map_err(|e| format!("Failed to write profile {}: {}", file_path.display(), e))?;

    Ok(file_path)
}

/// Load a profile from disk by name from the resolved profiles directory.
pub fn load_profile(name: &str, base_dir: Option<&Path>) -> Result<Profile, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("Profile name cannot be empty".to_string());
    }
    let dir = resolve_profiles_dir(base_dir)?;
    let file_path = dir.join(format!("{}.toml", trimmed));
    if !file_path.is_file() {
        return Err(format!(
            "Profile {:?} not found at {}",
            trimmed,
            file_path.display()
        ));
    }
    let content = fs::read_to_string(&file_path)
        .map_err(|e| format!("Failed to read profile {}: {}", file_path.display(), e))?;

    let path_str = file_path.to_string_lossy();
    parse_from_str(&content, &path_str)
}

/// List all profile names (`<name>.toml`) saved in the resolved profiles directory.
pub fn list_profiles(base_dir: Option<&Path>) -> Result<Vec<String>, String> {
    let dir = resolve_profiles_dir(base_dir)?;
    if !dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut profiles = Vec::new();
    let entries = fs::read_dir(&dir)
        .map_err(|e| format!("Failed to read profiles directory {}: {}", dir.display(), e))?;

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("toml") {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                profiles.push(stem.to_string());
            }
        }
    }

    profiles.sort();
    Ok(profiles)
}

/// Default profile marker filename inside the profiles directory.
pub const DEFAULT_MARKER_FILENAME: &str = "default";

/// Resolve the default profile marker path inside the profiles directory.
fn default_marker_path(base_dir: Option<&Path>) -> Result<PathBuf, String> {
    let dir = resolve_profiles_dir(base_dir)?;
    Ok(dir.join(DEFAULT_MARKER_FILENAME))
}

/// Retrieve the currently configured default profile name, if any.
///
/// Returns `None` if the marker is missing, empty, contains only whitespace,
/// or points to a profile (`<name>.toml`) that does not exist on disk.
pub fn get_default_profile(base_dir: Option<&Path>) -> Option<String> {
    let marker_path = default_marker_path(base_dir).ok()?;
    if !marker_path.is_file() {
        return None;
    }
    let content = fs::read_to_string(&marker_path).ok()?;
    let name = content.trim();
    if name.is_empty() || validate_profile_name(name).is_err() {
        return None;
    }
    let dir = resolve_profiles_dir(base_dir).ok()?;
    let profile_file = dir.join(format!("{}.toml", name));
    if !profile_file.is_file() {
        return None;
    }
    Some(name.to_string())
}

/// Set a saved profile as the default startup profile.
///
/// The target profile must exist and pass validation via `load_profile`.
pub fn set_default_profile(name: &str, base_dir: Option<&Path>) -> Result<(), String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("Profile name cannot be empty".to_string());
    }
    // Verify target profile exists and loads validly
    load_profile(trimmed, base_dir)?;

    let dir = resolve_profiles_dir(base_dir)?;
    fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create profiles directory {}: {}", dir.display(), e))?;

    let marker_path = dir.join(DEFAULT_MARKER_FILENAME);
    fs::write(&marker_path, format!("{}\n", trimmed)).map_err(|e| {
        format!(
            "Failed to write default profile marker {}: {}",
            marker_path.display(),
            e
        )
    })?;
    Ok(())
}

/// Clear the default startup profile marker if present.
pub fn clear_default_profile(base_dir: Option<&Path>) -> Result<(), String> {
    let marker_path = default_marker_path(base_dir)?;
    if marker_path.exists() {
        fs::remove_file(&marker_path).map_err(|e| {
            format!(
                "Failed to remove default profile marker {}: {}",
                marker_path.display(),
                e
            )
        })?;
    }
    Ok(())
}

/// Delete a profile file `<name>.toml` from disk.
pub fn delete_profile(name: &str, base_dir: Option<&Path>) -> Result<(), String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("Profile name cannot be empty".to_string());
    }
    let dir = resolve_profiles_dir(base_dir)?;
    let file_path = dir.join(format!("{}.toml", trimmed));
    if !file_path.exists() {
        return Err(format!(
            "Profile {:?} not found at {}",
            trimmed,
            file_path.display()
        ));
    }
    let was_default = get_default_profile(base_dir).as_deref() == Some(trimmed);

    fs::remove_file(&file_path)
        .map_err(|e| format!("Failed to delete profile {}: {}", file_path.display(), e))?;

    if was_default {
        let _ = clear_default_profile(base_dir);
    }
    Ok(())
}

/// Format a display label indicating that a profile is the default startup profile.
pub fn format_default_label(name: &str) -> String {
    format!("{} (default)", name)
}

/// Check if a profile name matches the currently configured default profile.
pub fn is_default(name: &str, default_name: Option<&str>) -> bool {
    default_name == Some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_profile_speed_roundtrip() {
        let prof = Profile::new_with_speed("speedy_wave", "wave", None, 0.9, 2.5, Vec::new());
        let serialized = prof.serialize_to_string();
        assert!(serialized.contains("speed = 2.50\n"));

        let parsed = parse_from_str(&serialized, "speedy_wave.toml").expect("Failed to parse");
        assert_eq!(parsed.name, "speedy_wave");
        assert_eq!(parsed.effect, "wave");
        assert_eq!(parsed.speed, 2.5);
        assert_eq!(parsed.brightness, 0.9);
    }

    #[test]
    fn test_profile_backward_compatibility_without_speed() {
        let legacy_content = "name = legacy_profile\neffect = rainbow\nbrightness = 0.75\n";
        let parsed =
            parse_from_str(legacy_content, "legacy.toml").expect("Failed to parse legacy profile");
        assert_eq!(parsed.name, "legacy_profile");
        assert_eq!(parsed.effect, "rainbow");
        assert_eq!(parsed.brightness, 0.75);
        assert_eq!(parsed.speed, 1.0); // Defaults to 1.0 when missing
    }

    #[test]
    fn test_profile_speed_validation_range() {
        // Valid bounds
        assert!(Profile::new_with_speed("p", "rainbow", None, 1.0, 0.25, Vec::new())
            .validate()
            .is_ok());
        assert!(Profile::new_with_speed("p", "rainbow", None, 1.0, 1.0, Vec::new())
            .validate()
            .is_ok());
        assert!(Profile::new_with_speed("p", "rainbow", None, 1.0, 3.0, Vec::new())
            .validate()
            .is_ok());

        // Out of bounds (< 0.25)
        assert!(Profile::new_with_speed("p", "rainbow", None, 1.0, 0.24, Vec::new())
            .validate()
            .is_err());
        assert!(Profile::new_with_speed("p", "rainbow", None, 1.0, -1.0, Vec::new())
            .validate()
            .is_err());

        // Out of bounds (> 3.0)
        assert!(Profile::new_with_speed("p", "rainbow", None, 1.0, 3.01, Vec::new())
            .validate()
            .is_err());
        assert!(Profile::new_with_speed("p", "rainbow", None, 1.0, 5.0, Vec::new())
            .validate()
            .is_err());

        // NaN
        assert!(Profile::new_with_speed("p", "rainbow", None, 1.0, f32::NAN, Vec::new())
            .validate()
            .is_err());
    }

    #[test]
    fn test_profile_assemble_params_with_speed() {
        let prof = Profile::new_with_speed("fast_rainbow", "rainbow", None, 1.0, 2.0, Vec::new());
        let params = prof.assemble_params(None).expect("Failed to assemble params");
        match params {
            AppliedProfileParams::Effect {
                effect, brightness, ..
            } => {
                assert_eq!(effect, Effect::Rainbow { period_s: 2.5 });
                assert_eq!(brightness, 1.0);
            }
            _ => panic!("Expected Effect params"),
        }
    }

    #[test]
    fn test_format_default_label() {
        assert_eq!(format_default_label("solid_blue"), "solid_blue (default)");
        assert_eq!(format_default_label(""), " (default)");
        assert!(is_default("solid_blue", Some("solid_blue")));
        assert!(!is_default("solid_blue", Some("other")));
        assert!(!is_default("solid_blue", None));
    }
}
