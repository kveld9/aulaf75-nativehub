# AULA F75 USB HID Protocol Specification

## 1. Transport Architecture

The AULA F75 mechanical keyboard communicates using standard USB Human Interface Device (USB HID) protocol.

### Verified Hardware Identification
- **Vendor ID (VID)**: `0x258a` (BY Tech Gaming)
- **Product ID (PID)**: `0x010c` (AULA F75 USB Mode)
- **Kernel Subsystem**: Linux `hid-generic` binds the interface and creates `/dev/hidraw*` character device nodes.
- **Access Control**: Device nodes default to root permissions; access is granted to unprivileged local users via `99-aula-f75.rules` using the systemd `uaccess` tag.

### Vendor Tool Background
The OEM configuration suite is distributed as a 32-bit PE executable (`.exe`) packaged with Inno Setup, accompanied by an MFC-based firmware update utility. This native Linux driver bypasses all vendor executables, communicating directly with `/dev/hidraw*`.

---

## 2. Feature Reports & Payload Layout

> [!IMPORTANT]
> **HARD SAFETY RULE — Report ID 5 Strictly Prohibited**:
> Report ID 5 is a write-only command mailbox on this MCU family (Sinowealth / BY Tech). Blind writes to Report 5 risk entering the ISP firmware-flash bootloader and causing a permanent or soft brick. Report 5 is explicitly **NOT mapped** and must never be sent.

### Verified Feature Reports

| Functional Area | Report Type | Report ID | Payload Size | Interface | Status |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **Direct RGB Frame** | Feature (`HIDIOCSFEATURE`) | `0x06` | 520 bytes (170 slots) | Interface 1 | Verified (2026-10-17) |
| **Command Mailbox / ISP** | Feature | `0x05` | N/A | Interface 1 | **PROHIBITED** (Soft-brick risk) |
| **Key Remapping** | Feature / Output | `TBD` | `TBD` | Interface 1 | Pending Tranche 3 USB Capture |
| **Macro Definitions** | Feature / Output | `TBD` | `TBD` | Interface 1 | Pending Tranche 3 USB Capture |
| **Multifunction Knob Mode** | Report Descriptor / Sysfs | `N/A` | Read-only | Interface 0 / 1 | Read-Only Observed; Host Remapping Pending Tranche 3 USB Capture |
| **USB Polling Rate** | Feature / Output | `TBD` | `TBD` | Interface 1 | Pending Tranche 3 USB Capture |
| **Wireless Dongle Protocol (2.4GHz)** | Sysfs Topology & Phys | `N/A` | Read-only | Interface 0 / 1 | Read-Only Topology & Dongle Detection; Feature Commands Pending Tranche 3 USB Capture |
| **Battery Status Query** | Sysfs & Feature (`HIDIOCGFEATURE`) | `TBD` | Read-only | Interface 1 | Read-Only Sysfs & GET_FEATURE Helper (Dormant in Wired Mode 258a:010c); Wireless Telemetry Pending Tranche 3 USB Capture |

---

## 3. Direct RGB Frame Specification (Report 0x06)

Confirmed live on `258a:010c` hardware (2026-10-17):

- **Transport**: Linux `hidraw` on USB interface 1 (composite control interface; not boot keyboard interface 0).
- **IOCTL Encoding**: `HIDIOCSFEATURE(520)` requires direction `_IOC_READ | _IOC_WRITE` (`dir = 3`). Calling ioctl with `dir = 1` fails with `EINVAL`:
  ```python
  req = (3 << 30) | (520 << 16) | (0x48 << 8) | 0x06
  ```
- **Frame Layout** (520 bytes total):
  - **Offsets `0..7`** (8 bytes): Fixed header: `06 08 00 00 01 00 7A 01` (header length field kept for compatibility).
  - **Offsets `8..517`** (510 bytes): 170 RGB slots (`SLOT_COUNT = (520 - 8) // 3 = 170`). Slot `i` (0 to 169) consists of 3 bytes `[R, G, B]` at byte offset `8 + 3 * i`:
    - Slot 0: offsets `8..10`
    - Slot 1: offsets `11..13`
    - Slot 169: offsets `515..517`
    - Whole-board effects populate all 170 slots with the same RGB triplet; per-key mapping will use slot-specific positions.
  - **Offsets `518..519`** (2 bytes): Zero padding (`0x00, 0x00`).
