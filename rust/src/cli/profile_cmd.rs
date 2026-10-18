//! Profile command execution handler for AULA F75.

use crate::cli::parse_color_hex;
use crate::cli::profile;
use crate::cli::{run_effect_lighting, run_keys_lighting, ProfileCommands};

fn parse_profile_color(color: Option<&String>) -> Result<Option<(u8, u8, u8)>, ()> {
    match color {
        Some(c) => match parse_color_hex(c) {
            Ok(rgb) => Ok(Some(rgb)),
            Err(e) => {
                eprintln!("Error parsing --color: {}", e);
                Err(())
            }
        },
        None => Ok(None),
    }
}

fn parse_key_override(key_arg: &str) -> Result<(String, (u8, u8, u8)), ()> {
    let (k, hex) = match key_arg.split_once('=') {
        Some(pair) => pair,
        None => {
            eprintln!(
                "Error: Invalid --key format {:?}, expected NAME=COLOR (e.g. esc=FF0000)",
                key_arg
            );
            return Err(());
        }
    };
    match parse_color_hex(hex) {
        Ok(c) => Ok((k.trim().to_string(), c)),
        Err(e) => {
            eprintln!("Error parsing color in --key {:?}: {}", key_arg, e);
            Err(())
        }
    }
}

fn parse_profile_keys(keys: &[String]) -> Result<Vec<(String, (u8, u8, u8))>, ()> {
    let mut parsed = Vec::with_capacity(keys.len());
    for key_arg in keys {
        parsed.push(parse_key_override(key_arg)?);
    }
    Ok(parsed)
}

fn handle_save_profile(
    name: &str,
    effect: &str,
    color: Option<&String>,
    brightness: f32,
    keys: &[String],
) -> i32 {
    let parsed_color = match parse_profile_color(color) {
        Ok(c) => c,
        Err(()) => return 1,
    };
    let parsed_keys = match parse_profile_keys(keys) {
        Ok(k) => k,
        Err(()) => return 1,
    };
    let prof = profile::Profile::new(
        name.to_string(),
        effect.to_string(),
        parsed_color,
        brightness,
        parsed_keys,
    );
    match profile::save_profile(&prof, None) {
        Ok(path) => {
            println!("Saved profile '{}' to {}", prof.name, path.display());
            0
        }
        Err(e) => {
            eprintln!("Error saving profile: {}", e);
            1
        }
    }
}

fn run_applied_profile_params(
    prof_name: &str,
    params: &profile::AppliedProfileParams,
) -> Result<(), String> {
    match params {
        profile::AppliedProfileParams::Effect {
            effect,
            brightness,
            seconds,
        } => {
            println!(
                "Applying profile '{}' (effect: {:?}, brightness: {:.2})...",
                prof_name, effect, brightness
            );
            let exit = run_effect_lighting(effect, *brightness, *seconds);
            if exit == 0 {
                Ok(())
            } else {
                Err(format!("Device communication exited with code {}", exit))
            }
        }
        profile::AppliedProfileParams::Keys { pairs, seconds } => {
            println!(
                "Applying profile '{}' ({} per-key overrides)...",
                prof_name,
                pairs.len()
            );
            let borrowed: Vec<(&str, (u8, u8, u8))> =
                pairs.iter().map(|(k, c)| (k.as_str(), *c)).collect();
            let exit = run_keys_lighting(&borrowed, *seconds);
            if exit == 0 {
                Ok(())
            } else {
                Err(format!("Device communication exited with code {}", exit))
            }
        }
    }
}

fn handle_load_profile(name: &str, seconds: Option<f64>) -> i32 {
    let prof = match profile::load_profile(name, None) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error loading profile: {}", e);
            return 1;
        }
    };

    let runner_res = prof.apply_with_runner(seconds, |params| {
        run_applied_profile_params(&prof.name, params)
    });

    match runner_res {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("Error applying profile: {}", e);
            1
        }
    }
}

fn handle_list_profiles() -> i32 {
    match profile::list_profiles(None) {
        Ok(list) => {
            if list.is_empty() {
                println!("No profiles found.");
            } else {
                let default_prof = profile::get_default_profile(None);
                println!("Saved profiles ({}):", list.len());
                for name in list {
                    if default_prof.as_deref() == Some(&name) {
                        println!("  {} (default)", name);
                    } else {
                        println!("  {}", name);
                    }
                }
            }
            0
        }
        Err(e) => {
            eprintln!("Error listing profiles: {}", e);
            1
        }
    }
}

fn handle_delete_profile(name: &str) -> i32 {
    match profile::delete_profile(name, None) {
        Ok(()) => {
            println!("Profile '{}' deleted.", name);
            0
        }
        Err(e) => {
            eprintln!("Error deleting profile: {}", e);
            1
        }
    }
}

fn print_profile_keys(keys: &[(String, (u8, u8, u8))]) {
    if !keys.is_empty() {
        println!("  Key Overrides ({}):", keys.len());
        for (k, c) in keys {
            println!("    {:<12} = #{:02X}{:02X}{:02X}", k, c.0, c.1, c.2);
        }
    }
}

fn handle_show_profile(name: &str) -> i32 {
    match profile::load_profile(name, None) {
        Ok(prof) => {
            let is_default = profile::get_default_profile(None).as_deref() == Some(&prof.name);
            if is_default {
                println!("Profile: {} (default)", prof.name);
            } else {
                println!("Profile: {}", prof.name);
            }
            println!("  Effect: {}", prof.effect);
            if let Some(c) = prof.color {
                println!("  Color: #{:02X}{:02X}{:02X}", c.0, c.1, c.2);
            }
            println!("  Brightness: {:.2}", prof.brightness);
            print_profile_keys(&prof.keys);
            0
        }
        Err(e) => {
            eprintln!("Error showing profile: {}", e);
            1
        }
    }
}

fn handle_set_default_profile(name: &str) -> i32 {
    match profile::set_default_profile(name, None) {
        Ok(()) => {
            println!("Profile '{}' set as default.", name.trim());
            0
        }
        Err(e) => {
            eprintln!("Error setting default profile: {}", e);
            1
        }
    }
}

fn handle_clear_default_profile() -> i32 {
    match profile::clear_default_profile(None) {
        Ok(()) => {
            println!("Default profile cleared.");
            0
        }
        Err(e) => {
            eprintln!("Error clearing default profile: {}", e);
            1
        }
    }
}

/// Handler for profile CLI subcommands.
pub fn cmd_profile(action: &ProfileCommands) -> i32 {
    match action {
        ProfileCommands::Save {
            name,
            effect,
            color,
            brightness,
            keys,
        } => handle_save_profile(name, effect, color.as_ref(), *brightness, keys),
        ProfileCommands::Load { name, seconds } => handle_load_profile(name, *seconds),
        ProfileCommands::List => handle_list_profiles(),
        ProfileCommands::Delete { name } => handle_delete_profile(name),
        ProfileCommands::Show { name } => handle_show_profile(name),
        ProfileCommands::SetDefault { name } => handle_set_default_profile(name),
        ProfileCommands::ClearDefault => handle_clear_default_profile(),
    }
}
