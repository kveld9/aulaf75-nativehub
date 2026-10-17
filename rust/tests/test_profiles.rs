//! Hardware-free unit and integration tests for local lighting profiles.

use std::fs;
use std::path::PathBuf;

use aula_f75::cli::profile::{
    delete_profile, list_profiles, load_profile, parse_from_str, resolve_profiles_dir,
    save_profile, AppliedProfileParams, Profile,
};
use aula_f75::effects::Effect;

static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> Self {
        let unique = format!(
            "aula_f75_test_{}_{}_{}",
            prefix,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let path = std::env::temp_dir().join(unique);
        fs::create_dir_all(&path).expect("Failed to create temp dir");
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[test]
fn test_save_load_roundtrip_effect() {
    let tmp = TempDir::new("roundtrip_fx");
    let prof = Profile::new("gamer_rainbow", "rainbow", None, 0.85, Vec::new());

    let saved_path = save_profile(&prof, Some(&tmp.path)).expect("Failed to save profile");
    assert!(saved_path.is_file());
    assert_eq!(
        saved_path.file_name().unwrap().to_str().unwrap(),
        "gamer_rainbow.toml"
    );

    let loaded = load_profile("gamer_rainbow", Some(&tmp.path)).expect("Failed to load profile");
    assert_eq!(loaded.name, "gamer_rainbow");
    assert_eq!(loaded.effect, "rainbow");
    assert_eq!(loaded.color, None);
    assert!((loaded.brightness - 0.85).abs() < 0.01);
    assert!(loaded.keys.is_empty());
}

#[test]
fn test_save_load_roundtrip_solid_color() {
    let tmp = TempDir::new("roundtrip_solid");
    let prof = Profile::new(
        "sunset_solid",
        "solid",
        Some((255, 128, 0)),
        0.50,
        Vec::new(),
    );

    save_profile(&prof, Some(&tmp.path)).expect("Failed to save profile");
    let loaded = load_profile("sunset_solid", Some(&tmp.path)).expect("Failed to load profile");
    assert_eq!(loaded.name, "sunset_solid");
    assert_eq!(loaded.effect, "solid");
    assert_eq!(loaded.color, Some((255, 128, 0)));
    assert!((loaded.brightness - 0.50).abs() < 0.01);
    assert!(loaded.keys.is_empty());
}

#[test]
fn test_save_load_roundtrip_per_key() {
    let tmp = TempDir::new("roundtrip_keys");
    let keys = vec![
        ("esc".to_string(), (255, 0, 0)),
        ("w".to_string(), (0, 255, 0)),
        ("a".to_string(), (0, 255, 0)),
        ("s".to_string(), (0, 255, 0)),
        ("d".to_string(), (0, 255, 0)),
        ("space".to_string(), (0, 0, 255)),
    ];
    let prof = Profile::new("fps_wasd", "off", None, 1.0, keys);

    save_profile(&prof, Some(&tmp.path)).expect("Failed to save profile");
    let loaded = load_profile("fps_wasd", Some(&tmp.path)).expect("Failed to load profile");
    assert_eq!(loaded.name, "fps_wasd");
    assert_eq!(loaded.effect, "off");
    assert_eq!(loaded.keys.len(), 6);
    assert_eq!(loaded.keys[0], ("esc".to_string(), (255, 0, 0)));
    assert_eq!(loaded.keys[1], ("w".to_string(), (0, 255, 0)));
    assert_eq!(loaded.keys[5], ("space".to_string(), (0, 0, 255)));
}

#[test]
fn test_list_profiles_alphabetical() {
    let tmp = TempDir::new("list_profiles");
    assert_eq!(
        list_profiles(Some(&tmp.path)).unwrap(),
        Vec::<String>::new()
    );

    let p1 = Profile::new("charlie", "rainbow", None, 1.0, Vec::new());
    let p2 = Profile::new("alpha", "wave", None, 1.0, Vec::new());
    let p3 = Profile::new("bravo", "stars", None, 1.0, Vec::new());

    save_profile(&p1, Some(&tmp.path)).unwrap();
    save_profile(&p2, Some(&tmp.path)).unwrap();
    save_profile(&p3, Some(&tmp.path)).unwrap();

    let list = list_profiles(Some(&tmp.path)).unwrap();
    assert_eq!(list, vec!["alpha", "bravo", "charlie"]);
}

#[test]
fn test_delete_profile() {
    let tmp = TempDir::new("delete_profile");
    let prof = Profile::new("temporary", "rainbow", None, 1.0, Vec::new());

    save_profile(&prof, Some(&tmp.path)).unwrap();
    assert!(load_profile("temporary", Some(&tmp.path)).is_ok());

    delete_profile("temporary", Some(&tmp.path)).expect("Failed to delete profile");
    let load_err = load_profile("temporary", Some(&tmp.path));
    assert!(load_err.is_err());
    assert!(load_err.unwrap_err().contains("not found"));

    let del_again = delete_profile("temporary", Some(&tmp.path));
    assert!(del_again.is_err());
    assert!(del_again.unwrap_err().contains("not found"));
}

#[test]
fn test_validation_rejects_bad_effect() {
    let prof = Profile::new("bad_eff", "super_hyper_fx", None, 1.0, Vec::new());
    let err = prof.validate();
    assert!(err.is_err());
    assert!(err.unwrap_err().contains("Unknown effect name"));
}

#[test]
fn test_validation_rejects_bad_brightness() {
    let over = Profile::new("over", "rainbow", None, 1.5, Vec::new());
    assert!(over.validate().is_err());

    let under = Profile::new("under", "rainbow", None, -0.1, Vec::new());
    assert!(under.validate().is_err());

    let nan = Profile::new("nan", "rainbow", None, f32::NAN, Vec::new());
    assert!(nan.validate().is_err());
}

#[test]
fn test_validation_rejects_unknown_key() {
    let prof = Profile::new(
        "bad_key",
        "off",
        None,
        1.0,
        vec![("fake_button_9999".to_string(), (255, 0, 0))],
    );
    let err = prof.validate();
    assert!(err.is_err());
    assert!(err.unwrap_err().contains("Unknown key name"));
}

#[test]
fn test_validation_solid_without_color_rejected() {
    let prof = Profile::new("solid_no_color", "solid", None, 1.0, Vec::new());
    let err = prof.validate();
    assert!(err.is_err());
    assert!(err.unwrap_err().contains("requires --color RRGGBB"));
}

#[test]
fn test_strict_parser_errors_name_file_and_line() {
    let file = "/etc/aula/test.toml";

    // 1. Missing '=' syntax error
    let content_bad_syntax = "name = valid\nnot_a_key_value_line\neffect = rainbow\n";
    let err = parse_from_str(content_bad_syntax, file).unwrap_err();
    assert!(
        err.contains("/etc/aula/test.toml:2: Missing '=' in key=value pair"),
        "Expected filename and line 2 in error, got: {}",
        err
    );

    // 2. Bad hex color syntax
    let content_bad_hex = "name = valid\neffect = solid\ncolor = NOT_HEX\n";
    let err = parse_from_str(content_bad_hex, file).unwrap_err();
    assert!(
        err.contains("/etc/aula/test.toml:3:"),
        "Expected line 3 in error, got: {}",
        err
    );
    assert!(err.contains("hex"));

    // 3. Bad brightness float
    let content_bad_b = "name = valid\neffect = rainbow\nbrightness = invalid_float\n";
    let err = parse_from_str(content_bad_b, file).unwrap_err();
    assert!(
        err.contains("/etc/aula/test.toml:3: Invalid brightness"),
        "Expected line 3 in error, got: {}",
        err
    );

    // 4. Unknown config key
    let content_unknown_key = "name = valid\neffect = rainbow\nunrecognized_field = 42\n";
    let err = parse_from_str(content_unknown_key, file).unwrap_err();
    assert!(
        err.contains("/etc/aula/test.toml:3: Unknown configuration key"),
        "Expected line 3 in error, got: {}",
        err
    );

    // 5. Empty key after 'key.'
    let content_empty_key = "name = valid\neffect = off\nkey. = FF0000\n";
    let err = parse_from_str(content_empty_key, file).unwrap_err();
    assert!(
        err.contains("/etc/aula/test.toml:3: Empty key name after 'key.'"),
        "Expected line 3 in error, got: {}",
        err
    );
}

#[test]
fn test_load_applies_via_fake_runner() {
    // 1. Effect profile assembled params
    let wave_prof = Profile::new("my_wave", "wave", None, 0.75, Vec::new());
    let mut recorded_effect = None;

    wave_prof
        .apply_with_runner(Some(12.5), |params| {
            recorded_effect = Some(params.clone());
            Ok(())
        })
        .expect("Apply runner failed");

    match recorded_effect {
        Some(AppliedProfileParams::Effect {
            effect,
            brightness,
            seconds,
        }) => {
            assert_eq!(effect, Effect::Wave { period_s: 3.0 });
            assert!((brightness - 0.75).abs() < 0.01);
            assert_eq!(seconds, Some(12.5));
        }
        _ => panic!("Expected AppliedProfileParams::Effect"),
    }

    // 2. Per-key profile assembled params with brightness scaling
    let keys_prof = Profile::new(
        "my_keys",
        "off",
        None,
        0.5,
        vec![("esc".to_string(), (200, 100, 50))],
    );
    let mut recorded_keys = None;

    keys_prof
        .apply_with_runner(None, |params| {
            recorded_keys = Some(params.clone());
            Ok(())
        })
        .expect("Apply runner failed");

    match recorded_keys {
        Some(AppliedProfileParams::Keys { pairs, seconds }) => {
            assert_eq!(pairs.len(), 1);
            assert_eq!(pairs[0].0, "esc");
            // Scaled 0.5: 200*0.5=100, 100*0.5=50, 50*0.5=25
            assert_eq!(pairs[0].1, (100, 50, 25));
            assert_eq!(seconds, None);
        }
        _ => panic!("Expected AppliedProfileParams::Keys"),
    }
}

#[test]
fn test_env_override_aula_f75_config() {
    let _lock = ENV_LOCK.lock().unwrap();
    let tmp = TempDir::new("env_override");
    let prev_env = std::env::var("AULA_F75_CONFIG").ok();

    std::env::set_var("AULA_F75_CONFIG", &tmp.path);

    let resolved = resolve_profiles_dir(None).unwrap();
    assert_eq!(resolved, tmp.path.join("profiles"));

    let prof = Profile::new("env_test", "rainbow", None, 1.0, Vec::new());
    save_profile(&prof, None).expect("save_profile via env failed");

    let list = list_profiles(None).expect("list_profiles via env failed");
    assert_eq!(list, vec!["env_test"]);

    let loaded = load_profile("env_test", None).expect("load_profile via env failed");
    assert_eq!(loaded.name, "env_test");

    delete_profile("env_test", None).expect("delete_profile via env failed");
    assert!(list_profiles(None).unwrap().is_empty());

    // Restore environment variable
    match prev_env {
        Some(v) => std::env::set_var("AULA_F75_CONFIG", v),
        None => std::env::remove_var("AULA_F75_CONFIG"),
    }
}

#[test]
fn test_gui_profiles_lifecycle() {
    let _lock = ENV_LOCK.lock().unwrap();
    let tmp = TempDir::new("gui_lifecycle");
    let prev_env = std::env::var("AULA_F75_CONFIG").ok();
    std::env::set_var("AULA_F75_CONFIG", &tmp.path);

    let mut app = aula_f75::gui::AulaF75App::default();
    assert_eq!(app.saved_profiles, Vec::<String>::new());

    // Save profile from GUI state
    app.profile_name_input = "gui_preset".to_string();
    app.selected_effect = "Rainbow".to_string();
    app.brightness = 0.8;
    app.save_current_profile();

    assert!(app.saved_profiles.contains(&"gui_preset".to_string()));
    assert_eq!(app.selected_profile, Some("gui_preset".to_string()));
    assert!(app.profile_name_input.is_empty());
    assert!(app.status_text.contains("saved"));

    // Apply profile from GUI
    app.on_apply_profile_clicked();
    assert!(app.status_text.contains("applied"));

    // Delete profile from GUI
    app.on_delete_profile_clicked();
    assert!(!app.saved_profiles.contains(&"gui_preset".to_string()));
    assert_eq!(app.selected_profile, None);
    assert!(app.status_text.contains("deleted"));

    // Restore environment variable
    match prev_env {
        Some(v) => std::env::set_var("AULA_F75_CONFIG", v),
        None => std::env::remove_var("AULA_F75_CONFIG"),
    }
}
