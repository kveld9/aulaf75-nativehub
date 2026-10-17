"""Command-line interface for AULA F75 inspection and configuration."""

from __future__ import annotations

import argparse
import sys
from typing import Any

from aula_f75.device import find_devices


def parse_hex_or_dec(val: str) -> int:
    """Parse string integer as hex or decimal."""
    try:
        return int(val, 0)
    except ValueError:
        return int(val, 16)


def cmd_list(args: argparse.Namespace, backend: Any = None) -> int:
    """List detected HID devices and flag known AULA hardware."""
    try:
        devices = find_devices(backend=backend)
    except Exception as exc:
        sys.stderr.write(f"Error enumerating devices: {exc}\n")
        return 1

    if not devices:
        print("No HID devices detected.")
        return 0

    print(f"Detected {len(devices)} HID interface(s):")
    for dev in devices:
        flag = "[MATCH]" if dev["is_known"] else "[     ]"
        vid_pid = f"{dev['vendor_id']:04x}:{dev['product_id']:04x}"
        path = dev["path"] or "<no-path>"
        iface = f"iface {dev['interface_number']}" if dev["interface_number"] >= 0 else "iface N/A"
        desc = dev["product_string"] or dev["device_name"]
        print(f"  {flag} {vid_pid}  {path:<20}  ({iface:<8})  {desc}")

    return 0


def cmd_info(args: argparse.Namespace, backend: Any = None) -> int:
    """Display detailed information for matching HID devices."""
    try:
        devices = find_devices(backend=backend)
    except Exception as exc:
        sys.stderr.write(f"Error enumerating devices: {exc}\n")
        return 1

    matched: list[dict[str, Any]] = []
    if args.path:
        matched = [d for d in devices if d["path"] == args.path]
    elif args.vid is not None and args.pid is not None:
        matched = [
            d for d in devices
            if d["vendor_id"] == args.vid and d["product_id"] == args.pid
        ]
    else:
        # Default: match any known AULA hardware
        matched = [d for d in devices if d["is_known"]]

    if not matched:
        print("No matching device found.")
        return 1

    for idx, dev in enumerate(matched, start=1):
        status_label = "Known AULA Device" if dev["is_known"] else "Unknown Device"
        print(f"Device #{idx} ({status_label}):")
        print(f"  Device Name:   {dev['device_name']}")
        print(f"  Vendor ID:     0x{dev['vendor_id']:04x}")
        print(f"  Product ID:    0x{dev['product_id']:04x}")
        print(f"  Path:          {dev['path']}")
        print(f"  Interface:     {dev['interface_number']}")
        print(f"  Manufacturer:  {dev['manufacturer_string']}")
        print(f"  Product:       {dev['product_string']}")
        print(f"  Release:       0x{dev['release_number']:04x}")
        print(f"  Usage Page:    0x{dev['usage_page']:04x}")
        print(f"  Usage:         0x{dev['usage']:04x}")

    return 0


def build_parser() -> argparse.ArgumentParser:
    """Build command-line argument parser."""
    parser = argparse.ArgumentParser(
        prog="aula-f75",
        description="Native Linux configuration utility for AULA F75 keyboard (read-only).",
    )
    subparsers = parser.add_subparsers(dest="command", required=True, help="Subcommand to execute")

    # list subcommand
    subparsers.add_parser("list", help="List detected HID devices and flag known AULA hardware")

    # info subcommand
    info_parser = subparsers.add_parser("info", help="Print details of matching AULA device")
    info_parser.add_argument("--path", help="Filter by device path (e.g. /dev/hidraw0)")
    info_parser.add_argument("--vid", type=parse_hex_or_dec, help="Filter by Vendor ID (hex or dec)")
    info_parser.add_argument("--pid", type=parse_hex_or_dec, help="Filter by Product ID (hex or dec)")

    return parser


def main(argv: list[str] | None = None, backend: Any = None) -> int:
    """CLI entry point."""
    parser = build_parser()
    args = parser.parse_args(argv)

    if args.command == "list":
        return cmd_list(args, backend=backend)
    elif args.command == "info":
        return cmd_info(args, backend=backend)

    parser.print_help()
    return 0


if __name__ == "__main__":
    sys.exit(main())
