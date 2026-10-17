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
| **Multifunction Knob Mode** | Feature / Output | `TBD` | `TBD` | Interface 1 | Pending Tranche 3 USB Capture |
| **USB Polling Rate** | Feature / Output | `TBD` | `TBD` | Interface 1 | Pending Tranche 3 USB Capture |
| **Wireless Dongle Protocol (2.4GHz)** | Feature / Input | `TBD` | `TBD` | Interface 1 | Pending Tranche 3 USB Capture |
| **Battery Status Query** | Feature / Input | `TBD` | `TBD` | Interface 1 | Pending Tranche 3 USB Capture |

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


---

## 4. Credits

Protocol facts cross-checked with OpenRGB Sinowealth 010C driver, gamen-rgb, and kreo-hive65-rgb-linux; implementation here is original.

