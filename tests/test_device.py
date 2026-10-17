"""Unit tests for AULA F75 device discovery and CLI."""

from __future__ import annotations

import errno
import sys
from typing import Any
from unittest import mock

import pytest


from aula_f75.cli import main, parse_color_hex
from aula_f75.device import (
    DIRECT_HEADER,
    FRAME_LEN,
    KEEPALIVE_HZ,
    REPORT_ID,
    SLOT_COUNT,
    build_direct_frame,
    control_node,
    find_devices,
    hid_set_feature,
    open_device,
    resolve_control_node,
    send_frame,
)



class FakeHidDevice:
    """Mock HID device handle."""

    def __init__(self) -> None:
        self.opened_path: str | bytes | None = None
        self.opened_vid: int | None = None
        self.opened_pid: int | None = None
        self.is_open: bool = False

    def open_path(self, path: str | bytes) -> None:
        self.opened_path = path
        self.is_open = True

    def open(self, vid: int, pid: int) -> None:
        self.opened_vid = vid
        self.opened_pid = pid
        self.is_open = True

    def close(self) -> None:
        self.is_open = False


class FakeBackend:
    """Mock HID backend supporting enumeration and device opening."""

    def __init__(self, devices: list[dict[str, Any]] | None = None) -> None:
        self._devices = devices if devices is not None else []
        self.last_device: FakeHidDevice | None = None

    def enumerate(self, vid: int = 0, pid: int = 0) -> list[dict[str, Any]]:
        if vid and pid:
            return [
                d for d in self._devices
                if d.get("vendor_id") == vid and d.get("product_id") == pid
            ]
        return list(self._devices)

    def device(self) -> FakeHidDevice:
        self.last_device = FakeHidDevice()
        return self.last_device


def test_known_device_flagged():
    """Verify that a known AULA F75 device (258a:010c) is correctly flagged."""
    devices = [
        {
            "vendor_id": 0x258A,
            "product_id": 0x010C,
            "path": b"/dev/hidraw0",
            "interface_number": 0,
            "manufacturer_string": "BY Tech",
            "product_string": "Gaming Keyboard",
            "release_number": 0x0100,
            "usage_page": 1,
            "usage": 6,
        }
    ]
    backend = FakeBackend(devices)
    result = find_devices(backend=backend)

    assert len(result) == 1
    dev = result[0]
    assert dev["vendor_id"] == 0x258A
    assert dev["product_id"] == 0x010C
    assert dev["path"] == "/dev/hidraw0"
    assert dev["is_known"] is True
    assert dev["device_name"] == "AULA F75"


def test_unknown_device_listed_as_unknown():
    """Verify that an unrecognized device is not flagged as known."""
    devices = [
        {
            "vendor_id": 0x1234,
            "product_id": 0x5678,
            "path": "/dev/hidraw1",
            "interface_number": 1,
            "manufacturer_string": "Generic",
            "product_string": "USB Device",
        }
    ]
    backend = FakeBackend(devices)
    result = find_devices(backend=backend)

    assert len(result) == 1
    dev = result[0]
    assert dev["vendor_id"] == 0x1234
    assert dev["product_id"] == 0x5678
    assert dev["is_known"] is False
    assert dev["device_name"] == "Unknown"


def test_empty_enumeration_handled():
    """Verify that empty enumeration returns an empty list without error."""
    backend = FakeBackend([])
    result = find_devices(backend=backend)
    assert result == []


def test_open_device_missing_backend_raises_error():
    """Verify clear RuntimeError when backend is None and hid cannot be imported."""
    with mock.patch.dict(sys.modules, {"hid": None}):
        with pytest.raises(RuntimeError, match="hidapi library is required"):
            open_device(path="/dev/hidraw0", backend=None)


