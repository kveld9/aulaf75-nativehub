# AULA F75 CLI Command Reference (`aula-f75-rs`)

The standalone Linux CLI binary (`aula-f75-rs`) operates with zero runtime dependencies beyond `libc` and the Linux kernel `hidraw` interface.

## Command Overview

The CLI provides 13 subcommands across hardware inspection, lighting control, presets, and audio reactivity:

| Subcommand | Description |
| :--- | :--- |
| `list` | Enumerate all detected USB HID devices and flag known AULA hardware |
| `info` | Inspect device metadata, USB topology, and interface facts |
| `battery` | Query battery charge percentage and charging state via sysfs power supply |
| `wireless` | Detect direct USB cable vs 2.4GHz wireless dongle connection topology |
| `knob` | Report observed multifunction rotary knob behavior from report descriptors |
| `solid` | Set solid whole-board RGB color with automatic keepalive |
| `off` | Turn off RGB lighting by transmitting all-zero direct frames |
| `key` | Set per-key RGB color for a specific named physical key or alias |
| `keys` | List all 80 supported physical key names and frame slot numbers |
| `fx` | Stream dynamic host-rendered effects with color, brightness, and speed control |
| `effects` | List all available host-rendered RGB lighting effects |
| `profile` | Manage local lighting profiles (`save`, `load`, `list`, `delete`, `show`) |
| `music` | Stream system audio-reactive frequency spectrum and beat pulses |

---

## Device Discovery & Inspection

### Enumerate HID devices (`list`)

Detects system HID interfaces and flags matching AULA F75 hardware via sysfs:

```bash
aula-f75-rs list
```

### Inspect device details (`info`)

Displays detailed hardware metadata for detected AULA hardware or a specific device path:

```bash
# Inspect first detected AULA F75 device
aula-f75-rs info

# Inspect a specific hidraw path
aula-f75-rs info --path /dev/hidraw0

# Filter by Vendor ID and Product ID
aula-f75-rs info --vid 258a --pid 010c
```

### Query battery status (`battery`)

Reads safe sysfs power supply attributes and HID feature telemetry:

```bash
aula-f75-rs battery
```

### Detect 2.4GHz wireless dongle (`wireless`)

Analyzes USB topology, interface numbers, and phys properties to distinguish direct cable vs 2.4GHz wireless dongle connections:

```bash
aula-f75-rs wireless
```

### Report multifunction knob mode (`knob`)

Inspects HID report descriptors and input event sources to determine observed rotary knob behavior:

```bash
aula-f75-rs knob
```

---

## Direct RGB Lighting Control

### Set solid RGB color (`solid`)

Sets whole-board static color using a 6-digit hex string (with or without `#`):

```bash
# Set solid red indefinitely (until Ctrl+C)
aula-f75-rs solid FF0000

# Set solid cyan for 5 seconds
aula-f75-rs solid "#00FFFF" --seconds 5
```

> [!NOTE]
> **CUSTOM Mode Requirement**: The physical keyboard must be toggled into its onboard "Custom" / user RGB profile (via onboard Fn shortcuts) to display host software-driven lighting.

> [!NOTE]
> **Keepalive Loop**: The keyboard microcontroller latches colors immediately, but reverts to its onboard lighting effect after a few idle seconds (~3-5 s). The `solid` command continuously transmits frames at ~1.5 Hz (`KEEPALIVE_HZ`) until `--seconds` expires or `Ctrl+C` is pressed.

### Turn off RGB lighting (`off`)

Turns off keyboard lighting by sending all-zero frames for ~2 seconds:

```bash
aula-f75-rs off
```

### Key Mapping & Per-Key RGB (`key`, `keys`)

```bash
# List all supported key names and slot numbers
aula-f75-rs keys

# Set RGB color for a specific named key (e.g. Esc in red)
aula-f75-rs key esc FF0000

# Light space key in green using alias for 5 seconds
aula-f75-rs key spacebar "#00FF00" --seconds 5
```

> [!NOTE]
> **Key Aliases & Case-Insensitivity**: Key names are case-insensitive and support common aliases (e.g. `esc`/`escape`, `space`/`spacebar`, `-`/`minus`, `[`/`lbracket`, `]`/`rbracket`, `\` / `backslash`, `;`/`semicolon`, `'`/`quote`, `,`/`comma`, `.`/`dot`, `/`/`slash`, `` ` ``/`backtick`, `up`/`uparrow`, `down`/`downarrow`, `left`/`leftarrow`, `right`/`rightarrow`, `del`/`delete`, `pgup`/`pageup`, `pgdn`/`pagedown`, `enter`/`return`).

> [!NOTE]
> **Keymap Layout Source**: Slot mappings (80 mapped physical keys) are derived from OpenRGB Sinowealth 010C `aula_f75_layout` cross-checked facts.

---

## Host-Rendered Lighting Effects (`fx`, `effects`)

```bash
# List all available host-rendered lighting effects
aula-f75-rs effects

# Stream dynamic Rainbow effect indefinitely
aula-f75-rs fx rainbow

# Stream Breathe pulsing effect in magenta
aula-f75-rs fx breathe --color ff00ff

# Stream twinkling Stars effect at 70% brightness
aula-f75-rs fx stars --brightness 0.7

# Stream Wave effect for 10 seconds
aula-f75-rs fx wave --seconds 10

# Stream dynamic Rainbow effect at 2x animation speed (multiplier 0.25 to 3.0)
aula-f75-rs fx rainbow --speed 2.0
```

Available effects:
- `solid`: Static single color across all keys (requires `--color`).
- `rainbow`: Smooth cyclic whole-board spectrum shift.
- `wave`: Horizontal chromatic wave propagating across keys.
- `breathe`: Pulsing illumination fading between black and color (requires `--color`).
- `stars`: Deterministic twinkling starry night across keys.
- `off`: Turn off RGB lighting.

---

## Local Lighting Profiles (`profile`)

Manage persistent local configurations without cloud or vendor software:

```bash
# Save profile
aula-f75-rs profile save NAME --effect E [--color RRGGBB] [--brightness B] [--key NAME=COLOR ...]

# Load and apply profile
aula-f75-rs profile load NAME [--seconds N]

# List saved profiles
aula-f75-rs profile list

# Show profile configuration details
aula-f75-rs profile show NAME

# Delete a saved profile
aula-f75-rs profile delete NAME
```

See [profiles.md](profiles.md) for full schema and configuration documentation.

---

## System Audio Reactive Lighting (`music`)

Stream real-time frequency-reactive lighting powered by PulseAudio/PipeWire:

```bash
# Run bars mode (frequency spectrum mapped across physical keys) with default cyan color
aula-f75-rs music

# Run pulse mode (whole-board beat pulse) with magenta color and 2.0x gain for 10 seconds
aula-f75-rs music --mode pulse --color ff00aa --gain 2.0 --seconds 10
```

See [music.md](music.md) for architecture, FFT processing, and configuration details.

---

## See Also

- [Desktop GUI Documentation](gui.md)
- [Local Lighting Profiles](profiles.md)
- [Music Mode Reactive Lighting](music.md)
- [USB HID Protocol Specification](PROTOCOL.md)
- [Project Overview & Quickstart](../README.md)
