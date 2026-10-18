//! Profile command execution handler for AULA F75.

use crate::cli::parse_color_hex;
use crate::cli::profile;
use crate::cli::{run_effect_lighting, run_keys_lighting, ProfileCommands};

/// Handler for profile CLI subcommands.
pub fn cmd_profile(action: &ProfileCommands) -> i32 {
    match action {
        ProfileCommands::Save {
            name,
            effect,
            color,
            brightness,
            keys,
        } => {
            let parsed_color = match color {
                Some(c) => match parse_color_hex(c) {
                    Ok(rgb) => Some(rgb),
                    Err(e) => {
                        eprintln!("Error parsing --color: {}", e);
                        return 1;
                    }
                },
                None => None,
            };

            let mut parsed_keys = Vec::new();
            for key_arg in keys {
                let (k, hex) = match key_arg.split_once('=') {
                    Some(pair) => pair,
                    None => {
                        eprintln!(
                            "Error: Invalid --key format {:?}, expected NAME=COLOR (e.g. esc=FF0000)",
                            key_arg
                        );
                        return 1;
                    }
                };
                let rgb = match parse_color_hex(hex) {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("Error parsing color in --key {:?}: {}", key_arg, e);
                        return 1;
                    }
                };
                parsed_keys.push((k.trim().to_string(), rgb));
            }

            let prof = profile::Profile::new(
                name.clone(),
                effect.clone(),
                parsed_color,
                *brightness,
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
        ProfileCommands::Load { name, seconds } => {
            let prof = match profile::load_profile(name, None) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("Error loading profile: {}", e);
                    return 1;
                }
            };

            let runner_res = prof.apply_with_runner(*seconds, |params| match params {
                profile::AppliedProfileParams::Effect {
                    effect,
                    brightness,
                    seconds,
                } => {
                    println!(
                        "Applying profile '{}' (effect: {:?}, brightness: {:.2})...",
                        prof.name, effect, brightness
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
                        prof.name,
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
            });

            match runner_res {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("Error applying profile: {}", e);
                    1
                }
            }
        }
        ProfileCommands::List => {
            match profile::list_profiles(None) {
                Ok(list) => {
                    if list.is_empty() {
                        println!("No profiles found.");
                    } else {
                        println!("Saved profiles ({}):", list.len());
                        for name in list {
                            println!("  {}", name);
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
        ProfileCommands::Delete { name } => {
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
        ProfileCommands::Show { name } => {
            match profile::load_profile(name, None) {
                Ok(prof) => {
                    println!("Profile: {}", prof.name);
                    println!("  Effect: {}", prof.effect);
                    if let Some(c) = prof.color {
                        println!("  Color: #{:02X}{:02X}{:02X}", c.0, c.1, c.2);
                    }
                    println!("  Brightness: {:.2}", prof.brightness);
                    if !prof.keys.is_empty() {
                        println!("  Key Overrides ({}):", prof.keys.len());
                        for (k, c) in &prof.keys {
                            println!("    {:<12} = #{:02X}{:02X}{:02X}", k, c.0, c.1, c.2);
                        }
                    }
                    0
                }
                Err(e) => {
                    eprintln!("Error showing profile: {}", e);
                    1
                }
            }
        }
    }
}