def test_open_device_by_path_and_vid_pid():
    """Verify opening a device using a fake backend by path and by VID/PID."""
    backend = FakeBackend()
    dev = open_device(path="/dev/hidraw0", backend=backend)
    assert dev.is_open is True
    assert dev.opened_path == b"/dev/hidraw0"

    dev2 = open_device(vid=0x258A, pid=0x010C, backend=backend)
    assert dev2.is_open is True
    assert dev2.opened_vid == 0x258A
    assert dev2.opened_pid == 0x010C


def test_open_device_requires_path_or_vid_pid():
    """Verify ValueError when neither path nor VID/PID is provided."""
    backend = FakeBackend()
    with pytest.raises(ValueError, match="Must specify either device path"):
        open_device(backend=backend)


def test_cli_list_command(capsys):
    """Test CLI 'list' command output formatting."""
    devices = [
        {
            "vendor_id": 0x258A,
            "product_id": 0x010C,
            "path": "/dev/hidraw0",
            "interface_number": 0,
            "product_string": "AULA F75 Keyboard",
        },
        {
            "vendor_id": 0x0001,
            "product_id": 0x0002,
            "path": "/dev/hidraw1",
            "interface_number": 1,
            "product_string": "Other Mouse",
        },
    ]
    backend = FakeBackend(devices)
    exit_code = main(["list"], backend=backend)
    assert exit_code == 0

    captured = capsys.readouterr().out
    assert "[MATCH]" in captured
    assert "258a:010c" in captured
    assert "/dev/hidraw0" in captured
    assert "[     ]" in captured
    assert "0001:0002" in captured


def test_cli_info_command(capsys):
    """Test CLI 'info' command prints matching device metadata."""
    devices = [
        {
            "vendor_id": 0x258A,
            "product_id": 0x010C,
            "path": "/dev/hidraw0",
            "interface_number": 0,
            "manufacturer_string": "BY Tech",
            "product_string": "AULA F75",
        }
    ]
    backend = FakeBackend(devices)
    exit_code = main(["info"], backend=backend)
    assert exit_code == 0

    captured = capsys.readouterr().out
    assert "Device Name:   AULA F75" in captured
    assert "Vendor ID:     0x258a" in captured
    assert "Product ID:    0x010c" in captured
    assert "/dev/hidraw0" in captured


def test_cli_info_command_not_found(capsys):
    """Test CLI 'info' command returns 1 when no matching device exists."""
    backend = FakeBackend([])
    exit_code = main(["info"], backend=backend)
    assert exit_code == 1

    captured = capsys.readouterr().out
    assert "No matching device found" in captured


def test_build_direct_frame_exact_structure():
    """Verify frame is exactly 520 bytes with exact 8-byte header and RGB at slots 0, 1, 169."""
    rgb = (0x12, 0x34, 0x56)
    frame = build_direct_frame(rgb)

    # Frame is exactly 520 bytes
    assert len(frame) == 520
    assert len(frame) == FRAME_LEN

    # Exact 8-byte header
    assert frame[:8] == DIRECT_HEADER
    assert frame[:8] == bytes((0x06, 0x08, 0x00, 0x00, 0x01, 0x00, 0x7A, 0x01))
    assert frame[0] == 0x06
    assert frame[0] == REPORT_ID

    # RGB at slot 0 (offset 8)
    assert frame[8:11] == bytes((0x12, 0x34, 0x56))

    # RGB at slot 1 (offset 11)
    assert frame[11:14] == bytes((0x12, 0x34, 0x56))

    # RGB at slot 169 (offset 8 + 3*169 = 515..517)
    assert frame[515:518] == bytes((0x12, 0x34, 0x56))

    # All 170 slots contain the specified RGB
    assert SLOT_COUNT == 170
    for i in range(SLOT_COUNT):
        offset = 8 + 3 * i
        assert frame[offset : offset + 3] == bytes((0x12, 0x34, 0x56))

    # Bytes 518..519 zero pad
    assert frame[518:520] == b"\x00\x00"