- **Payload Fill Invariant (F75 Observation)**: The microcontroller firmware parses the full 512-byte payload region. Empirical testing on real hardware demonstrated that short-filled frames (e.g. 126 slots with zeros after) trigger transient `EPIPE` stalls, whereas full 170-slot frames filling all 510 RGB bytes are accepted cleanly back-to-back.
- **Latching & Keepalive Behavior**: Colors latch immediately upon receiving the frame. No initialization handshake or apply command is needed. However, the MCU firmware reverts to its onboard lighting effect after a few idle seconds (~3-5 s). The host utility must resend the frame at ~1-2 Hz (`KEEPALIVE_HZ`) to maintain solid colors.
- **Onboard Mode Requirement**: The keyboard must be in its onboard custom/static RGB profile to display host software-driven frames.

### 3.1 Per-Key LED Slot Layout

Individual key LEDs map directly to slot indices `0..169` within the 520-byte Report 6 frame. Each slot occupies 3 contiguous bytes `[R, G, B]` starting at byte offset `8 + 3 * slot`. Unlit keys or unmapped slots remain zeroed (`00 00 00`).

The AULA F75 75% physical matrix provides 80 mapped LED slots (max slot index 89 < 170). Unmapped slots include physical matrix gaps and components without discrete LEDs (e.g. rotary knob):

| Row | Keys and Frame Slot Numbers (`slot = index`) |
| :--- | :--- |
| **Row 0** | `esc=0`, `f1=12`, `f2=18`, `f3=24`, `f4=30`, `f5=36`, `f6=42`, `f7=48`, `f8=54`, `f9=60`, `f10=66`, `f11=72`, `f12=78`, `del=85`, `pgup=86`, `pgdn=87` |
| **Row 1** | `backtick=1`, `1=7`, `2=13`, `3=19`, `4=25`, `5=31`, `6=37`, `7=43`, `8=49`, `9=55`, `0=61`, `minus=67`, `equal=73`, `backspace=79` |
| **Row 2** | `tab=2`, `q=8`, `w=14`, `e=20`, `r=26`, `t=32`, `y=38`, `u=44`, `i=50`, `o=56`, `p=62`, `lbracket=68`, `rbracket=74`, `backslash=80` |
| **Row 3** | `capslock=3`, `a=9`, `s=15`, `d=21`, `f=27`, `g=33`, `h=39`, `j=45`, `k=51`, `l=57`, `semicolon=63`, `quote=69`, `enter=81` |
| **Row 4** | `lshift=4`, `z=10`, `x=16`, `c=22`, `v=28`, `b=34`, `n=40`, `m=46`, `comma=52`, `dot=58`, `slash=64`, `rshift=70`, `up=82`, `end=88` |
| **Row 5** | `lctrl=5`, `lwin=11`, `lalt=17`, `space=35`, `rfn=53`, `rctrl=59`, `left=77`, `down=83`, `right=89` |

#### Key LED Protocol Behavior
- **Frame Assembly**: Per-key frames preserve the exact 8-byte header (`06 08 00 00 01 00 7A 01`), populate requested `(R, G, B)` triplets at `8 + 3 * slot`, leave non-targeted slots as `00 00 00`, and zero-pad the final 2 bytes (518..519).
- **Keepalive Invariant**: Per-key lighting follows the same keepalive requirement as full-board solid colors: frames must be resent at ~1.5 Hz (`KEEPALIVE_HZ`) to prevent the keyboard from reverting to onboard lighting patterns.
- **Credit & Status**: Key-to-slot mapping cross-checked against OpenRGB Sinowealth 010C driver (`aula_f75_layout`); hardware-free test suite verified; live hardware verification pending.

### 3.2 Host-Rendered RGB Effects Engine

To replicate vendor onboard lighting effects without touching dangerous or unverified feature reports (such as Report 5), all dynamic animations (`rainbow`, `wave`, `breathe`, `stars`) and static color modes (`solid`, `off`) are rendered purely on the host CPU and continuously streamed to the keyboard via Report 6 frames.

- **Streaming Transport**: Standard 520-byte Report 6 feature reports sent to interface 1 (`/dev/hidraw*`).
- **Refresh Rates**: Dynamic effects stream at ~25 Hz (`Duration::from_millis(40)` or tick frequency `25.0 Hz`) to achieve smooth perceptual color transitions; static `solid` mode streams at ~1.5 Hz (`KEEPALIVE_HZ`) to maintain the hardware keepalive latch.
- **Safety Invariant**: All host effects stream strictly on verified Report 6. Blind writes to Report 5 or non-verified configuration mailboxes remain strictly prohibited.

