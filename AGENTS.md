# Repository Rules & Architecture Guidelines

## Core Principles

- **Linux-Native Only**: Direct Linux kernel USB HID interface via `hidraw`. Wine, Proton, Windows emulation, or vendor PE executable wrappers are strictly out of scope.
- **Target Runtime**: Python 3.10+ (tested on Python 3.14) on Linux with standard library `os` and `fcntl` for raw `hidraw` ioctls; optional `hidapi` C-extension for discovery. Rust edition 2021+ for the standalone CLI binary (`aula-f75-rs`) via direct `libc` ioctls with zero native deps beyond `libc`.
- **Language**: All user-facing text, documentation, error messages, and commit messages must be strictly in English.

## Direct RGB Protocol & Safety Rules

- **Control Node**: Interface 1 (`/dev/hidraw*`) handles feature report writes (not interface 0 boot keyboard).
- **Report 6 Direct RGB**: Single 520-byte feature report with fixed 8-byte header `06 08 00 00 01 00 7A 01`, 170 3-byte RGB slots at `8 + 3*i`, zero padding. Encoded with direction `READ|WRITE = 3` (`(3 << 30) | (520 << 16) | (0x48 << 8) | 0x06`).

- **CLI Commands**: `solid RRGGBB [--seconds N]` and `off` (solid `000000` for ~2s).
- **Keepalive Invariant**: Board reverts to onboard lighting effects after idle seconds; host software must resend direct frames at ~1-2 Hz (`KEEPALIVE_HZ`).
- **CUSTOM Mode Note**: Board must be toggled into its onboard custom/static lighting profile to render software RGB colors.
- **HARD SAFETY RULE — Report 5 Prohibition**: Never send data on Report ID 5. On this MCU family, Report 5 is a write-only command mailbox where blind writes risk entering the ISP firmware bootloader (soft-brick). Report 5 remains strictly unmapped and forbidden.

## Testing & Quality Gates

- **Verification Gate**:
  - Python: `pytest -q`
  - Rust: `cargo test` (executed inside `rust/`; offline-safe once crates.io dependencies are fetched)
- **Hardware-Free Unit Tests**: Never require physical hardware in the test suite. All device interactions must use dependency injection:
  - Discovery: sysfs-first resolution via `resolve_control_node()` with mockable backend parameter in `find_devices()` and `open_device()`, and injectable sysfs base paths in Rust (`resolve_control_node_with_roots()`).
  - RGB writes: injectable `opener` and `ioctl_fn` parameters in Python `send_frame()` and `main()`; mockable closures/injectable functions in Rust (`send_with_retry`, `run_keepalive_loop`) so unit tests never invoke real `fcntl`/`ioctl` or touch real hardware device nodes.
- **Scope Discipline**: Work within designated file-system boundaries and deliver clean, atomic changes.