def test_build_direct_frame_invalid_channels_raise():
    """Verify invalid channel values raise ValueError."""
    # Negative channel values
    with pytest.raises(ValueError, match="must be an integer in 0..255"):
        build_direct_frame((-1, 100, 100))
    with pytest.raises(ValueError, match="must be an integer in 0..255"):
        build_direct_frame((100, -5, 100))
    with pytest.raises(ValueError, match="must be an integer in 0..255"):
        build_direct_frame((100, 100, -1))

    # Channel values > 255
    with pytest.raises(ValueError, match="must be an integer in 0..255"):
        build_direct_frame((256, 100, 100))
    with pytest.raises(ValueError, match="must be an integer in 0..255"):
        build_direct_frame((100, 300, 100))
    with pytest.raises(ValueError, match="must be an integer in 0..255"):
        build_direct_frame((100, 100, 999))

    # Non-integer values
    with pytest.raises(ValueError, match="must be an integer in 0..255"):
        build_direct_frame((100.5, 100, 100))  # type: ignore[arg-type]
    with pytest.raises(ValueError, match="must be an integer in 0..255"):
        build_direct_frame((True, 100, 100))  # type: ignore[arg-type]
    with pytest.raises(ValueError, match="must be an integer in 0..255"):
        build_direct_frame(("ff", 100, 100))  # type: ignore[arg-type]

    # Incorrect tuple lengths
    with pytest.raises(ValueError, match="exactly 3 channels"):
        build_direct_frame((100, 100))  # type: ignore[arg-type]
    with pytest.raises(ValueError, match="exactly 3 channels"):
        build_direct_frame((100, 100, 100, 100))  # type: ignore[arg-type]


def test_parse_color_hex_valid_and_invalid():
    """Verify hex parsing with/without '#', case-insensitivity, and error on invalid strings."""
    # Valid hex strings
    assert parse_color_hex("ff0000") == (255, 0, 0)
    assert parse_color_hex("#ff0000") == (255, 0, 0)
    assert parse_color_hex("00FF00") == (0, 255, 0)
    assert parse_color_hex("#00ff00") == (0, 255, 0)
    assert parse_color_hex("0000ff") == (0, 0, 255)
    assert parse_color_hex("#1A2b3C") == (0x1A, 0x2B, 0x3C)

    # Invalid lengths or characters raise ValueError
    with pytest.raises(ValueError, match="exactly 6 hex digits"):
        parse_color_hex("fff")
    with pytest.raises(ValueError, match="exactly 6 hex digits"):
        parse_color_hex("#fff")
    with pytest.raises(ValueError, match="exactly 6 hex digits"):
        parse_color_hex("12345")
    with pytest.raises(ValueError, match="exactly 6 hex digits"):
        parse_color_hex("1234567")
    with pytest.raises(ValueError, match="exactly 6 hex digits"):
        parse_color_hex("")
    with pytest.raises(ValueError, match="Invalid hex character"):
        parse_color_hex("zz0011")


def test_cli_solid_bad_hex_exits_nonzero(capsys):
    """Verify bad hex strings cause CLI solid command to exit non-zero (code 2)."""
    assert main(["solid", "xyz123"]) == 2
    assert main(["solid", "12345"]) == 2
    assert main(["solid", "#1234567"]) == 2
    assert main(["solid", ""]) == 2
    err = capsys.readouterr().err
    assert "Error" in err


