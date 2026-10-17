# Repository Rules & Architecture Guidelines

## Core Principles

- **Linux-Native Only**: Direct Linux kernel USB HID interface via `hidraw`. Wine, Proton, Windows emulation, or vendor PE executable wrappers are strictly out of scope.
- **Target Runtime**: Python 3.14 on Linux with the `hidapi` C-extension (`import hid`).
- **Language**: All user-facing text, documentation, error messages, and commit messages must be strictly in English.

## Testing & Quality Gates

- **Verification Gate**: `pytest -q`. All unit tests must pass cleanly.
- **Hardware-Free Unit Tests**: Never require physical hardware in the test suite. All device interactions must use dependency injection with a mockable backend parameter in `find_devices()` and `open_device()`.
- **Scope Discipline**: Work within designated file-system boundaries and deliver clean, atomic changes.
