//! Unit tests for CLI parsing and command dispatch.

use super::*;

#[test]
fn test_parse_color_hex_valid() {
    assert_eq!(parse_color_hex("ff0000").unwrap(), (255, 0, 0));
    assert_eq!(parse_color_hex("#00ff00").unwrap(), (0, 255, 0));
    assert_eq!(parse_color_hex("#0000FF").unwrap(), (0, 0, 255));
    assert_eq!(parse_color_hex("1A2B3C").unwrap(), (0x1A, 0x2B, 0x3C));
    assert_eq!(parse_color_hex(" #123456 ").unwrap(), (0x12, 0x34, 0x56));
}

#[test]
fn test_parse_color_hex_invalid() {
    assert!(parse_color_hex("fff").is_err());
    assert!(parse_color_hex("12345").is_err());
    assert!(parse_color_hex("1234567").is_err());
    assert!(parse_color_hex("12345g").is_err());
    assert!(parse_color_hex("#").is_err());
    assert!(parse_color_hex("").is_err());
}

#[test]
fn test_parse_hex_or_dec() {
    assert_eq!(parse_hex_or_dec("0x258A").unwrap(), 0x258A);
    assert_eq!(parse_hex_or_dec("0X010C").unwrap(), 0x010C);
    assert_eq!(parse_hex_or_dec("258a").unwrap(), 0x258A);
    assert_eq!(parse_hex_or_dec("9610").unwrap(), 9610);
    assert!(parse_hex_or_dec("zzzz").is_err());
}

#[test]
fn test_cli_subcommand_key_parse() {
    let parsed = Cli::try_parse_from(["aula-f75-rs", "key", "esc", "ff0000", "--seconds", "3"]);
    assert!(parsed.is_ok());
    if let Commands::Key { name, color, seconds } = parsed.unwrap().command {
        assert_eq!(name, "esc");
        assert_eq!(color, "ff0000");
        assert_eq!(seconds, Some(3.0));
    } else {
        panic!("Expected Commands::Key");
    }
}

#[test]
fn test_cli_subcommand_keys_parse() {
    let parsed = Cli::try_parse_from(["aula-f75-rs", "keys"]);
    assert!(parsed.is_ok());
    assert!(matches!(parsed.unwrap().command, Commands::Keys));
}

#[test]
fn test_cli_subcommand_fx_parse() {
    let parsed = Cli::try_parse_from([
        "aula-f75-rs",
        "fx",
        "rainbow",
        "--brightness",
        "0.8",
        "--seconds",
        "10",
    ]);
    assert!(parsed.is_ok());
    if let Commands::Fx {
        name,
        color,
        brightness,
        speed,
        seconds,
    } = parsed.unwrap().command
    {
        assert_eq!(name, "rainbow");
        assert_eq!(color, None);
        assert_eq!(brightness, 0.8);
        assert_eq!(speed, 1.0);
        assert_eq!(seconds, Some(10.0));
    } else {
        panic!("Expected Commands::Fx");
    }

    let parsed_breathe = Cli::try_parse_from([
        "aula-f75-rs",
        "fx",
        "breathe",
        "--color",
        "ff00ff",
    ]);
    assert!(parsed_breathe.is_ok());
    if let Commands::Fx {
        name,
        color,
        brightness,
        speed,
        seconds,
    } = parsed_breathe.unwrap().command
    {
        assert_eq!(name, "breathe");
        assert_eq!(color, Some("ff00ff".to_string()));
        assert_eq!(brightness, 1.0);
        assert_eq!(speed, 1.0);
        assert_eq!(seconds, None);
    } else {
        panic!("Expected Commands::Fx");
    }

    let parsed_speed = Cli::try_parse_from([
        "aula-f75-rs",
        "fx",
        "wave",
        "--speed",
        "2.5",
    ]);
    assert!(parsed_speed.is_ok());
    if let Commands::Fx {
        name,
        speed,
        ..
    } = parsed_speed.unwrap().command
    {
        assert_eq!(name, "wave");
        assert_eq!(speed, 2.5);
    } else {
        panic!("Expected Commands::Fx");
    }
}

#[test]
fn test_cli_subcommand_effects_parse() {
    let parsed = Cli::try_parse_from(["aula-f75-rs", "effects"]);
    assert!(parsed.is_ok());
    assert!(matches!(parsed.unwrap().command, Commands::Effects));
    assert_eq!(cmd_effects(), 0);
}