def test_solid_sends_repeated_identical_frames():
    """Verify solid sends repeated identical frames through injected fake (count >= 2 for short --seconds)."""
    fake_fd = 42
    recorded_calls: list[tuple[int, int, bytes]] = []

    def fake_opener() -> int:
        return fake_fd

    def fake_ioctl(fd: int, req: int, data: Any) -> None:
        recorded_calls.append((fd, req, bytes(data)))

    exit_code = main(
        ["solid", "ff00aa", "--seconds", "0.01"],
        opener=fake_opener,
        ioctl_fn=fake_ioctl,
    )
    assert exit_code == 0

    # Must send repeated frames (count >= 2)
    assert len(recorded_calls) >= 2

    expected_frame = build_direct_frame((0xFF, 0x00, 0xAA))
    expected_req = (3 << 30) | (520 << 16) | (0x48 << 8) | 0x06

    for fd, req, data in recorded_calls:
        assert fd == fake_fd
        assert req == expected_req
        assert data == expected_frame
        assert data[:8] == DIRECT_HEADER
        assert data[8:11] == bytes((0xFF, 0x00, 0xAA))
        assert data[11:14] == bytes((0xFF, 0x00, 0xAA))
        assert data[515:518] == bytes((0xFF, 0x00, 0xAA))
        assert data[518:520] == b"\x00\x00"


def test_off_sends_all_zero_frames():
    """Verify off sends all-zero frames through injected fake."""
    fake_fd = 77
    recorded_calls: list[tuple[int, int, bytes]] = []

    def fake_opener() -> int:
        return fake_fd

    def fake_ioctl(fd: int, req: int, data: Any) -> None:
        recorded_calls.append((fd, req, bytes(data)))

    exit_code = main(
        ["off", "--seconds", "0.01"],
        opener=fake_opener,
        ioctl_fn=fake_ioctl,
    )
    assert exit_code == 0

    assert len(recorded_calls) >= 2

    expected_zero_frame = build_direct_frame((0x00, 0x00, 0x00))
    expected_req = (3 << 30) | (520 << 16) | (0x48 << 8) | 0x06

    for fd, req, data in recorded_calls:
        assert fd == fake_fd
        assert req == expected_req
        assert data == expected_zero_frame
        assert data[:8] == DIRECT_HEADER
        # slots 0, 1, 169 are (0, 0, 0)
        assert data[8:11] == bytes((0, 0, 0))
        assert data[11:14] == bytes((0, 0, 0))
        assert data[515:518] == bytes((0, 0, 0))
        # padding is all zero (bytes 518..519)
        assert data[518:520] == b"\x00\x00"



def test_report_5_never_referenced_by_send_path():
    """Verify report 5 is never referenced or produced by any send path."""
    import inspect
    import aula_f75.cli as cli_mod
    import aula_f75.device as dev_mod

    # 1. Report ID constant is 6
    assert REPORT_ID == 6

    # 2. Frame builders never set report ID 5
    direct_frame = build_direct_frame((10, 20, 30))
    assert direct_frame[0] == 0x06
    assert direct_frame[0] != 0x05

    # 3. Assert no b'\x05' first-byte frame builder exists in device module
    for name, obj in inspect.getmembers(dev_mod):
        if inspect.isfunction(obj) and name.startswith("build_"):
            try:
                out = obj((0, 0, 0))
                if isinstance(out, (bytes, bytearray)):
                    assert out[0] != 0x05
                    assert out[0] == 0x06
            except (TypeError, ValueError):
                pass

    # 4. Check hid_set_feature actively blocks report ID 5
    fake_calls = []

    def fake_ioctl(fd: int, req: int, data: Any) -> None:
        fake_calls.append(data)

    with pytest.raises(ValueError, match="report ID 5"):
        hid_set_feature(10, b"\x05" + b"\x00" * 519, ioctl_fn=fake_ioctl)
    assert len(fake_calls) == 0

    # 5. Check module source text: neither device.py nor cli.py references sending report 5
    dev_src = inspect.getsource(dev_mod)
    cli_src = inspect.getsource(cli_mod)
    assert "REPORT_ID = 5" not in dev_src
    assert "REPORT_ID = 5" not in cli_src


