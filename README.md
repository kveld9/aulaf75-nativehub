# aula-f75

Native Linux configuration utility for the AULA F75 mechanical keyboard via USB HID (`hidraw`).

## Overview

The AULA F75 keyboard exposes USB HID interfaces (typical VID `258a`, PID `010c`, BY Tech Gaming) handled by the Linux kernel `hid-generic` driver. This project provides a standalone, Linux-native tool to detect, inspect, and configure keyboard hardware directly through `/dev/hidraw*` device nodes without proprietary vendor software.

Tranche 1 establishes repository scaffolding, injectable device abstractions, and read-only device access (enumeration and inspection).

## Non-Goals

- **No Wine or Windows Emulation**: Operates directly on Linux kernel hidraw interfaces.
- **No Firmware Flashing**: Firmware flashing and bootloader updates are out of scope.
- **No Blind Protocol Writes**: Feature-report writes (RGB lighting, key remapping, macros) remain disabled until payload structures are verified via USB captures in Tranche 2.

## Udev Rule Setup

By default, `/dev/hidraw*` device nodes require elevated privileges. Install the provided udev rule to grant unprivileged user access via `uaccess`:

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
