# AULA F75 Desktop GUI (`aula-f75-gui`)

The native Linux desktop GUI application is built with `eframe` and `egui` (0.31), featuring a sober dark two-panel layout, direct USB HID control, and non-blocking host-rendered effect streaming.

## Requirements

- **Desktop Session**: Active X11 or Wayland display session.
- **Udev Rule**: Configured udev rule (`99-aula-f75.rules`) allowing unprivileged user access to `/dev/hidraw*`.
- **Keyboard Profile**: Physical keyboard toggled into its onboard "Custom" / user static RGB mode (via onboard Fn shortcuts).

---

## Running the GUI

```bash
# If installed via install.sh
aula-f75-gui

# Or run from release build directory
./target/release/aula-f75-gui

# Or run via cargo
cd rust
cargo run --bin aula-f75-gui
```

---

## Architecture & Features

### Cohesive Modern Dark Theme
- Custom Nordic dark palette with neutral panel/central fills and single cyan accent (`#00b4d8`).
- 4px-based spacing multiples, 6px consistent card corner radiuses, and readable typographic hierarchy.

### Side Panel Hardware Status & Reference
- **Connection Badge**: Multi-state pill indicator reporting `Streaming Active`, `Connected (idle)`, `Connecting...`, `Disconnected`, or `Error`.
- **Device Facts Grid**: Shows device model (`AULA F75`), VID:PID (`258a:010c`), interface 1 endpoint, and active `/dev/hidraw*` node path.
- **Diagnostic Microcopy**: Explains the probable cause and suggested remediation when a device node is missing or an error occurs.
- **Keyboard Reference**: Collapsible drawer documenting essential physical hardware shortcuts:
  - *Custom RGB Mode*: Required for host software lighting.
  - *Fn + Backspace*: Toggle backlight on / off.
  - *Knob Long-Press (~3-5s)*: Switch between Volume (multimedia) and Backlight lighting modes.
  - *Fn + B*: Battery level check (1-0 number row indicator).

### Segmented Effect Strip
Selectable card strip for instant switching between:
- `Solid`: Static single color across all keys.
- `Rainbow`: Continuous dynamic spectrum shift.
- `Wave`: Horizontal chromatic wave propagation.
- `Breathe`: Pulsing breathing cycle.
- `Stars`: Twinkling starry night pattern.
- `Off`: Clean hardware shutoff (all-zero frame).

### Live Color Preview Swatch & Sliders
- Interactive color picker and discrete R/G/B channel sliders.
- Hex color text input supporting 6-digit hex values with or without `#`.
- Real-time scaled output color swatch displaying active base vs scaled output color.
- Global brightness slider (`0%` to `100%`).
- Animation speed slider (`0.25x` to `3.0x`, default `1.0x`) for effects supporting speed adjustments (`Rainbow`, `Wave`, `Breathe`).

### Lighting Profiles Panel
Integrated management interface for persistent presets:
- Dropdown selector listing saved local profiles.
- **Apply** button to immediately stream preset parameters.
- **Delete** button to remove unwanted profile files.
- **Save Profile** input capturing active effect, color, brightness, and speed into `$XDG_CONFIG_HOME/aula-f75/profiles/`.

### Audio Reactive Lighting (Music Mode)
- **Reactive Mode**: Toggle between `Bars` (column spectrum) and `Pulse` (whole-board beat pulse).
- **Color Selection**: Dedicated color picker for reactive lighting.
- **Audio Gain**: Adjust input sensitivity multiplier (`0.1x` to `5.0x`).
- **Level Meter**: Real-time visual energy `ProgressBar` streamed via non-blocking channels from the worker thread.

### Non-Blocking Background Worker
- Direct Report 6 frames are streamed at ~25 Hz (`Duration::from_millis(40)`) for fluid animations and ~1.5 Hz (`KEEPALIVE_HZ`) for solid keepalive.
- Transient error retry (`EPIPE`, `ETIMEDOUT`) with exponential backoff on a dedicated background thread.
- The UI rendering thread is never blocked during hardware communication or device reconnects.

### Status Footer Bar
Persistent bottom bar reporting active stream name, estimated tick rate, and hardware communication status.

---

## Troubleshooting

### Unresponsive Window Dialog ("Application is not responding")
Direct USB HID communication executes exclusively on a dedicated background worker thread and never blocks the UI rendering thread. If the desktop environment displays an "Application is not responding" dialog:
1. Close the application window.
2. Unplug and replug the keyboard's USB cable.
3. Relaunch `aula-f75-gui`.
4. If persistent, verify permissions on `/dev/hidraw*` via `ls -l /dev/hidraw*` and ensure `99-aula-f75.rules` is active.

---

## See Also

- [CLI Command Reference](cli-reference.md)
- [Local Lighting Profiles](profiles.md)
- [Music Mode Reactive Lighting](music.md)
- [USB HID Protocol Specification](PROTOCOL.md)
- [Project Overview & Quickstart](../README.md)
