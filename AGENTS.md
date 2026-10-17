# Repository Rules & Architecture Guidelines

## Core Principles

- **Linux-Native Only**: Direct Linux kernel USB HID interface via `hidraw`. Wine, Proton, Windows emulation, or vendor PE executable wrappers are strictly out of scope.
- **Target Runtime**: Python 3.10+ (tested on Python 3.14) on Linux with standard library `os` and `fcntl` for raw `hidraw` ioctls; optional `hidapi` C-extension for discovery. Rust edition 2021+ for the standalone CLI binary (`aula-f75-rs`) and native desktop GUI (`aula-f75-gui`) via `egui`/`eframe`.
- **Language**: All user-facing text, documentation, error messages, and commit messages must be strictly in English.

## Direct RGB Protocol & Safety Rules

- **Control Node**: Interface 1 (`/dev/hidraw*`) handles feature report writes (not interface 0 boot keyboard).
- **Report 6 Direct RGB**: Single 520-byte feature report with fixed 8-byte header `06 08 00 00 01 00 7A 01`, 170 3-byte RGB slots at `8 + 3*i`, zero padding. Encoded with direction `READ|WRITE = 3` (`(3 << 30) | (520 << 16) | (0x48 << 8) | 0x06`).

- **CLI & GUI Modes**: `solid RRGGBB [--seconds N]`, `off` (solid `000000` for ~2s), `key NAME COLOR [--seconds N]`, and `keys` (list all mapped keys with slot numbers) in CLI; interactive color picker, hex input, and non-blocking streaming keepalive worker in GUI (`cargo run --bin aula-f75-gui`). Requires desktop session (X11/Wayland) and udev rule.
- **Per-Key LED Mapping (AULA F75)**: 80 physical keys mapped to frame slot indices (0..169, max index 89). Byte offset is `8 + 3 * slot`. Non-targeted slots are zero-filled. Case-insensitive lookup with standard aliases (`esc`/`escape`, `space`/`spacebar`, `-`/`minus`, arrows, etc.). Layout source: OpenRGB Sinowealth 010C `aula_f75_layout` (live hardware verification pending).
- **Keepalive Invariant**: Board reverts to onboard lighting effects after idle seconds; host software must resend direct frames at ~1-2 Hz (`KEEPALIVE_HZ`).
- **CUSTOM Mode Note**: Board must be toggled into its onboard custom/static lighting profile to render software RGB colors.
- **HARD SAFETY RULE — Report 5 Prohibition**: Never send data on Report ID 5. On this MCU family, Report 5 is a write-only command mailbox where blind writes risk entering the ISP firmware bootloader (soft-brick). Report 5 remains strictly unmapped and forbidden.

## Testing & Quality Gates

- **Verification Gate**:
  - Python: `pytest -q`
  - Rust: `cargo test` (executed inside `rust/`; tests CLI logic, keymap frame offsets/uniqueness, and GUI state/channel streaming; skips GUI window rendering; offline-safe once crates.io dependencies are fetched)
- **Hardware-Free Unit Tests**: Never require physical hardware in the test suite. All device interactions must use dependency injection:
  - Discovery: sysfs-first resolution via `resolve_control_node()` with mockable backend parameter in `find_devices()` and `open_device()`, and injectable sysfs base paths in Rust (`resolve_control_node_with_roots()`).
  - RGB writes: injectable `opener` and `ioctl_fn` parameters in Python `send_frame()` and `main()`; mockable closures/injectable functions in Rust (`send_with_retry`, `run_keepalive_loop`, `send_key_frame`, `run_worker_stream`) so unit tests never invoke real `fcntl`/`ioctl` or touch real hardware device nodes.
  - Key mapping: frame offset verification (e.g. esc at bytes 8..10, space at 8+35*3), alias resolution, slot uniqueness (< 170), and key enumeration (>= 80 keys) verified purely in-memory.
  - GUI state: pure state machine transitions (`transition_to_solid`, `transition_to_off`), hex parsing, and channel stream tests (`run_worker_stream`) executed with mock transmitters and receivers without spawning active GUI windows or connecting to hardware.
- **Scope Discipline**: Work within designated file-system boundaries and deliver clean, atomic changes.