def test_control_node_resolution():
    """Verify control_node picks the interface-1 node."""
    devices = [
        {"vendor_id": 0x258A, "product_id": 0x010C, "interface_number": 0, "path": "/dev/hidraw0", "is_known": True},
        {"vendor_id": 0x258A, "product_id": 0x010C, "interface_number": 1, "path": "/dev/hidraw1", "is_known": True},
    ]
    node = control_node(devices=devices)
    assert node == "/dev/hidraw1"

    # When no interface 1 exists
    no_iface1 = [
        {"vendor_id": 0x258A, "product_id": 0x010C, "interface_number": 0, "path": "/dev/hidraw0", "is_known": True},
    ]
    assert control_node(devices=no_iface1) is None


def test_send_frame_with_fake_opener_and_ioctl():
    """Verify send_frame sends direct frame through fake opener and ioctl without real fcntl."""
    fake_fd = 99
    recorded = []

    def fake_opener() -> int:
        return fake_fd

    def fake_ioctl(fd: int, req: int, data: Any) -> None:
        recorded.append((fd, req, bytes(data)))

    send_frame(fake_opener, (12, 34, 56), ioctl_fn=fake_ioctl)
    assert len(recorded) == 1
    assert recorded[0][0] == fake_fd
    assert recorded[0][2][:8] == DIRECT_HEADER
    assert recorded[0][2][8:11] == bytes((12, 34, 56))


def test_keyboard_interrupt_clean_exit():
    """Verify KeyboardInterrupt in CLI exits cleanly with 0."""

    def fake_opener() -> int:
        raise KeyboardInterrupt()

    exit_code = main(["solid", "ffffff"], opener=fake_opener)
    assert exit_code == 0


def _create_fake_sysfs(
    base_path,
    hidraw_name="hidraw1",
    bus_id="5-2",
    iface_comp="5-2:1.1",
    vid_hex="258a",
    pid_hex="010c",
):
    sys_class_hidraw = base_path / "sys" / "class" / "hidraw"
    usb_devices = base_path / "sys" / "bus" / "usb" / "devices"
    sys_class_hidraw.mkdir(parents=True, exist_ok=True)
    usb_devices.mkdir(parents=True, exist_ok=True)

    # Device directory in /sys/devices/...
    dev_dir = base_path / "sys" / "devices" / "pci0000:00" / bus_id
    dev_dir.mkdir(parents=True, exist_ok=True)
    (dev_dir / "idVendor").write_text(f"{vid_hex}\n")
    (dev_dir / "idProduct").write_text(f"{pid_hex}\n")

    # Symlink in usb_devices
    usb_link = usb_devices / bus_id
    if not usb_link.exists():
        usb_link.symlink_to(dev_dir)

    # Interface directory
    iface_dir = dev_dir / iface_comp
    iface_dir.mkdir(parents=True, exist_ok=True)

    # HID child directory
    hid_dir = iface_dir / f"0003:{vid_hex.upper()}:{pid_hex.upper()}.0001"
    hid_dir.mkdir(parents=True, exist_ok=True)

    # hidraw entry
    hidraw_entry = sys_class_hidraw / hidraw_name
    hidraw_entry.mkdir(parents=True, exist_ok=True)
    dev_symlink = hidraw_entry / "device"
    if not dev_symlink.exists():
        dev_symlink.symlink_to(hid_dir)

    return sys_class_hidraw, usb_devices


def test_resolve_control_node_match_iface_1(tmp_path):
    """Verify sysfs resolver matches interface 1 and returns /dev/hidrawN."""
    sys_class_hidraw, usb_devices = _create_fake_sysfs(
        tmp_path,
        hidraw_name="hidraw1",
        bus_id="5-2",
        iface_comp="5-2:1.1",
        vid_hex="258a",
        pid_hex="010c",
    )
    result = resolve_control_node(
        vid=0x258A,
        pid=0x010C,
        sys_class_hidraw=sys_class_hidraw,
        usb_devices=usb_devices,
    )
    assert result == "/dev/hidraw1"


