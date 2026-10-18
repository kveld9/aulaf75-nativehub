//! Device telemetry, battery status, wireless dongle, and knob discovery commands.

use std::path::Path;

use crate::device::{self, power};

/// Query and report battery status via sysfs power supply and safe HID read.
pub fn cmd_battery() -> i32 {
    let sys_class_hidraw = Path::new(device::DEFAULT_SYSFS_HIDRAW);
    let devices = device::enumerate_devices(sys_class_hidraw);
    let aula_devs: Vec<&device::DeviceInfo> = devices.iter().filter(|d| d.is_known).collect();

    if aula_devs.is_empty() {
        eprintln!("Error: AULA F75 device not detected.");
        return 1;
    }

    println!("Querying AULA F75 battery status...");

    let sysfs_power = Path::new("/sys/class/power_supply");
    let status = power::query_sysfs_battery_for_device(sysfs_power, Some("258A:010C"));

    if status.present && status.percent.is_some() {
        println!("Battery Status:");
        println!("  Present:       Yes");
        println!("  Level:         {}%", status.percent.unwrap());
        if let Some(chg) = status.charging {
            println!(
                "  State:         {}",
                if chg {
                    "Charging"
                } else {
                    "Discharging / Not charging"
                }
            );
        }
    } else {
        println!("Battery Status:  Not reported by device");
        println!("  Detail:        No sysfs power_supply or battery telemetry exposed in wired USB mode (258a:010c).");
        println!("  Note:          Requires live USB capture in 2.4GHz wireless / Bluetooth mode.");
    }

    0
}

/// Detect 2.4GHz wireless dongle vs direct cable connection topology.
pub fn cmd_wireless() -> i32 {
    let sys_class_hidraw = Path::new(device::DEFAULT_SYSFS_HIDRAW);
    let candidates = device::enumerate_wireless_nodes_with_roots(sys_class_hidraw);

    if candidates.is_empty() {
        eprintln!("Error: AULA F75 device not detected.");
        return 1;
    }

    println!("AULA F75 USB Connection & Dongle Detection:");
    let mut dongle_found = false;
    for (i, c) in candidates.iter().enumerate() {
        if c.is_dongle {
            dongle_found = true;
        }
        let iface_str = c
            .interface_number
            .map(|n| format!("Interface {}", n))
            .unwrap_or_else(|| "Interface N/A".to_string());
        println!("  Candidate #{}: {} ({})", i + 1, c.path.display(), iface_str);
        println!(
            "    USB Parent:    {}",
            if c.usb_parent.is_empty() {
                "N/A"
            } else {
                &c.usb_parent
            }
        );
        println!(
            "    Phys:          {}",
            if c.phys.is_empty() { "N/A" } else { &c.phys }
        );
        println!("    Classification: {:?}", c.connection_kind);
        println!("    Detail:        {}", c.detail);
    }

    println!();
    if dongle_found {
        println!("Summary: 2.4GHz wireless dongle receiver detected.");
    } else {
        println!("Summary: Direct USB cable connection active (no 2.4GHz dongle detected).");
    }

    0
}

/// Report currently observed multifunction knob mode.
pub fn cmd_knob() -> i32 {
    let sys_class_hidraw = Path::new(device::DEFAULT_SYSFS_HIDRAW);
    let devices = device::enumerate_devices(sys_class_hidraw);
    if !devices.iter().any(|d| d.is_known) {
        eprintln!("Error: AULA F75 device not detected.");
        return 1;
    }

    let report = device::query_knob_mode_with_roots(sys_class_hidraw);
    println!("AULA F75 Multifunction Knob Mode:");
    println!("  Behavior Source: {:?}", report.source);
    println!("  Description:     {}", report.description);
    for detail in &report.details {
        println!("  - {}", detail);
    }

    0
}
