"""Command-line interface for AULA F75 inspection and configuration."""

from __future__ import annotations

import argparse
import errno
import os
import sys
import time
from typing import Any


from aula_f75.device import (
    KEEPALIVE_HZ,
    control_node,
    find_devices,
    send_frame,
)


def parse_hex_or_dec(val: str) -> int:
    """Parse string integer as hex or decimal."""
    try:
        return int(val, 0)
    except ValueError:
        return int(val, 16)


def parse_color_hex(val: str) -> tuple[int, int, int]:
    """Parse a 6-digit hex string (with optional leading '#') into an (R, G, B) tuple.

    Args:
        val: Hex color string (e.g. 'ff0000', '#00ff00', '1A2B3C').

    Returns:
        Tuple of (R, G, B) integers (0-255).

    Raises:
        ValueError: If string is not exactly 6 hex characters or contains invalid characters.
    """
    s = val.strip()
    if s.startswith("#"):
        s = s[1:]
    if len(s) != 6:
        raise ValueError(f"Color must be exactly 6 hex digits, got {val!r}")
    try:
        r = int(s[0:2], 16)
        g = int(s[2:4], 16)
        b = int(s[4:6], 16)
    except ValueError as exc:
        raise ValueError(f"Invalid hex character in color {val!r}") from exc
    return (r, g, b)


def _run_keepalive_loop(
    rgb: tuple[int, int, int],
    seconds: float = 0.0,
    backend: Any = None,
    opener: Any = None,
    ioctl_fn: Any = None,
    sleep_fn: Any = None,
    time_fn: Any = None,
) -> int:
    """Send RGB frame in a keepalive loop until seconds expire or Ctrl+C is pressed."""
    clock = time_fn if time_fn is not None else time.monotonic
    sleep = sleep_fn if sleep_fn is not None else time.sleep

    if opener is None and hasattr(backend, "opener"):
        opener = backend.opener
    if ioctl_fn is None and hasattr(backend, "ioctl_fn"):
        ioctl_fn = backend.ioctl_fn
    elif ioctl_fn is None and hasattr(backend, "ioctl"):
        ioctl_fn = backend.ioctl

    close_fd = False
    fd = None
    if opener is None:
        node = control_node(backend=backend)
        if not node:
            sys.stderr.write("Error: AULA F75 interface 1 control node not found.\n")
            return 1
        try:
            fd = os.open(node, os.O_RDWR)
            close_fd = True
            opener = lambda: fd
        except OSError as exc:
            sys.stderr.write(f"Error opening control node {node}: {exc}\n")
            return 1

    interval = 1.0 / KEEPALIVE_HZ
    start_time = clock()
    count = 0
    consecutive_failures = 0
    try:
        while True:
            try:
                send_frame(opener, rgb, ioctl_fn=ioctl_fn)
                consecutive_failures = 0
            except OSError as exc:
                if exc.errno in (errno.EPIPE, errno.ETIMEDOUT):
                    consecutive_failures += 1
                    if consecutive_failures >= 10:
                        sys.stderr.write(f"Device communication error: {exc}\n")
                        return 1
                    sleep(interval)
                    continue
                sys.stderr.write(f"Device communication error: {exc}\n")
                return 1

            count += 1
            if seconds > 0:
                elapsed = clock() - start_time
                if elapsed >= seconds and count >= 2:
                    break
            sleep(interval)
            if seconds > 0:
                elapsed = clock() - start_time
                if elapsed >= seconds and count >= 2:
                    break
    except KeyboardInterrupt:
        return 0
    except OSError as exc:
        sys.stderr.write(f"Device communication error: {exc}\n")
        return 1

    finally:
        if close_fd and fd is not None:
            try:
                os.close(fd)
            except OSError:
                pass

    return 0


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