def test_resolve_control_node_skip_iface_0(tmp_path):
    """Verify sysfs resolver skips interface 0 and finds interface 1."""
    # Create iface 0 as hidraw0
    _create_fake_sysfs(
        tmp_path,
        hidraw_name="hidraw0",
        bus_id="5-2",
        iface_comp="5-2:1.0",
        vid_hex="258a",
        pid_hex="010c",
    )
    # Create iface 1 as hidraw1
    sys_class_hidraw, usb_devices = _create_fake_sysfs(
        tmp_path,
        hidraw_name="hidraw1",
        bus_id="5-2",
        iface_comp="5-2:1.1",
        vid_hex="258a",
        pid_hex="010c",
    )
    result = resolve_control_node(
        vid=0x258A,
        pid=0x010C,
        sys_class_hidraw=sys_class_hidraw,
        usb_devices=usb_devices,
    )
    assert result == "/dev/hidraw1"


def test_resolve_control_node_vid_mismatch_gives_none(tmp_path):
    """Verify sysfs resolver returns None when VID/PID does not match."""
    sys_class_hidraw, usb_devices = _create_fake_sysfs(
        tmp_path,
        hidraw_name="hidraw1",
        bus_id="5-2",
        iface_comp="5-2:1.1",
        vid_hex="9999",
        pid_hex="8888",
    )
    result = resolve_control_node(
        vid=0x258A,
        pid=0x010C,
        sys_class_hidraw=sys_class_hidraw,
        usb_devices=usb_devices,
    )
    assert result is None


def test_resolve_control_node_wrong_interface_gives_none(tmp_path):
    """Verify sysfs resolver returns None when only wrong interface (iface 0) exists."""
    sys_class_hidraw, usb_devices = _create_fake_sysfs(
        tmp_path,
        hidraw_name="hidraw0",
        bus_id="5-2",
        iface_comp="5-2:1.0",
        vid_hex="258a",
        pid_hex="010c",
    )
    result = resolve_control_node(
        vid=0x258A,
        pid=0x010C,
        sys_class_hidraw=sys_class_hidraw,
        usb_devices=usb_devices,
    )
    assert result is None


def test_control_node_tries_sysfs_first(tmp_path):
    """Verify control_node tries sysfs resolution first before falling back."""
    sys_class_hidraw, usb_devices = _create_fake_sysfs(
        tmp_path,
        hidraw_name="hidraw5",
        bus_id="1-2",
        iface_comp="1-2:1.1",
        vid_hex="258a",
        pid_hex="010c",
    )
    result = control_node(
        sys_class_hidraw=sys_class_hidraw,
        usb_devices=usb_devices,
    )
    assert result == "/dev/hidraw5"


def test_keepalive_retry_fail_twice_then_succeed():
    """Verify keepalive loop retries on EPIPE/ETIMEDOUT, succeeds on 3rd attempt, and returns 0 with 3+ sends."""
    fake_fd = 42
    calls: list[tuple[int, int, bytes]] = []
    attempt = 0

    def fake_opener() -> int:
        return fake_fd

    def fake_ioctl(fd: int, req: int, data: Any) -> None:
        nonlocal attempt
        attempt += 1
        calls.append((fd, req, bytes(data)))
        if attempt == 1:
            raise OSError(errno.EPIPE, "Broken pipe")
        elif attempt == 2:
            raise OSError(errno.ETIMEDOUT, "Connection timed out")
        # Attempt 3 and later succeed

    exit_code = main(
        ["solid", "ff0000", "--seconds", "0.01"],
        opener=fake_opener,
        ioctl_fn=fake_ioctl,
        sleep_fn=lambda _: None,
    )
    assert exit_code == 0
    assert len(calls) >= 3