#[test]
fn test_cli_subcommand_profile_parse() {
    // Save with effect, color, brightness, keys
    let parsed_save = Cli::try_parse_from([
        "aula-f75-rs",
        "profile",
        "save",
        "gaming",
        "--effect",
        "rainbow",
        "--color",
        "FF0000",
        "--brightness",
        "0.75",
        "--key",
        "esc=00FF00",
        "--key",
        "space=0000FF",
    ]);
    assert!(parsed_save.is_ok());
    match parsed_save.unwrap().command {
        Commands::Profile {
            action:
                ProfileCommands::Save {
                    name,
                    effect,
                    color,
                    brightness,
                    keys,
                },
        } => {
            assert_eq!(name, "gaming");
            assert_eq!(effect, "rainbow");
            assert_eq!(color, Some("FF0000".to_string()));
            assert_eq!(brightness, 0.75);
            assert_eq!(keys, vec!["esc=00FF00", "space=0000FF"]);
        }
        _ => panic!("Expected Commands::Profile::Save"),
    }

    // Load
    let parsed_load = Cli::try_parse_from([
        "aula-f75-rs",
        "profile",
        "load",
        "gaming",
        "--seconds",
        "5.0",
    ]);
    assert!(parsed_load.is_ok());
    match parsed_load.unwrap().command {
        Commands::Profile {
            action: ProfileCommands::Load { name, seconds },
        } => {
            assert_eq!(name, "gaming");
            assert_eq!(seconds, Some(5.0));
        }
        _ => panic!("Expected Commands::Profile::Load"),
    }

    // List
    let parsed_list = Cli::try_parse_from(["aula-f75-rs", "profile", "list"]);
    assert!(parsed_list.is_ok());
    assert!(matches!(
        parsed_list.unwrap().command,
        Commands::Profile {
            action: ProfileCommands::List
        }
    ));

    // Delete
    let parsed_delete = Cli::try_parse_from(["aula-f75-rs", "profile", "delete", "gaming"]);
    assert!(parsed_delete.is_ok());
    match parsed_delete.unwrap().command {
        Commands::Profile {
            action: ProfileCommands::Delete { name },
        } => {
            assert_eq!(name, "gaming");
        }
        _ => panic!("Expected Commands::Profile::Delete"),
    }

    // Show
    let parsed_show = Cli::try_parse_from(["aula-f75-rs", "profile", "show", "gaming"]);
    assert!(parsed_show.is_ok());
    match parsed_show.unwrap().command {
        Commands::Profile {
            action: ProfileCommands::Show { name },
        } => {
            assert_eq!(name, "gaming");
        }
        _ => panic!("Expected Commands::Profile::Show"),
    }
}

#[test]
fn test_cli_subcommand_music_parse() {
    // Default music command
    let parsed_default = Cli::try_parse_from(["aula-f75-rs", "music"]);
    assert!(parsed_default.is_ok());
    match parsed_default.unwrap().command {
        Commands::Music {
            mode,
            color,
            gain,
            seconds,
        } => {
            assert_eq!(mode, "bars");
            assert_eq!(color, "00ffff");
            assert_eq!(gain, 1.0);
            assert_eq!(seconds, None);
        }
        _ => panic!("Expected Commands::Music"),
    }

    // Custom parameters
    let parsed_custom = Cli::try_parse_from([
        "aula-f75-rs",
        "music",
        "--mode",
        "pulse",
        "--color",
        "ff0055",
        "--gain",
        "2.5",
        "--seconds",
        "10.0",
    ]);
    assert!(parsed_custom.is_ok());
    match parsed_custom.unwrap().command {
        Commands::Music {
            mode,
            color,
            gain,
            seconds,
        } => {
            assert_eq!(mode, "pulse");
            assert_eq!(color, "ff0055");
            assert_eq!(gain, 2.5);
            assert_eq!(seconds, Some(10.0));
        }
        _ => panic!("Expected Commands::Music"),
    }
}

#[test]
fn test_cli_subcommand_battery_parse() {
    let parsed = Cli::try_parse_from(["aula-f75-rs", "battery"]);
    assert!(parsed.is_ok());
    assert!(matches!(parsed.unwrap().command, Commands::Battery));
}

#[test]
fn test_cli_subcommand_wireless_parse() {
    let parsed = Cli::try_parse_from(["aula-f75-rs", "wireless"]);
    assert!(parsed.is_ok());
    assert!(matches!(parsed.unwrap().command, Commands::Wireless));
}

#[test]
fn test_cli_subcommand_knob_parse() {
    let parsed = Cli::try_parse_from(["aula-f75-rs", "knob"]);
    assert!(parsed.is_ok());
    assert!(matches!(parsed.unwrap().command, Commands::Knob));
}