def cmd_solid(
    args: argparse.Namespace,
    backend: Any = None,
    opener: Any = None,
    ioctl_fn: Any = None,
    sleep_fn: Any = None,
    time_fn: Any = None,
) -> int:
    """Set solid RGB color on all keys, keeping it alive until interrupted or expired."""
    try:
        rgb = parse_color_hex(args.color)
    except ValueError as exc:
        sys.stderr.write(f"Error parsing hex color: {exc}\n")
        return 2

    return _run_keepalive_loop(
        rgb=rgb,
        seconds=getattr(args, "seconds", 0.0),
        backend=backend,
        opener=opener,
        ioctl_fn=ioctl_fn,
        sleep_fn=sleep_fn,
        time_fn=time_fn,
    )


def cmd_off(
    args: argparse.Namespace,
    backend: Any = None,
    opener: Any = None,
    ioctl_fn: Any = None,
    sleep_fn: Any = None,
    time_fn: Any = None,
) -> int:
    """Turn off RGB lighting by setting solid 000000 for a brief hold (~2s) then exit."""
    seconds = getattr(args, "seconds", 2.0)
    if seconds <= 0:
        seconds = 2.0
    return _run_keepalive_loop(
        rgb=(0, 0, 0),
        seconds=seconds,
        backend=backend,
        opener=opener,
        ioctl_fn=ioctl_fn,
        sleep_fn=sleep_fn,
        time_fn=time_fn,
    )


def build_parser() -> argparse.ArgumentParser:
    """Build command-line argument parser."""
    parser = argparse.ArgumentParser(
        prog="aula-f75",
        description="Native Linux configuration utility for AULA F75 keyboard.",
    )
    subparsers = parser.add_subparsers(dest="command", required=True, help="Subcommand to execute")

    # list subcommand
    subparsers.add_parser("list", help="List detected HID devices and flag known AULA hardware")

    # info subcommand
    info_parser = subparsers.add_parser("info", help="Print details of matching AULA device")
    info_parser.add_argument("--path", help="Filter by device path (e.g. /dev/hidraw0)")
    info_parser.add_argument("--vid", type=parse_hex_or_dec, help="Filter by Vendor ID (hex or dec)")
    info_parser.add_argument("--pid", type=parse_hex_or_dec, help="Filter by Product ID (hex or dec)")

    # solid subcommand
    solid_parser = subparsers.add_parser(
        "solid", help="Set solid RGB color on all keys"
    )
    solid_parser.add_argument(
        "color", help="Hex color string (e.g. 'ff0000', '#00ff00')"
    )
    solid_parser.add_argument(
        "--seconds",
        type=float,
        default=0.0,
        help="Duration to maintain color in seconds (0 = hold until Ctrl+C)",
    )

    # off subcommand
    off_parser = subparsers.add_parser(
        "off", help="Turn off RGB lighting (solid 000000 for ~2s)"
    )
    off_parser.add_argument(
        "--seconds",
        type=float,
        default=2.0,
        help="Duration to maintain off frame in seconds (default: 2.0s)",
    )

    return parser


def main(
    argv: list[str] | None = None,
    backend: Any = None,
    opener: Any = None,
    ioctl_fn: Any = None,
    sleep_fn: Any = None,
    time_fn: Any = None,
) -> int:
    """CLI entry point."""
    parser = build_parser()
    try:
        args = parser.parse_args(argv)
    except SystemExit as exc:
        return exc.code if isinstance(exc.code, int) else 2
    except KeyboardInterrupt:
        return 0

    try:
        if args.command == "list":
            return cmd_list(args, backend=backend)
        elif args.command == "info":
            return cmd_info(args, backend=backend)
        elif args.command == "solid":
            return cmd_solid(
                args,
                backend=backend,
                opener=opener,
                ioctl_fn=ioctl_fn,
                sleep_fn=sleep_fn,
                time_fn=time_fn,
            )
        elif args.command == "off":
            return cmd_off(
                args,
                backend=backend,
                opener=opener,
                ioctl_fn=ioctl_fn,
                sleep_fn=sleep_fn,
                time_fn=time_fn,
            )
    except KeyboardInterrupt:
        return 0

    parser.print_help()
    return 0


if __name__ == "__main__":
    sys.exit(main())

