"""Device discovery and access abstraction for AULA F75 keyboard."""

from __future__ import annotations

from typing import Any

# Table of known device Vendor and Product IDs (VID, PID)
KNOWN_DEVICES: dict[tuple[int, int], str] = {
    (0x258A, 0x010C): "AULA F75",
}


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
