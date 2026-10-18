# aula-f75

Native Linux configuration utility for the AULA F75 mechanical keyboard via USB HID (`hidraw`).

## Overview

The AULA F75 keyboard exposes USB HID interfaces (typical VID `258a`, PID `010c`, BY Tech Gaming) handled by the Linux kernel `hid-generic` driver. This project provides a standalone, Linux-native tool to detect, inspect, and configure keyboard hardware directly through `/dev/hidraw*` device nodes without proprietary vendor software.

- **Tranche 1**: Repository scaffolding, injectable device abstractions, and read-only device inspection.
- **Tranche 2**: Verified direct RGB lighting write paths (`solid`, `off`) using Linux stdlib `hidraw` ioctls on interface 1.
- **Tranche 3**: Standalone Rust CLI binary (`aula-f75-rs`) with zero external native dependencies beyond `libc`.
- **Tranche 4**: Native Rust GUI (`aula-f75-gui`) using `egui` and `eframe` with a non-blocking streaming worker thread.
- **Tranche 5**: Per-key LED map for the AULA F75 (80 mapped key slots) and `key`/`keys` CLI subcommands.
- **Tranche 6**: Host-rendered RGB lighting effects engine (`rainbow`, `wave`, `breathe`, `stars`, `solid`, `off`) and presentable desktop GUI panel.
- **Tranche 7**: Local lighting profiles (`save`, `load`, `list`, `delete`, `show`) for effects, colors, brightness, and per-key overrides with XDG flat file persistence and desktop GUI integration.
- **Tranche 8**: System-audio reactive lighting (music mode) with real-time Cooley-Tukey FFT, log-spaced frequency bands, AutoGain, and host-rendered Report 6 direct frames across CLI and GUI.
- **Tranche 9**: Packaging, desktop launcher (`aula-f75.desktop`), user systemd service (`aula-f75.service`), automated installer (`install.sh`), and retirement of the initial Python prototype (fully superseded by native Rust binaries).

## Non-Goals & Safety Rules

- **No Wine or Windows Emulation**: Operates directly on Linux kernel hidraw interfaces.
- **No Firmware Flashing**: Firmware flashing and bootloader updates are strictly out of scope.
- **Hard Safety Rule — Report 5 Prohibition**: Never send data on Report ID 5. On this MCU family, Report 5 is a write-only command mailbox where blind writes risk entering the ISP firmware-flash bootloader (soft-brick). Report 5 remains unimplemented and prohibited.
- **No Blind Protocol Writes**: Non-verified feature-report writes (key remapping, macros) remain disabled until payload structures are verified.

## Python Prototype Retirement

The original Python prototype (`aula_f75/`, `pyproject.toml`) developed in Tranches 1 and 2 served as the initial reverse-engineering harness and protocol verification testbed. With the completion of the standalone native Rust toolchain (`aula-f75-rs` CLI and `aula-f75-gui` desktop app), the Python prototype has been retired and removed from the repository. The Rust implementation is self-contained, high-performance, and has zero external runtime dependencies beyond `libc`.

## Installation

### Prerequisites

