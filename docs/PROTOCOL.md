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

## 2. Feature Reports & Payload Layout (Tranche 2 Workboard)

> [!IMPORTANT]
> **No Speculative Report IDs**: Report IDs and byte structures must be extracted empirically from `usbmon` packet captures in Tranche 2. Do not guess or invent report structures.

| Functional Area | Report Type | Report ID | Payload Size | Status |
| :--- | :--- | :--- | :--- | :--- |
| **RGB Mode & Effects** | Feature / Output | `TBD` | `TBD` | Pending Tranche 2 USB Capture |
| **RGB Brightness & Speed** | Feature / Output | `TBD` | `TBD` | Pending Tranche 2 USB Capture |
| **Per-Key RGB Matrix** | Feature / Output | `TBD` | `TBD` | Pending Tranche 2 USB Capture |
| **Key Remapping** | Feature / Output | `TBD` | `TBD` | Pending Tranche 2 USB Capture |
| **Macro Definitions** | Feature / Output | `TBD` | `TBD` | Pending Tranche 2 USB Capture |
| **Multifunction Knob Mode** | Feature / Output | `TBD` | `TBD` | Pending Tranche 2 USB Capture |
| **USB Polling Rate** | Feature / Output | `TBD` | `TBD` | Pending Tranche 2 USB Capture |
| **Wireless Dongle Protocol (2.4GHz)** | Feature / Input | `TBD` | `TBD` | Pending Tranche 2 USB Capture |
| **Battery Status Query** | Feature / Input | `TBD` | `TBD` | Pending Tranche 2 USB Capture |

---

## 3. Tranche 2 Capture Methodology

To populate the TBD tables above during Tranche 2:
1. Mount debugfs if necessary (`mount -t debugfs none_debugs /sys/kernel/debug`).
2. Load the `usbmon` kernel module (`modprobe usbmon`).
3. Identify the USB bus hosting `258a:010c` (`lsusb`).
4. Capture raw HID feature and output reports using Wireshark or `tcpdump -i usbmon<bus>`.
5. Trigger parameter changes in the reference environment (adjusting brightness, switching RGB patterns, binding macros) and isolate the corresponding delta payloads.
