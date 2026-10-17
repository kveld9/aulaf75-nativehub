"""Unit tests for AULA F75 device discovery and CLI."""

from __future__ import annotations

import sys
from typing import Any
from unittest import mock

import pytest

from aula_f75.cli import main
from aula_f75.device import find_devices, open_device


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