- **Rust toolchain** (1.70+ / Edition 2021) with `cargo` (via [rustup](https://rustup.rs)).
- **Linux kernel** with USB HID (`hidraw`) support.
- **Sudo privileges** for installing the udev rule (`99-aula-f75.rules`) into `/etc/udev/rules.d/`.
- **Audio utilities** (for `music` mode only): `pipewire-pulse` or `pulseaudio-utils` (provides `parec` and `pactl`).

### Automated Installation

Run the provided `install.sh` script from the repository root:

```bash
./install.sh
```

The installer performs the following actions:
1. Builds release binaries (`aula-f75-rs` and `aula-f75-gui`) via `cargo build --release`.
2. Installs both binaries to `${HOME}/.local/bin/`.
3. Installs `99-aula-f75.rules` to `/etc/udev/rules.d/` (prompting for sudo) and reloads udev rules via `udevadm`.
4. Installs `aula-f75.desktop` to `${XDG_DATA_HOME:-~/.local/share}/applications/`.
5. Installs `aula-f75.service` to `${XDG_CONFIG_HOME:-~/.config}/systemd/user/`.

### Manual Build & Installation

If preferred, build and install components manually:

```bash
# Build release binaries
cd rust
cargo build --release

# Copy binaries to ~/.local/bin
install -Dm755 target/release/aula-f75-rs ~/.local/bin/aula-f75-rs
install -Dm755 target/release/aula-f75-gui ~/.local/bin/aula-f75-gui

# Install udev rule (requires sudo)
sudo install -Dm644 ../99-aula-f75.rules /etc/udev/rules.d/99-aula-f75.rules
sudo udevadm control --reload-rules && sudo udevadm trigger

# Install desktop entry
install -Dm644 ../aula-f75.desktop ~/.local/share/applications/aula-f75.desktop

# Install user systemd service
install -Dm644 ../aula-f75.service ~/.config/systemd/user/aula-f75.service
systemctl --user daemon-reload
```

### Systemd User Service

To automatically apply default RGB lighting upon user login, enable the installed systemd user unit:

```bash
systemctl --user enable --now aula-f75.service
```

By default, the service executes `aula-f75-rs solid 303030` (dim warm hold). To apply a saved lighting profile on login instead (e.g. `default` or `work`), update `ExecStart` in `~/.config/systemd/user/aula-f75.service`:

```ini
ExecStart=%h/.local/bin/aula-f75-rs profile load work
```

### Uninstallation

To remove all installed components:

```bash
# Remove installed binaries
rm -f ~/.local/bin/aula-f75-rs ~/.local/bin/aula-f75-gui

# Remove desktop entry
rm -f ~/.local/share/applications/aula-f75.desktop
update-desktop-database ~/.local/share/applications/ 2>/dev/null || true

# Stop, disable, and remove user systemd service
systemctl --user disable --now aula-f75.service 2>/dev/null || true
rm -f ~/.config/systemd/user/aula-f75.service
systemctl --user daemon-reload 2>/dev/null || true

# Remove udev rule (requires sudo)
sudo rm -f /etc/udev/rules.d/99-aula-f75.rules
sudo udevadm control --reload-rules && sudo udevadm trigger
```

## CLI Usage (`aula-f75-rs`)

The standalone Linux CLI binary (`aula-f75-rs`) operates with zero runtime dependencies beyond `libc`.

### Enumerate HID devices

Detects system HID interfaces and flags matching AULA F75 hardware via sysfs:

```bash
aula-f75-rs list
```

### Inspect device details

Displays detailed hardware metadata for detected AULA hardware or a specific device path:

```bash
# Inspect first detected AULA F75 device
aula-f75-rs info

# Inspect a specific hidraw path
aula-f75-rs info --path /dev/hidraw0
```

### Query battery status

Reads safe sysfs power supply attributes and HID feature telemetry:

```bash
aula-f75-rs battery
```

### Detect 2.4GHz wireless dongle

Analyzes USB topology, interface numbers, and phys properties to distinguish direct cable vs 2.4GHz wireless dongle connections:

```bash
aula-f75-rs wireless
```

### Report multifunction knob mode

Inspects HID report descriptors and input event sources to determine observed rotary knob behavior:

```bash
aula-f75-rs knob
```

### Set solid RGB color

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

### Turn off RGB lighting

Turns off keyboard lighting by sending all-zero frames for ~2 seconds:

```bash
aula-f75-rs off
```

### Key Mapping & Per-Key RGB

```bash
# List all supported key names and slot numbers
aula-f75-rs keys

# Set RGB color for a specific named key (e.g. Esc in red)
aula-f75-rs key esc FF0000

# Light space key in green using alias for 5 seconds
aula-f75-rs key spacebar "#00FF00" --seconds 5
```

### Host-Rendered Lighting Effects

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

### Local Lighting Profiles

```bash
# Save a local profile with an effect and brightness
aula-f75-rs profile save rainbow-chill --effect rainbow --brightness 0.8

# Save a solid color profile
aula-f75-rs profile save cyan-work --effect solid --color 00FFFF --brightness 0.5

# Save a per-key gaming profile (WASD in red, space in green)
aula-f75-rs profile save fps-wasd --effect off --key w=FF0000 --key a=FF0000 --key s=FF0000 --key d=FF0000 --key space=00FF00

# List saved profiles
aula-f75-rs profile list

# Show profile configuration details
aula-f75-rs profile show fps-wasd

# Load and apply a profile (effect or per-key overrides)
aula-f75-rs profile load rainbow-chill

# Delete a profile
aula-f75-rs profile delete cyan-work
```

> [!NOTE]
> **Key Aliases & Case-Insensitivity**: Key names are case-insensitive and support common aliases (e.g. `esc`/`escape`, `space`/`spacebar`, `-`/`minus`, `[`/`lbracket`, `]`/`rbracket`, `\` / `backslash`, `;`/`semicolon`, `'`/`quote`, `,`/`comma`, `.`/`dot`, `/`/`slash`, `` ` ``/`backtick`, `up`/`uparrow`, `down`/`downarrow`, `left`/`leftarrow`, `right`/`rightarrow`, `del`/`delete`, `pgup`/`pageup`, `pgdn`/`pagedown`, `enter`/`return`).

> [!NOTE]
> **Keymap Layout Source**: Slot mappings (80 mapped physical keys) are derived from OpenRGB Sinowealth 010C `aula_f75_layout` cross-checked facts (live hardware verification pending).


## Rust GUI (`aula-f75-gui`)

Tranche 4 and Tranche 6 provide a native Linux desktop GUI application built with `eframe` and `egui` (0.31), featuring a dark sober two-panel layout, direct RGB lighting control, and continuous host-rendered effect streaming.

### Requirements

- Active desktop session (X11 or Wayland).
- Udev rule configured (`99-aula-f75.rules`) to allow unprivileged user access to `/dev/hidraw*`.

### Running

```bash
# If installed via install.sh
aula-f75-gui

# Or run from release build directory
./target/release/aula-f75-gui

# Or run via cargo
cd rust
cargo run --bin aula-f75-gui
```

### Features

- **Cohesive Modern Dark Theme**: Bespoke Nordic dark palette with single cyan accent (`#00b4d8`), 4px-based spacing multiples, 6px consistent corner radiuses, and readable typographic hierarchy.
- **Side Panel Hardware Status & Reference**: Left side panel displays multi-state connection badge (`Streaming Active`, `Connected (idle)`, `Connecting...`, `Disconnected`, `Error`), compact device facts grid (`AULA F75`, `258a:010c`, interface 1 node path), diagnostic cause/action microcopy, and a collapsible **Keyboard Reference** documenting onboard Fn shortcuts (Custom RGB profile requirement, Fn+Backspace toggle, Knob long-press modes, Fn+B battery indicator).
- **Segmented Effect Strip**: Sleek horizontal selectable card strip for instant switching between `Solid`, `Rainbow`, `Wave`, `Breathe`, `Stars`, and `Off`.
- **Live Color Preview Swatch & Sliders**: Color swatch panel showing the current scaled RGB output alongside interactive color picker, RGB sliders, hex input, brightness slider, and animation speed slider (Rainbow, Wave, Breathe).
- **Lighting Profiles Panel**: Integrated profiles management interface with profile dropdown selector, instant **Apply** button, **Delete** button, and **Save Profile** input capturing active parameters into local flat file presets without cloud or vendor dependencies.
- **Audio Reactive Lighting (Music Mode)**: Frequency-reactive lighting with mode selector (`Bars`, `Pulse`), base color picker, audio gain slider, real-time energy level meter, and dedicated controls.
- **Non-Blocking Streaming Worker**: Direct Report 6 frames are streamed at ~25 Hz (`Duration::from_millis(40)`) for fluid animations and ~1.5 Hz (`KEEPALIVE_HZ`) for solid keepalive on a dedicated background thread with transient error retry (`EPIPE`, `ETIMEDOUT`). The UI rendering thread is never blocked.
- **Professional Controls & Live Footer**: Clean labeled buttons (no emoji icons) and persistent status bar reporting active stream name, estimated tick rate, and hardware communication status.
- **Clean Lifecycle**: Window close signals worker termination and cleanly drops the device descriptor without blocking the UI thread.

### Troubleshooting

- **Unresponsive Window Dialog ("Application is not responding")**: Direct USB HID communication executes exclusively on a dedicated background worker thread with retry backoff and never blocks the UI rendering thread. If the desktop environment ever displays an "Application is not responding" dialog, close the window, unplug and replug the keyboard's USB cable, and reopen `aula-f75-gui`. If the issue persists, please report it along with your desktop environment details (GNOME/KDE, Wayland/X11) and kernel version.

> [!NOTE]
> **CUSTOM Mode Requirement**: The physical keyboard must be toggled into its onboard "Custom" / user static RGB mode (via onboard Fn shortcuts) for software-driven colors to display.

## Local Lighting Profiles (Tranche 7)

Tranche 7 introduces fully offline, local lighting profiles for saving, loading, listing, and applying lighting configurations across both CLI and GUI.

### Storage Location & Environment Overrides

Profiles are stored as human-readable, flat `.toml` key-value files in the standard XDG user configuration path:

- Default location: `$XDG_CONFIG_HOME/aula-f75/profiles/<name>.toml`
- Fallback: `~/.config/aula-f75/profiles/<name>.toml`
- Environment override: `AULA_F75_CONFIG=/custom/dir` points directly to the base config directory (creating a `profiles/` subdirectory as needed).

### File Schema

Each profile is stored as a flat `key = value` text file parsed strictly with file-and-line error diagnostics:

```toml
# AULA F75 Lighting Profile
name = fps-wasd
effect = off
brightness = 0.80
key.w = FF0000
key.a = FF0000
key.s = FF0000
key.d = FF0000
key.space = 00FF00
```

Supported keys:
- `name`: Profile identifier string.
- `effect`: Host effect name (`solid`, `rainbow`, `wave`, `breathe`, `stars`, `off`, `custom`).
- `color`: 6-digit hex color triplet (`RRGGBB`) required for `solid` and `breathe`.
- `brightness`: Floating point multiplier clamped to `0.0`..=`1.0`.
- `key.<name>`: Per-key RGB color override where `<name>` is any valid key or alias recognized by the keymap (e.g. `key.esc = FF0000`).

### CLI Commands

```bash
# Save profile
aula-f75-rs profile save NAME --effect E [--color RRGGBB] [--brightness B] [--key NAME=COLOR ...]

# Load and apply profile
aula-f75-rs profile load NAME [--seconds N]

# List profiles
aula-f75-rs profile list

# Show profile details
aula-f75-rs profile show NAME

# Delete profile
aula-f75-rs profile delete NAME
```

## System Audio Reactive Lighting (Tranche 8)

Tranche 8 provides host-driven, audio-reactive RGB lighting (`music` mode) powered by the Linux audio stack without adding external native C/C++ audio library dependencies.

### Architecture & Design

- **Audio Capture**: Spawns `parec` on the default output sink monitor (`pactl get-default-sink` + `.monitor`) using 44.1 kHz 16-bit little-endian stereo PCM (`s16le`).
- **Signal Processing**: Hand-rolled radix-2 Cooley-Tukey FFT implemented purely in Rust standard library with inline Hann windowing and precomputed twiddle factor tables.
- **Spectrum & AutoGain**: Analyzes log-spaced frequency bands (40 Hz to 12 kHz) with an exponential decaying peak tracker (`AutoGain`) to automatically adjust sensitivity to audio volume.
- **Hardware Rendering**: Maps normalized frequency band energies to Report 6 direct RGB frames streamed at ~20 fps.

### Requirements

- A Linux audio server with `pulseaudio-utils` or `pipewire-pulse` providing `parec` and `pactl`.
- Keyboard toggled into its onboard **Custom / Static RGB** mode (via onboard Fn shortcuts).

### CLI Usage

```bash
# Run bars mode (frequency spectrum mapped across physical keys) with default cyan color
aula-f75-rs music

# Run pulse mode (whole-board beat pulse) with magenta color and 2.0x gain for 10 seconds
aula-f75-rs music --mode pulse --color ff00aa --gain 2.0 --seconds 10
```

### GUI Integration

The native desktop GUI (`aula-f75-gui`) includes a dedicated **Audio Reactive Lighting (Music Mode)** panel:
- **Mode Dropdown**: Toggle between `Bars` (column spectrum) and `Pulse` (whole-board volume pulse).
- **Color Picker**: Dynamic color selection for the reactive base lighting.
- **Audio Gain Slider**: Real-time gain multiplier adjustment (`0.1x` to `5.0x`).
- **Level Meter**: Real-time visual audio energy level `ProgressBar` streamed via non-blocking channels from the worker thread.
- **Non-blocking Worker**: Audio capture and FFT analysis execute entirely on the background worker thread, ensuring the UI stays responsive.

### Verification Gate

All unit and integration tests are strictly hardware-free and testable without physical devices or active desktop display sessions:

```bash
cd rust
cargo test
```