def test_keepalive_retry_always_fail_exits_one(capsys):
    """Verify keepalive loop exits 1 after 10 consecutive retryable failures."""
    fake_fd = 42
    calls: list[tuple[int, int, bytes]] = []

    def fake_opener() -> int:
        return fake_fd

    def fake_ioctl(fd: int, req: int, data: Any) -> None:
        calls.append((fd, req, bytes(data)))
        raise OSError(errno.EPIPE, "Broken pipe")

    exit_code = main(
        ["solid", "ff0000", "--seconds", "0.01"],
        opener=fake_opener,
        ioctl_fn=fake_ioctl,
        sleep_fn=lambda _: None,
    )
    assert exit_code == 1
    assert len(calls) == 10
    err = capsys.readouterr().err
    assert "Device communication error" in err
    assert "Broken pipe" in err


def test_keepalive_non_retryable_errno_exits_immediately(capsys):
    """Verify keepalive loop exits 1 immediately on non-retryable errno (e.g. EACCES)."""
    fake_fd = 42
    calls: list[tuple[int, int, bytes]] = []

    def fake_opener() -> int:
        return fake_fd

    def fake_ioctl(fd: int, req: int, data: Any) -> None:
        calls.append((fd, req, bytes(data)))
        raise OSError(errno.EACCES, "Permission denied")

    exit_code = main(
        ["solid", "ff0000", "--seconds", "0.01"],
        opener=fake_opener,
        ioctl_fn=fake_ioctl,
        sleep_fn=lambda _: None,
    )
    assert exit_code == 1
    assert len(calls) == 1
    err = capsys.readouterr().err
    assert "Device communication error" in err
    assert "Permission denied" in err


def test_resolve_control_node_pci_multi_colon_regression(tmp_path):
    """Verify PCI multi-colon components (e.g. 0000:00:08.1) do not falsely match;

    skips hidraw1 (5-2:1.0 boot interface) and selects hidraw3 (5-2:1.1 control interface).
    """
    sys_class_hidraw = tmp_path / "sys" / "class" / "hidraw"
    usb_devices = tmp_path / "sys" / "bus" / "usb" / "devices"
    sys_class_hidraw.mkdir(parents=True, exist_ok=True)
    usb_devices.mkdir(parents=True, exist_ok=True)

    # Path with PCI multi-colon component: .../pci0000:00/0000:00:08.1/usb5/5-2
    dev_dir = tmp_path / "sys" / "devices" / "pci0000:00" / "0000:00:08.1" / "usb5" / "5-2"
    dev_dir.mkdir(parents=True, exist_ok=True)
    (dev_dir / "idVendor").write_text("258a\n")
    (dev_dir / "idProduct").write_text("010c\n")

    # Symlink in usb_devices
    (usb_devices / "5-2").symlink_to(dev_dir)

    # Interface 0 (boot keyboard interface -> hidraw1)
    iface0_dir = dev_dir / "5-2:1.0"
    iface0_dir.mkdir(parents=True, exist_ok=True)
    hid0_dir = iface0_dir / "0003:258A:010C.0008"
    hid0_dir.mkdir(parents=True, exist_ok=True)
    hidraw1_entry = sys_class_hidraw / "hidraw1"
    hidraw1_entry.mkdir(parents=True, exist_ok=True)
    (hidraw1_entry / "device").symlink_to(hid0_dir)

    # Interface 1 (control interface -> hidraw3)
    iface1_dir = dev_dir / "5-2:1.1"
    iface1_dir.mkdir(parents=True, exist_ok=True)
    hid1_dir = iface1_dir / "0003:258A:010C.0009"
    hid1_dir.mkdir(parents=True, exist_ok=True)
    hidraw3_entry = sys_class_hidraw / "hidraw3"
    hidraw3_entry.mkdir(parents=True, exist_ok=True)
    (hidraw3_entry / "device").symlink_to(hid1_dir)

    node = resolve_control_node(
        vid=0x258A,
        pid=0x010C,
        sys_class_hidraw=sys_class_hidraw,
        usb_devices=usb_devices,
    )
    # Must skip hidraw1 (5-2:1.0) and select hidraw3 (5-2:1.1)
    assert node == "/dev/hidraw3"




