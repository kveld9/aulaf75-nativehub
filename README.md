# aula-f75

Native Linux configuration utility and direct RGB lighting driver for the AULA F75 mechanical keyboard via USB HID (`hidraw`).

## Overview

The AULA F75 keyboard exposes USB HID interfaces (VID `258a`, PID `010c`, BY Tech Gaming) handled by the Linux kernel `hid-generic` driver. This project provides standalone, Linux-native tools to detect, inspect, configure, and stream RGB lighting directly through `/dev/hidraw*` device nodes without proprietary vendor software, Wine, or Proton wrappers.

- **Standalone CLI Binary (`aula-f75-rs`)**: 13 subcommands for hardware enumeration, direct static colors, per-key RGB mapping, host-rendered effects, preset profiles, battery querying, wireless dongle detection, knob mode inspection, and audio-reactive music mode. Zero runtime dependencies beyond `libc`.
- **Native Desktop GUI (`aula-f75-gui`)**: Modern dark desktop interface built with `egui` and `eframe` (0.31). Features non-blocking worker threads, segmented effect strip, live color swatch, profiles management, and real-time audio spectrum visualizer.
- **Direct RGB Engine**: Single 520-byte Report 6 direct feature report driving 80 physical key LEDs at ~25 Hz with automatic keepalive loops.

---

## Safety Rules & Non-Goals

- **Hard Safety Rule — Report 5 Prohibition**: Never send data on Report ID 5. On this MCU family (Sinowealth / BY Tech), Report 5 is a write-only command mailbox where blind writes risk entering the ISP firmware-flash bootloader and causing a soft brick. Report 5 remains strictly unmapped and forbidden in both ioctl directions.
- **No Wine or Windows Emulation**: Operates directly on Linux kernel `/dev/hidraw*` interfaces.
- **No Firmware Flashing**: Firmware flashing and bootloader updates are strictly out of scope.
- **Hardware-Free Unit Tests**: All device discovery and ioctl write paths support dependency injection for offline testing without physical devices.

---

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

The installer builds release binaries, installs them to `~/.local/bin/`, installs the udev rule to `/etc/udev/rules.d/`, registers the desktop launcher, and sets up the systemd user service.

### Manual Build & Installation

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
```

### Systemd User Service

To automatically apply default RGB lighting upon user login:

```bash
systemctl --user enable --now aula-f75.service
```

### Uninstallation

```bash
rm -f ~/.local/bin/aula-f75-rs ~/.local/bin/aula-f75-gui
rm -f ~/.local/share/applications/aula-f75.desktop
systemctl --user disable --now aula-f75.service 2>/dev/null || true
rm -f ~/.config/systemd/user/aula-f75.service
sudo rm -f /etc/udev/rules.d/99-aula-f75.rules
sudo udevadm control --reload-rules && sudo udevadm trigger
```

---

## Quickstart

> [!NOTE]
> **CUSTOM Mode Requirement**: The physical keyboard must be toggled into its onboard "Custom" / user RGB profile (via onboard Fn shortcuts) to display host software-driven lighting.

### CLI Quickstart

```bash
# Detect connected keyboard interfaces
aula-f75-rs list

# Set solid cyan color indefinitely
aula-f75-rs solid 00FFFF

# Stream dynamic Rainbow effect at 1.5x speed
aula-f75-rs fx rainbow --speed 1.5

# Light WASD keys in red
aula-f75-rs key w FF0000

# Launch audio-reactive spectrum lighting
aula-f75-rs music --mode bars
```

### GUI Quickstart

```bash
# Launch desktop GUI
aula-f75-gui
```

---

## Documentation Index

Detailed documentation for each component is available in the [`docs/`](docs/) directory:

- 📖 **[CLI Command Reference](docs/cli-reference.md)**: Full syntax and documentation for all 13 CLI subcommands, key mapping aliases, and parameters.
- 🖥️ **[Desktop GUI Application](docs/gui.md)**: GUI architecture, features, theme tokens, troubleshooting, and streaming worker model.
- 💾 **[Local Lighting Profiles](docs/profiles.md)**: XDG file storage specification, flat TOML schema, CLI commands, and GUI preset management.
- 🎵 **[System Audio Reactive Lighting](docs/music.md)**: Real-time FFT signal processing, log-spaced frequency bands, AutoGain, and music mode configuration.
- 🔌 **[USB HID Protocol Specification](docs/PROTOCOL.md)**: Verified Report 6 direct frame structure, timing constraints, keepalive protocol, and hardware reverse-engineering findings.

---

## Verification Gate

All unit and integration tests are strictly hardware-free and testable offline without physical hardware or desktop display sessions:

```bash
cd rust
cargo test
```

---

## License

This project is dual-licensed under either:

- [MIT License](LICENSE)
- [Apache License, Version 2.0](LICENSE)

at your option.
