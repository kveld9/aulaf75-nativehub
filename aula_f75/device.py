"""Device discovery and access abstraction for AULA F75 keyboard."""

from __future__ import annotations

import os
from typing import Any, Callable

try:
    import fcntl
except ImportError:  # pragma: no cover
    fcntl = None  # type: ignore[assignment]

# Table of known device Vendor and Product IDs (VID, PID)
KNOWN_DEVICES: dict[tuple[int, int], str] = {
    (0x258A, 0x010C): "AULA F75",
}

# Empirically verified RGB protocol constants (258a:010c)
FRAME_LEN: int = 520
REPORT_ID: int = 6
DIRECT_HEADER: bytes = bytes((0x06, 0x08, 0x00, 0x00, 0x01, 0x00, 0x7A, 0x01))
# Boards parse the full 512-byte payload region and the F75 rejects short-filled frames;
# header length field is kept for compatibility.
SLOT_COUNT: int = (FRAME_LEN - 8) // 3  # 170 slots
KEEPALIVE_HZ: float = 1.5




def _get_default_backend() -> Any:
    """Import and return the system hid module, or raise a RuntimeError."""
    try:
        import hid
        return hid
    except ImportError as exc:
        raise RuntimeError(
            "The hidapi library is required for device access. "
            "Install hidapi (e.g. 'pip install hidapi') or supply an injectable backend."
        ) from exc


def normalize_device_info(raw: dict[str, Any]) -> dict[str, Any]:
    """Normalize raw HID device dict into standard fields."""
    vid = raw.get("vendor_id", 0)
    pid = raw.get("product_id", 0)

    raw_path = raw.get("path", "")
    if isinstance(raw_path, bytes):
        path_str = raw_path.decode("utf-8", errors="replace")
    else:
        path_str = str(raw_path) if raw_path is not None else ""

    known_name = KNOWN_DEVICES.get((vid, pid))
    return {
        "vendor_id": vid,
        "product_id": pid,
        "path": path_str,
        "interface_number": raw.get("interface_number", -1),
        "manufacturer_string": raw.get("manufacturer_string") or "",
        "product_string": raw.get("product_string") or "",
        "release_number": raw.get("release_number", 0),
        "usage_page": raw.get("usage_page", 0),
        "usage": raw.get("usage", 0),
        "is_known": known_name is not None,
        "device_name": known_name if known_name is not None else "Unknown",
    }


def find_devices(backend: Any = None) -> list[dict[str, Any]]:
    """Enumerate HID devices and flag known AULA hardware.

    Args:
        backend: Optional injectable HID backend object providing enumerate().
                 If None, uses the default 'hid' library.

    Returns:
        List of normalized device information dictionaries.
    """
    if backend is None:
        backend = _get_default_backend()

    if not hasattr(backend, "enumerate"):
        return []

    try:
        raw_devices = backend.enumerate(0, 0)
    except TypeError:
        raw_devices = backend.enumerate()

    return [normalize_device_info(dev) for dev in (raw_devices or [])]


def open_device(
    path: str | bytes | None = None,
    vid: int | None = None,
    pid: int | None = None,
    backend: Any = None,
) -> Any:
    """Open a HID device by path or VID/PID using the provided backend.

    Args:
        path: Filesystem or bus path to hidraw device node.
        vid: Vendor ID (e.g. 0x258a).
        pid: Product ID (e.g. 0x010c).
        backend: Optional injectable HID backend. Defaults to 'hid' library.

    Returns:
        Opened device object.

    Raises:
        RuntimeError: If default backend cannot be imported.
        ValueError: If neither path nor (vid, pid) is specified.
        AttributeError: If backend does not support device opening methods.
    """
    if backend is None:
        backend = _get_default_backend()

    if path is None and (vid is None or pid is None):
        raise ValueError("Must specify either device path or both vid and pid")

    if hasattr(backend, "device"):
        dev = backend.device()
        if path is not None:
            raw_path = path.encode("utf-8") if isinstance(path, str) else path
            dev.open_path(raw_path)
        else:
            dev.open(vid, pid)
        return dev
    elif hasattr(backend, "Device"):
        if path is not None:
            raw_path = path.encode("utf-8") if isinstance(path, str) else path
            return backend.Device(path=raw_path)
        return backend.Device(vid=vid, pid=pid)
    elif hasattr(backend, "open_path") and path is not None:
        return backend.open_path(path)
    elif hasattr(backend, "open") and vid is not None and pid is not None:
        return backend.open(vid, pid)
    else:
        raise AttributeError("Provided backend does not implement device opening methods")