### 3.3 System-Audio Reactive Lighting (Music Mode)

Music mode renders dynamic audio-reactive lighting directly on the host CPU and streams Report 6 frames without vendor software or firmware modifications:

- **Host Loop & Frame Rate**: Sliding audio analysis and frame transmission run at ~20 fps (`Duration::from_millis(50)`). This refresh rate provides responsive visual beat synchronization while remaining comfortably within the hidraw throughput bounds verified on interface 1.
- **Audio Capture Transport**: Captures standard PulseAudio/PipeWire monitor streams using a child `parec` process (`s16le`, 44.1 kHz, stereo). The stream is mixed to mono `f32` in real time with an adjustable input gain.
- **FFT & Spectrum Analysis**: 1024-point radix-2 Cooley-Tukey FFT with precomputed bit-reversal and inline Hann windowing. Log-spaced energy bands (40 Hz to 12 kHz) are computed and normalized dynamically using a decaying peak tracker (`AutoGain`).
- **Matrix Mapping**:
  - `Bars`: Frequency band energies map horizontally across the physical 80-key layout following `KEY_SLOTS` order (`slot_i = band[i * nbands / nslots]`).
  - `Pulse`: Whole-board base color is scaled proportionally to average audio volume.
- **Safety Invariant**: Transmitted strictly on the verified Report 6 path (`06 08 00 00 01 00 7A 01` header). Report 5 remains prohibited.

### 3.4 Read-Only Hardware Discovery: Battery, Wireless Dongle & Rotary Knob

To discover hardware capabilities safely without vendor `.exe` utilities or hazardous mailbox writes:

1. **Battery Status Discovery**:
   - **Sysfs Power Supply Inspection**: Queries `/sys/class/power_supply` for matching kernel HID battery devices (`capacity`, `status`, `present`).
   - **Safe HID GET_FEATURE Telemetry**: Provides `HIDIOCGFEATURE` read helper protected by a hard guard against Report ID 5.
   - **Wired Invariant (PID 010c)**: In standard wired USB mode, the MCU firmware exposes no standard battery telemetry over feature reports or sysfs power_supply entries. Reports "Not reported by device" gracefully without inventing synthetic percentage figures.
   - **Pending Tranche 3 USB Capture**: Live USB capture during 2.4GHz wireless operation is required to confirm whether the wireless dongle multiplexes battery percentage via a dedicated input/feature report.

2. **2.4GHz Wireless Dongle vs Direct Cable Detection**:
   - **Sysfs Topology Resolution**: Evaluates USB bus parent identifiers, interface numbers, and `phys` attributes (e.g. `usb-0000:00:14.0-X/inputY`) across `/sys/class/hidraw/`.
   - **Candidate Classification**: Distinguishes direct USB cable connection on Interface 1 (control node) vs Interface 0 (boot keyboard) vs 2.4GHz wireless dongle receiver node based on device naming and physical topology without executing any device writes.
   - **Pending Tranche 3 USB Capture**: Vendor feature commands for wireless pairing, RF channel hopping, and dongle status require live USB packet analysis.

3. **Multifunction Rotary Knob Mode Discovery**:
   - **Report Descriptor Inspection**: Parses Linux `/sys/class/hidraw/hidrawX/device/report_descriptor` to determine what the OS observes: Consumer Control media events (Volume Increment `0x00E9`, Volume Decrement `0x00EA`, Mute `0x00E2`) vs Generic Desktop Dial/Wheel (`0x0037`/`0x0038`).
   - **Onboard Multiplexing**: The AULA F75 knob hardware supports switching between Volume and Backlight modes via onboard long-press (~3-5 s). This transition is handled entirely inside the keyboard's MCU firmware and does not re-enumerate the HID report descriptor or signal the host.
   - **Pending Tranche 3 USB Capture**: Capturing vendor software USB traffic is required to determine whether software-driven knob reassignment or lighting mode lock is possible via feature reports.

---

## 4. Credits

Protocol facts cross-checked with OpenRGB Sinowealth 010C driver (`aula_f75_layout`), gamen-rgb, and kreo-hive65-rgb-linux; implementation here is original.


