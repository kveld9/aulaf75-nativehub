# System Audio Reactive Lighting (Music Mode)

Audio-reactive RGB lighting (`music` mode) provides real-time keyboard illumination driven by system audio playback without requiring third-party C/C++ audio library dependencies.

## Architecture & Design

- **Audio Capture**: Spawns `parec` on the default output sink monitor (`pactl get-default-sink` + `.monitor`) using 44.1 kHz 16-bit little-endian stereo PCM (`s16le`).
- **Signal Processing**: Hand-rolled radix-2 Cooley-Tukey FFT implemented purely in the Rust standard library with inline Hann windowing and precomputed twiddle factor tables.
- **Spectrum & AutoGain**: Analyzes 16 log-spaced frequency bands (40 Hz to 12 kHz) with an exponential decaying peak tracker (`AutoGain`) to automatically adjust sensitivity to audio volume.
- **Hardware Rendering**: Maps normalized frequency band energies to Report 6 direct RGB frames streamed at ~20 fps.

---

## Requirements

1. **Linux Audio Server**: PulseAudio or PipeWire with `pulseaudio-utils` or `pipewire-pulse` installed (providing `parec` and `pactl`).
2. **Keyboard Profile**: Physical keyboard toggled into its onboard **Custom / Static RGB** mode (via onboard Fn shortcuts).

---

## CLI Usage

The `music` subcommand supports two reactive visualization modes:
- `bars`: Frequency spectrum mapped across physical key columns (low bass on the left, high treble on the right).
- `pulse`: Whole-board beat pulse responding to overall audio energy.

```bash
# Run bars mode with default cyan color (#00FFFF)
aula-f75-rs music

# Run pulse mode with magenta color and 2.0x gain for 10 seconds
aula-f75-rs music --mode pulse --color ff00aa --gain 2.0 --seconds 10

# Run bars mode with custom color and higher sensitivity
aula-f75-rs music --mode bars --color 00ff88 --gain 2.5
```

### Parameters

| Option | Default | Description |
| :--- | :--- | :--- |
| `--mode` | `bars` | Reactive lighting pattern (`bars` or `pulse`) |
| `--color` | `00ffff` | Base hex color triplet (`RRGGBB` or `#RRGGBB`) |
| `--gain` | `1.0` | Audio sensitivity multiplier (`0.1` to `5.0`) |
| `--seconds` | None | Run duration in seconds (0 or omitted = run until Ctrl+C) |

---

## GUI Integration

The native desktop GUI (`aula-f75-gui`) includes a dedicated **Audio Reactive Lighting (Music Mode)** panel:
- **Reactive Mode**: Toggle between `Bars` (column spectrum) and `Pulse` (whole-board beat pulse).
- **Color Selection**: Dedicated interactive color picker for the reactive base illumination.
- **Audio Gain Slider**: Dynamic sensitivity multiplier slider (`0.1x` to `5.0x`).
- **Real-Time Level Meter**: Energy level `ProgressBar` fed via non-blocking channels from the worker thread.
- **Non-Blocking Execution**: Audio capture and FFT processing run exclusively on the background worker thread, ensuring the GUI remains fully responsive.

---

## See Also

- [CLI Command Reference](cli-reference.md)
- [Desktop GUI Documentation](gui.md)
- [Local Lighting Profiles](profiles.md)
- [USB HID Protocol Specification](PROTOCOL.md)
- [Project Overview & Quickstart](../README.md)