def build_direct_frame(rgb: tuple[int, int, int]) -> bytes:
    """Build a 520-byte direct RGB frame for the AULA F75 (Report ID 6).

    Args:
        rgb: Tuple of (R, G, B) channel values (0-255).

    Returns:
        Exact 520-byte frame with 8-byte header, 170 slots, and zero padding.

    Raises:
        ValueError: If rgb is not a 3-element tuple/list of integers in range 0-255.
    """
    if len(rgb) != 3:
        raise ValueError(f"RGB must contain exactly 3 channels, got {len(rgb)}")

    for idx, val in enumerate(rgb):
        if not isinstance(val, int) or isinstance(val, bool) or not (0 <= val <= 255):
            raise ValueError(
                f"RGB channel {idx} must be an integer in 0..255, got {val!r}"
            )

    r, g, b = rgb
    buf = bytearray(FRAME_LEN)
    buf[0:8] = DIRECT_HEADER
    for i in range(SLOT_COUNT):
        offset = 8 + 3 * i
        buf[offset] = r
        buf[offset + 1] = g
        buf[offset + 2] = b
    return bytes(buf)


def hid_set_feature(fd: int, data: bytes, ioctl_fn: Any = None) -> None:
    """Send a HID feature report using the Linux HIDIOCSFEATURE ioctl.

    Args:
        fd: Open file descriptor to a hidraw character device.
        data: Feature report payload (Report ID must be first byte).
        ioctl_fn: Optional callable replacing fcntl.ioctl (for testing/injection).

    Raises:
        ValueError: If data is empty or targets forbidden report ID 5.
        OSError: If fcntl is not available or the ioctl call fails.
    """
    if not data:
        raise ValueError("Feature report data cannot be empty")
    if data[0] == 5:
        raise ValueError("Safety violation: writes to report ID 5 are strictly prohibited")

    if fcntl is None and ioctl_fn is None:
        raise OSError("fcntl is not available on this platform")

    length = len(data)
    # HIDIOCSFEATURE(len) encoding:
    # dir = 3 (READ|WRITE), size = length, type = 0x48 ('H'), nr = 0x06
    # SET: req = (3 << 30) | (length << 16) | (0x48 << 8) | 0x06
    req = (3 << 30) | (length << 16) | (0x48 << 8) | 0x06

    fn = ioctl_fn if ioctl_fn is not None else fcntl.ioctl  # type: ignore[union-attr]
    buf = bytearray(data)
    fn(fd, req, buf)


def resolve_control_node(
    vid: int = 0x258A,
    pid: int = 0x010C,
    sys_class_hidraw: str | os.PathLike[str] = "/sys/class/hidraw",
    usb_devices: str | os.PathLike[str] = "/sys/bus/usb/devices",
) -> str | None:
    """Resolve the /dev/hidrawN path for USB interface 1 via sysfs without hidapi.

    Scans sys_class_hidraw entries, resolves /sys/class/hidraw/<name>/device to its
    real path, keeps nodes whose interface component matches interface number 1
    (path component like '5-2:1.1'), and confirms the parent USB device
    idVendor and idProduct files (hex) equal vid and pid.

    Args:
        vid: Expected USB Vendor ID (defaults to 0x258A).
        pid: Expected USB Product ID (defaults to 0x010C).
        sys_class_hidraw: Path to sysfs hidraw class directory.
        usb_devices: Path to sysfs usb bus devices directory.

    Returns:
        The device node path (e.g. '/dev/hidraw1') or None if not found.
    """
    sys_class_hidraw_str = str(sys_class_hidraw)
    usb_devices_str = str(usb_devices)

    if not os.path.isdir(sys_class_hidraw_str):
        return None

    try:
        entries = [e for e in os.listdir(sys_class_hidraw_str) if e.startswith("hidraw")]
    except OSError:
        return None

    # Sort numerically by hidraw index (hidraw0, hidraw1, ...)
    entries.sort(key=lambda s: int(s[6:]) if s[6:].isdigit() else s)

    for entry in entries:
        dev_link = os.path.join(sys_class_hidraw_str, entry, "device")
        if not os.path.exists(dev_link):
            continue

        real_path = os.path.realpath(dev_link)
        parts = [p for p in real_path.split(os.sep) if p]

        # Identify the interface component matching interface number 1
        # Exactly one colon AND the part after the colon starts with 1. (e.g. '5-2:1.1' -> config.iface 1.1)
        # Excludes PCI components (multiple colons: 0000:00:08.1) and HID components (not starting with 1.)
        # Excludes interface 0 (e.g. '5-2:1.0' does not end with .1)
        iface_idx = -1
        for idx, comp in enumerate(parts):
            if comp.count(":") == 1:
                after_colon = comp.split(":", 1)[1]
                if after_colon.startswith("1.") and after_colon.endswith(".1"):
                    iface_idx = idx
                    break


        if iface_idx == -1:
            continue

        iface_comp = parts[iface_idx]
        usb_dev_name = iface_comp.split(":", 1)[0]

        # Candidate directories containing idVendor and idProduct
        candidate_dirs: list[str] = []

        # 1. Immediate parent directory of interface component in real_path
        if iface_idx > 0:
            parent_dir = os.sep + os.path.join(*parts[:iface_idx])
            candidate_dirs.append(parent_dir)

        # 2. usb_devices symlink/directory for the device
        if os.path.isdir(usb_devices_str):
            candidate_dirs.append(os.path.join(usb_devices_str, usb_dev_name))

        # 3. Ancestors in real_path hierarchy
        curr = os.path.dirname(real_path)
        for _ in range(4):
            if curr and curr != os.path.dirname(curr):
                if curr not in candidate_dirs:
                    candidate_dirs.append(curr)
                curr = os.path.dirname(curr)

        matched = False
        for candidate in candidate_dirs:
            vendor_file = os.path.join(candidate, "idVendor")
            product_file = os.path.join(candidate, "idProduct")
            if os.path.isfile(vendor_file) and os.path.isfile(product_file):
                try:
                    with open(vendor_file, "r", encoding="utf-8") as vf:
                        read_vid = int(vf.read().strip(), 16)
                    with open(product_file, "r", encoding="utf-8") as pf:
                        read_pid = int(pf.read().strip(), 16)
                    if read_vid == vid and read_pid == pid:
                        matched = True
                        break
                except (OSError, ValueError):
                    continue

        if matched:
            return f"/dev/{entry}"

    return None


