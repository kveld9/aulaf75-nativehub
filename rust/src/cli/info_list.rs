//! Device enumeration and detailed inspection commands for AULA F75.

use crate::device::DeviceInfo;

/// List detected HID devices and flag known AULA hardware.
pub fn cmd_list(devices: &[DeviceInfo]) -> i32 {
    if devices.is_empty() {
        println!("No HID devices detected.");
        return 0;
    }

    println!("Detected {} HID interface(s):", devices.len());
    for dev in devices {
        let flag = if dev.is_known { "[MATCH]" } else { "[     ]" };
        let vid_pid = format!("{:04x}:{:04x}", dev.vendor_id, dev.product_id);
        let path = dev.path.to_string_lossy();
        let iface = match dev.interface_number {
            Some(n) if n >= 0 => format!("iface {}", n),
            _ => "iface N/A".to_string(),
        };
        let desc = if !dev.product_string.is_empty() {
            &dev.product_string
        } else {
            &dev.device_name
        };
        println!("  {} {}  {:<20}  ({:<8})  {}", flag, vid_pid, path, iface, desc);
    }
    0
}

/// Print details of matching AULA device.
pub fn cmd_info(
    devices: &[DeviceInfo],
    path: Option<&str>,
    vid: Option<u16>,
    pid: Option<u16>,
) -> i32 {
    let matched: Vec<&DeviceInfo> = if path.is_some() || vid.is_some() || pid.is_some() {
        devices
            .iter()
            .filter(|d| {
                if let Some(p) = path {
                    if d.path.to_string_lossy() != p {
                        return false;
                    }
                }
                if let Some(v) = vid {
                    if d.vendor_id != v {
                        return false;
                    }
                }
                if let Some(pi) = pid {
                    if d.product_id != pi {
                        return false;
                    }
                }
                true
            })
            .collect()
    } else {
        devices.iter().filter(|d| d.is_known).collect()
    };

    if matched.is_empty() {
        println!("No matching device found.");
        return 1;
    }

    for (idx, dev) in matched.iter().enumerate() {
        let status_label = if dev.is_known {
            "Known AULA Device"
        } else {
            "Unknown Device"
        };
        println!("Device #{} ({}):", idx + 1, status_label);
        println!("  Device Name:   {}", dev.device_name);
        println!("  Vendor ID:     0x{:04x}", dev.vendor_id);
        println!("  Product ID:    0x{:04x}", dev.product_id);
        println!("  Path:          {}", dev.path.display());
        println!(
            "  Interface:     {}",
            dev.interface_number
                .map(|n| n.to_string())
                .unwrap_or_else(|| "N/A".to_string())
        );
        println!("  Manufacturer:  {}", dev.manufacturer_string);
        println!("  Product:       {}", dev.product_string);
        println!("  Release:       0x{:04x}", dev.release_number);
        println!("  Usage Page:    0x{:04x}", dev.usage_page);
        println!("  Usage:         0x{:04x}", dev.usage);
        println!(
            "  USB Parent:    {}",
            if dev.usb_parent.is_empty() {
                "N/A"
            } else {
                &dev.usb_parent
            }
        );
        println!(
            "  Phys:          {}",
            if dev.phys.is_empty() {
                "N/A"
            } else {
                &dev.phys
            }
        );
        println!("  Connection:    {:?}", dev.classify_connection());
    }

    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cmd_info_single_filter_and_combinations() {
        let test_devs = vec![
            DeviceInfo {
                vendor_id: 0x258A,
                product_id: 0x010C,
                path: std::path::PathBuf::from("/dev/hidraw1"),
                interface_number: Some(1),
                manufacturer_string: "AULA".to_string(),
                product_string: "AULA F75".to_string(),
                release_number: 1,
                usage_page: 0xff00,
                usage: 1,
                is_known: true,
                device_name: "AULA F75".to_string(),
                usb_parent: "1-1".to_string(),
                phys: "usb-00:00/input1".to_string(),
            },
            DeviceInfo {
                vendor_id: 0x1234,
                product_id: 0x5678,
                path: std::path::PathBuf::from("/dev/hidraw2"),
                interface_number: Some(0),
                manufacturer_string: "Other".to_string(),
                product_string: "Other Mouse".to_string(),
                release_number: 1,
                usage_page: 1,
                usage: 2,
                is_known: false,
                device_name: "Other Mouse".to_string(),
                usb_parent: "1-2".to_string(),
                phys: "usb-00:00/input2".to_string(),
            },
        ];

        // 1. Unfiltered query matches known devices (exit 0)
        assert_eq!(cmd_info(&test_devs, None, None, None), 0);

        // 2. Single filter --vid matching AULA VID (exit 0)
        assert_eq!(cmd_info(&test_devs, None, Some(0x258A), None), 0);

        // 3. Single filter --vid matching non-existent VID yields no match (exit 1)
        assert_eq!(cmd_info(&test_devs, None, Some(0x9999), None), 1);

        // 4. Single filter --pid matching AULA PID (exit 0)
        assert_eq!(cmd_info(&test_devs, None, None, Some(0x010C)), 0);

        // 5. Single filter --pid matching non-existent PID yields no match (exit 1)
        assert_eq!(cmd_info(&test_devs, None, None, Some(0x9999)), 1);

        // 6. Both --vid and --pid matching AULA (exit 0)
        assert_eq!(cmd_info(&test_devs, None, Some(0x258A), Some(0x010C)), 0);

        // 7. Both --vid and --pid where one does not match yields no match (exit 1)
        assert_eq!(cmd_info(&test_devs, None, Some(0x258A), Some(0x9999)), 1);

        // 8. Filter by path matching (exit 0)
        assert_eq!(cmd_info(&test_devs, Some("/dev/hidraw1"), None, None), 0);

        // 9. Filter by non-existent path yields no match (exit 1)
        assert_eq!(cmd_info(&test_devs, Some("/dev/hidraw99"), None, None), 1);
    }
}
