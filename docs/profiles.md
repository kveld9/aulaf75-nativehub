# Local Lighting Profiles

Lighting profiles provide fully offline, file-based persistence for saving, loading, listing, and applying lighting configurations across both CLI and GUI without vendor software or cloud dependencies.

## Storage Location & Environment Overrides

Profiles are stored as human-readable, flat `.toml` key-value files in the standard XDG user configuration path:

- **Default Location**: `$XDG_CONFIG_HOME/aula-f75/profiles/<name>.toml`
- **Fallback Location**: `~/.config/aula-f75/profiles/<name>.toml`
- **Environment Override**: Setting `AULA_F75_CONFIG=/custom/dir` points directly to the base configuration directory (creating a `profiles/` subdirectory as needed).

Profiles are preserved upon application upgrade or uninstallation.

---

## File Schema

Each profile is stored as a flat `key = value` text file parsed strictly with file-and-line error diagnostics:

```toml
# AULA F75 Lighting Profile
name = fps-wasd
effect = off
brightness = 0.80
speed = 1.00
key.w = FF0000
key.a = FF0000
key.s = FF0000
key.d = FF0000
key.space = 00FF00
```

### Supported Keys

| Key | Type | Description |
| :--- | :--- | :--- |
| `name` | string | Profile identifier string (must match `<name>.toml`) |
| `effect` | string | Effect name (`solid`, `rainbow`, `wave`, `breathe`, `stars`, `off`, `custom`) |
| `color` | string | 6-digit hex color triplet (`RRGGBB`), required for `solid` and `breathe` |
| `brightness` | float | Brightness multiplier clamped to `0.0`..=`1.0` |
| `speed` | float | Animation speed factor clamped to `0.25`..=`3.0` (default `1.0`) |
| `key.<name>` | string | Per-key RGB color override where `<name>` is any valid key or alias |

---

## CLI Commands

### Save a Profile
```bash
# Save an effect profile with brightness and speed
aula-f75-rs profile save rainbow-chill --effect rainbow --brightness 0.8

# Save a solid color profile
aula-f75-rs profile save cyan-work --effect solid --color 00FFFF --brightness 0.5

# Save a per-key gaming profile (WASD in red, space in green)
aula-f75-rs profile save fps-wasd --effect off --key w=FF0000 --key a=FF0000 --key s=FF0000 --key d=FF0000 --key space=00FF00
```

### Load and Apply a Profile
```bash
# Apply a saved profile indefinitely (until Ctrl+C)
aula-f75-rs profile load rainbow-chill

# Apply a profile for 10 seconds
aula-f75-rs profile load fps-wasd --seconds 10
```

### List Profiles
```bash
aula-f75-rs profile list
```

### Show Profile Details
```bash
aula-f75-rs profile show fps-wasd
```

### Delete a Profile
```bash
aula-f75-rs profile delete cyan-work
```

---

## GUI Integration

The native desktop GUI (`aula-f75-gui`) includes an integrated **Lighting Profiles** panel allowing you to:
- Select and apply any profile from a dropdown list with a single click.
- Save the currently active effect, color, brightness, and speed under a new profile name.
- Delete unwanted profiles from disk immediately.

---

## See Also

- [CLI Command Reference](cli-reference.md)
- [Desktop GUI Documentation](gui.md)
- [Music Mode Reactive Lighting](music.md)
- [USB HID Protocol Specification](PROTOCOL.md)
- [Project Overview & Quickstart](../README.md)