def control_node(
    devices: list[dict[str, Any]] | None = None,
    backend: Any = None,
    sys_class_hidraw: str | os.PathLike[str] = "/sys/class/hidraw",
    usb_devices: str | os.PathLike[str] = "/sys/bus/usb/devices",
) -> str | None:
    """Locate the filesystem path for the interface 1 hidraw control node.

    Tries sysfs resolution (resolve_control_node) first without requiring hidapi.
    Falls back to find_devices enumeration if sysfs resolution returns None.

    Args:
        devices: Optional list of normalized device dicts.
        backend: Optional injectable backend for find_devices().
        sys_class_hidraw: Path to sysfs hidraw directory.
        usb_devices: Path to sysfs usb devices directory.

    Returns:
        Filesystem path to the control node (e.g. '/dev/hidraw1') or None.
    """
    if devices is None and backend is None:
        sysfs_node = resolve_control_node(
            sys_class_hidraw=sys_class_hidraw,
            usb_devices=usb_devices,
        )
        if sysfs_node:
            return sysfs_node

    if devices is not None:
        # First preference: known AULA device with interface 1
        for dev in devices:
            if dev.get("is_known") and dev.get("interface_number") == 1:
                path = dev.get("path")
                if path:
                    return str(path)

        # Fallback preference: any interface 1 device
        for dev in devices:
            if dev.get("interface_number") == 1:
                path = dev.get("path")
                if path:
                    return str(path)

        return None

    try:
        devices = find_devices(backend=backend)
    except Exception:
        devices = None

    if devices is not None:
        for dev in devices:
            if dev.get("is_known") and dev.get("interface_number") == 1:
                path = dev.get("path")
                if path:
                    return str(path)

        for dev in devices:
            if dev.get("interface_number") == 1:
                path = dev.get("path")
                if path:
                    return str(path)

    # Fallback to sysfs resolver if enumeration was empty or unavailable
    return resolve_control_node(
        sys_class_hidraw=sys_class_hidraw,
        usb_devices=usb_devices,
    )



def default_opener() -> int:
    """Default opener: locate and open the interface 1 hidraw control node.

    Returns:
        Open file descriptor for the device node.

    Raises:
        OSError: If the control node cannot be located or opened.
    """
    node = control_node()
    if not node:
        raise OSError("AULA F75 interface 1 control node not found")
    return os.open(node, os.O_RDWR)


def send_frame(
    opener: Callable[[], int] | int | None = None,
    rgb: tuple[int, int, int] = (0, 0, 0),
    ioctl_fn: Any = None,
) -> None:
    """Build and send a direct RGB frame to the keyboard.

    Args:
        opener: Injectable callable returning an open fd-like int, an existing fd, or None.
                If None, opens the interface 1 hidraw control node and closes it on exit.
        rgb: (R, G, B) tuple of channel values in range 0-255.
        ioctl_fn: Optional callable replacing fcntl.ioctl for testing/injection.

    Raises:
        ValueError: If RGB channels are invalid or report ID is forbidden.
        OSError: If opening or writing to the device fails.
    """
    if isinstance(opener, (tuple, list)) and rgb == (0, 0, 0):
        rgb = opener  # type: ignore[assignment]
        opener = None

    frame = build_direct_frame(rgb)

    should_close = False
    if opener is None:
        fd = default_opener()
        should_close = True
    elif callable(opener):
        fd = opener()
    else:
        fd = opener

    try:
        hid_set_feature(fd, frame, ioctl_fn=ioctl_fn)
    finally:
        if should_close:
            try:
                os.close(fd)
            except OSError:
                pass

