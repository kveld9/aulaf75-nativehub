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
        let trimmed_name = self.name.trim();
        if trimmed_name.is_empty() {
            return Err("Profile name cannot be empty".to_string());
        }
        if trimmed_name.contains('/') || trimmed_name.contains('\\') || trimmed_name.contains("..") {
            return Err(format!(
                "Profile name {:?} contains invalid path characters",
                self.name
            ));
        }

        if !(0.0..=1.0).contains(&self.brightness) || self.brightness.is_nan() {
            return Err(format!(
                "Brightness must be between 0.0 and 1.0, got {}",
                self.brightness
            ));
        }

        if !(0.25..=3.0).contains(&self.speed) || self.speed.is_nan() {
            return Err(format!(
                "Speed must be between 0.25 and 3.0, got {}",
                self.speed
            ));
        }

        let eff_lower = self.effect.trim().to_ascii_lowercase();
        if eff_lower == "custom" {
            if self.keys.is_empty() {
                return Err("Effect 'custom' requires at least one per-key override".to_string());
            }
        } else {
            let rgb_opt = self.color.map(|c| RGB(c.0, c.1, c.2));
            Effect::from_name_with_speed(&eff_lower, rgb_opt, self.speed)?;
        }

        for (key_name, _) in &self.keys {
            if proto::slot_for_key(key_name).is_none() {
                return Err(format!("Unknown key name: {:?}", key_name));
            }
        }

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

/// Parse a profile from a flat `key = value` text representation.
///
/// Parsing is strict: syntax errors or unknown keys return descriptive errors
/// indicating the originating file label and line number.
pub fn parse_from_str(content: &str, file_label: &str) -> Result<Profile, String> {
    let mut name: Option<String> = None;
    let mut effect: Option<String> = None;
    let mut color: Option<(u8, u8, u8)> = None;
    let mut brightness: Option<f32> = None;
    let mut speed: Option<f32> = None;
    let mut keys: Vec<(String, (u8, u8, u8))> = Vec::new();

    for (idx, line) in content.lines().enumerate() {
        let line_num = idx + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let (k_raw, v_raw) = trimmed.split_once('=').ok_or_else(|| {
            format!(
                "{}:{}: Missing '=' in key=value pair: {:?}",
                file_label, line_num, line
            )
        })?;

        let k = k_raw.trim();
        let v = v_raw.trim();

        if k.is_empty() {
            return Err(format!(
                "{}:{}: Empty key name in line {:?}",
                file_label, line_num, line
            ));
        }

        if k.eq_ignore_ascii_case("name") {
            name = Some(v.to_string());
        } else if k.eq_ignore_ascii_case("effect") {
            effect = Some(v.to_string());
        } else if k.eq_ignore_ascii_case("color") {
            let rgb =
                parse_color_hex(v).map_err(|e| format!("{}:{}: {}", file_label, line_num, e))?;
            color = Some(rgb);
        } else if k.eq_ignore_ascii_case("brightness") {
            let b = v.parse::<f32>().map_err(|e| {
                format!(
                    "{}:{}: Invalid brightness {:?}: {}",
                    file_label, line_num, v, e
                )
            })?;
            brightness = Some(b);
        } else if k.eq_ignore_ascii_case("speed") {
            let s = v.parse::<f32>().map_err(|e| {
                format!(
                    "{}:{}: Invalid speed {:?}: {}",
                    file_label, line_num, v, e
                )
            })?;
            speed = Some(s);
        } else if let Some(key_name) = k.strip_prefix("key.") {
            let key_name = key_name.trim();
            if key_name.is_empty() {
                return Err(format!(
                    "{}:{}: Empty key name after 'key.'",
                    file_label, line_num
                ));
            }
            let rgb =
                parse_color_hex(v).map_err(|e| format!("{}:{}: {}", file_label, line_num, e))?;
            keys.push((key_name.to_string(), rgb));
        } else {
            return Err(format!(
                "{}:{}: Unknown configuration key {:?}",
                file_label, line_num, k
            ));
        }
    }

    let effect = effect
        .ok_or_else(|| format!("{}: Missing required field 'effect'", file_label))?;

    let name = name.unwrap_or_else(|| {
        Path::new(file_label)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unnamed")
            .to_string()
    });

    let brightness = brightness.unwrap_or(1.0);
    let speed = speed.unwrap_or(1.0);

    let profile = Profile {
        name,
        effect,
        color,
        brightness,
        speed,
        keys,
    };

    profile
        .validate()
        .map_err(|e| format!("{}: Validation error: {}", file_label, e))?;

    Ok(profile)
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
    fs::remove_file(&file_path)
        .map_err(|e| format!("Failed to delete profile {}: {}", file_path.display(), e))?;
    Ok(())
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
}
