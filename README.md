# aula-f75

Native Linux configuration utility for the AULA F75 mechanical keyboard via USB HID (`hidraw`).

## Overview

The AULA F75 keyboard exposes USB HID interfaces (typical VID `258a`, PID `010c`, BY Tech Gaming) handled by the Linux kernel `hid-generic` driver. This project provides a standalone, Linux-native tool to detect, inspect, and configure keyboard hardware directly through `/dev/hidraw*` device nodes without proprietary vendor software.

- **Tranche 1**: Repository scaffolding, injectable device abstractions, and read-only device inspection.
- **Tranche 2**: Verified direct RGB lighting write paths (`solid`, `off`) using Linux stdlib `hidraw` ioctls on interface 1.
- **Tranche 3**: Standalone Rust CLI binary (`aula-f75-rs`) with zero external native dependencies beyond `libc`.
- **Tranche 4**: Native Rust GUI (`aula-f75-gui`) using `egui` and `eframe` with a non-blocking streaming worker thread.
- **Tranche 5**: Per-key LED map for the AULA F75 (80 mapped key slots) and `key`/`keys` CLI subcommands.

## Non-Goals & Safety Rules

- **No Wine or Windows Emulation**: Operates directly on Linux kernel hidraw interfaces.
- **No Firmware Flashing**: Firmware flashing and bootloader updates are strictly out of scope.
- **Hard Safety Rule — Report 5 Prohibition**: Never send data on Report ID 5. On this MCU family, Report 5 is a write-only command mailbox where blind writes risk entering the ISP firmware-flash bootloader (soft-brick). Report 5 remains unimplemented and prohibited.
- **No Blind Protocol Writes**: Non-verified feature-report writes (key remapping, macros) remain disabled until payload structures are verified.

## Udev Rule Setup

By default, `/dev/hidraw*` device nodes require elevated privileges. Install the provided udev rule to grant unprivileged user access via `uaccess` and mode `0666`:

```bash
sudo cp 99-aula-f75.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules && sudo udevadm trigger
```

## Installation

```bash
pip install .
```

For development and testing:

```bash
pip install .[test]
pytest -q
```

All test suites are strictly hardware-free and execute against mockable backends.

## CLI Usage

### Enumerate HID devices

Detects system HID interfaces and flags matching AULA F75 hardware:

```bash
aula-f75 list
```

### Inspect device details

Displays detailed hardware metadata for detected AULA hardware or a specific device path:

```bash
# Inspect first detected AULA F75 device
aula-f75 info

# Inspect a specific hidraw path
aula-f75 info --path /dev/hidraw0
```

### Set solid RGB color

Sets whole-board static color using a 6-digit hex string (with or without `#`):

```bash
# Set solid red indefinitely (until Ctrl+C)
aula-f75 solid ff0000

# Set solid cyan with leading hash for 5 seconds
aula-f75 solid "#00ffff" --seconds 5
```

> [!NOTE]
> **CUSTOM Mode Requirement**: The physical keyboard must be toggled into its onboard "Custom" / user RGB profile (via onboard Fn shortcuts) to display host software-driven lighting.

> [!NOTE]
> **Keepalive Loop**: The keyboard microcontroller latches colors immediately, but reverts to its onboard lighting effect after a few idle seconds (~3-5 s). The `solid` command continuously transmits frames at ~1.5 Hz (`KEEPALIVE_HZ`) until `--seconds` expires or `Ctrl+C` is pressed.

### Turn off RGB lighting

Turns off keyboard lighting by sending all-zero frames for ~2 seconds:

```bash
aula-f75 off
```

## Rust CLI (`aula-f75-rs`)

Tranche 3 provides a standalone Linux CLI binary in Rust with zero runtime dependencies beyond `libc`.

### Building

```bash
cd rust
cargo build --release
```

> [!NOTE]
> The initial build requires network connectivity to fetch dependencies (`clap` and `libc`) from crates.io. Subsequent builds and the test gate are offline-safe.

### Running

```bash
# Set solid red indefinitely (until Ctrl+C)
./target/release/aula-f75-rs solid FF0000

# Set solid cyan for 5 seconds
./target/release/aula-f75-rs solid "#00FFFF" --seconds 5

# Turn off lighting (solid 000000 for ~2s)
./target/release/aula-f75-rs off

# Enumerate HID devices via sysfs
./target/release/aula-f75-rs list

# Inspect device metadata
./target/release/aula-f75-rs info

# List all supported key names and slot numbers
./target/release/aula-f75-rs keys

# Set RGB color for a specific named key (e.g. Esc in red)
./target/release/aula-f75-rs key esc FF0000

# Light space key in green using alias for 5 seconds
./target/release/aula-f75-rs key spacebar "#00FF00" --seconds 5
```

> [!NOTE]
> **Key Aliases & Case-Insensitivity**: Key names are case-insensitive and support common aliases (e.g. `esc`/`escape`, `space`/`spacebar`, `-`/`minus`, `[`/`lbracket`, `]`/`rbracket`, `\` / `backslash`, `;`/`semicolon`, `'`/`quote`, `,`/`comma`, `.`/`dot`, `/`/`slash`, `` ` ``/`backtick`, `up`/`uparrow`, `down`/`downarrow`, `left`/`leftarrow`, `right`/`rightarrow`, `del`/`delete`, `pgup`/`pageup`, `pgdn`/`pagedown`, `enter`/`return`).

> [!NOTE]
> **Keymap Layout Source**: Slot mappings (80 mapped physical keys) are derived from OpenRGB Sinowealth 010C `aula_f75_layout` cross-checked facts (live hardware verification pending).


## Rust GUI (`aula-f75-gui`)

Tranche 4 provides a native Linux desktop GUI application built with `eframe` and `egui`, reusing the verified direct RGB protocol and device resolution logic from the core crate.

### Requirements

- Active desktop session (X11 or Wayland).
- Udev rule configured (`99-aula-f75.rules`) to allow unprivileged user access to `/dev/hidraw*`.

### Running

```bash
cd rust
cargo run --bin aula-f75-gui
```

Alternatively, run the compiled release binary:

```bash
./target/release/aula-f75-gui
```

### Features

- **Reactive Status Header**: Displays connection indicator, resolved `/dev/hidraw*` interface 1 control node path, and surfaces any permission or disconnection errors without panicking.
- **Color Controls**: Interactive color picker widget, manual RGB channel sliders, and hex text field supporting 6-digit hex values (e.g. `FF0000` or `#00FFFF`).
- **Non-Blocking Streaming Worker**: Direct RGB keepalive frames are transmitted at ~1.5 Hz (`KEEPALIVE_HZ`) on a dedicated background thread, matching the CLI retry and backoff behavior on transient communication errors. The UI rendering thread is never blocked on USB I/O.
- **Solid & Off Actions**: "Solid" starts or updates the continuous direct RGB keepalive stream. "Off" sends an all-zero frame to turn off lighting, cleanly terminates the worker thread, and closes the device descriptor.
- **Clean Lifecycle**: Closing the application window signals worker termination, joins the thread, and closes the file descriptor with zero resource leaks.

> [!NOTE]
> **CUSTOM Mode Requirement**: The physical keyboard must be toggled into its onboard "Custom" / user static RGB mode (via onboard Fn shortcuts) for software-driven colors to display.

### Verification Gate

All unit and integration tests are strictly hardware-free and testable without physical devices or active desktop display sessions:

```bash
cd rust
cargo test
```

