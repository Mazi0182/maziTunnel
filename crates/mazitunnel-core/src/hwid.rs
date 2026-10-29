use std::fs;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::paths::get_client_data_dir;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientIdentity {
    pub hwid: String,
    pub hostname: String,
    pub username: String,
    pub os: String,
    pub version: String,
    pub hwid_details: serde_json::Value,
}

pub fn get_client_identity(version: &str) -> ClientIdentity {
    let hostname = get_hostname();
    let username = get_username();
    let os_name = get_os_info();

    let (mb_uuid, mb_product) = get_motherboard_info();
    let machine_guid = get_machine_guid();
    let vol_serial = get_volume_serial();
    let cpu_id = get_cpu_id();
    let mac = get_mac_address();
    let device_id = get_or_create_device_id();

    let has_hw_ids = !machine_guid.is_empty()
        || !mb_uuid.is_empty()
        || !vol_serial.is_empty()
        || !cpu_id.is_empty();
    let stable_seed = if has_hw_ids {
        hostname.to_uppercase()
    } else {
        get_or_create_device_id()
    };

    let fingerprint = format!(
        "UUID:{}|GUID:{}|VOL:{}|CPU:{}|MB:{}|HOST:{}",
        mb_uuid, machine_guid, vol_serial, cpu_id, mb_product, stable_seed
    );

    let mut hasher = Sha256::new();
    hasher.update(fingerprint.as_bytes());
    let h = hex::encode_upper(hasher.finalize());

    // 24 characters (21+ characters, starts with HWID-)
    let hwid = format!("HWID-{}-{}-{}-{}", &h[0..4], &h[4..8], &h[8..12], &h[12..16]);

    let hwid_details = serde_json::json!({
        "motherboard_uuid": mb_uuid,
        "motherboard_product": mb_product,
        "machine_guid": machine_guid,
        "volume_serial": vol_serial,
        "cpu_id": cpu_id,
        "mac": mac,
        "device_id": device_id,
    });

    ClientIdentity {
        hwid,
        hostname,
        username,
        os: os_name,
        version: version.to_string(),
        hwid_details,
    }
}

fn get_hostname() -> String {
    if let Ok(name) = std::env::var("COMPUTERNAME") {
        if !name.trim().is_empty() {
            return name.trim().to_string();
        }
    }
    if let Ok(name) = std::env::var("HOSTNAME") {
        if !name.trim().is_empty() {
            return name.trim().to_string();
        }
    }
    if let Ok(content) = fs::read_to_string("/etc/hostname") {
        let name = content.trim();
        if !name.is_empty() {
            return name.to_string();
        }
    }
    "PC".to_string()
}

fn get_username() -> String {
    if let Ok(u) = std::env::var("USERNAME") {
        if !u.trim().is_empty() {
            return u.trim().to_string();
        }
    }
    if let Ok(u) = std::env::var("USER") {
        if !u.trim().is_empty() {
            return u.trim().to_string();
        }
    }
    "Player".to_string()
}

fn get_os_info() -> String {
    #[cfg(target_os = "windows")]
    {
        format!("Windows ({})", std::env::consts::ARCH)
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(os_rel) = fs::read_to_string("/etc/os-release") {
            for line in os_rel.lines() {
                if let Some(rest) = line.strip_prefix("PRETTY_NAME=") {
                    return rest.trim_matches('"').to_string();
                }
            }
        }
        format!("Linux ({})", std::env::consts::ARCH)
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        format!("{} ({})", std::env::consts::OS, std::env::consts::ARCH)
    }
}

fn get_motherboard_info() -> (String, String) {
    #[cfg(target_os = "linux")]
    {
        let uuid = fs::read_to_string("/sys/class/dmi/id/product_uuid")
            .unwrap_or_default()
            .trim()
            .to_string();
        let vendor = fs::read_to_string("/sys/class/dmi/id/board_vendor")
            .unwrap_or_default()
            .trim()
            .to_string();
        let name = fs::read_to_string("/sys/class/dmi/id/board_name")
            .unwrap_or_default()
            .trim()
            .to_string();
        let product = format!("{} {}", vendor, name).trim().to_string();
        (uuid, product)
    }
    #[cfg(not(target_os = "linux"))]
    {
        ("".to_string(), "".to_string())
    }
}

fn get_machine_guid() -> String {
    #[cfg(target_os = "windows")]
    {
        extern "system" {
            fn RegOpenKeyExA(
                hKey: isize,
                lpSubKey: *const u8,
                ulOptions: u32,
                samDesired: u32,
                phkResult: *mut isize,
            ) -> i32;
            fn RegQueryValueExA(
                hKey: isize,
                lpValueName: *const u8,
                lpReserved: *mut u32,
                lpType: *mut u32,
                lpData: *mut u8,
                lpcbData: *mut u32,
            ) -> i32;
            fn RegCloseKey(hKey: isize) -> i32;
        }

        const HKEY_LOCAL_MACHINE: isize = 0x80000002u32 as isize;
        const KEY_READ: u32 = 0x20019;
        const KEY_WOW64_64KEY: u32 = 0x0100;

        let subkey = b"SOFTWARE\\Microsoft\\Cryptography\0";
        let value = b"MachineGuid\0";
        let mut hkey: isize = 0;

        unsafe {
            if RegOpenKeyExA(
                HKEY_LOCAL_MACHINE,
                subkey.as_ptr(),
                0,
                KEY_READ | KEY_WOW64_64KEY,
                &mut hkey,
            ) == 0
            {
                let mut buf = [0u8; 128];
                let mut len = buf.len() as u32;
                let res = RegQueryValueExA(
                    hkey,
                    value.as_ptr(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    buf.as_mut_ptr(),
                    &mut len,
                );
                RegCloseKey(hkey);
                if res == 0 && len > 0 {
                    let s = String::from_utf8_lossy(&buf[..(len as usize).saturating_sub(1)])
                        .trim()
                        .to_string();
                    if !s.is_empty() {
                        return s;
                    }
                }
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(mid) = fs::read_to_string("/etc/machine-id") {
            let s = mid.trim();
            if !s.is_empty() {
                return s.to_string();
            }
        }
        if let Ok(mid) = fs::read_to_string("/var/lib/dbus/machine-id") {
            let s = mid.trim();
            if !s.is_empty() {
                return s.to_string();
            }
        }
    }
    "".to_string()
}

fn get_volume_serial() -> String {
    "".to_string()
}

fn get_cpu_id() -> String {
    if let Ok(cpu) = std::env::var("PROCESSOR_IDENTIFIER") {
        if !cpu.trim().is_empty() {
            return cpu.trim().to_string();
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(cpuinfo) = fs::read_to_string("/proc/cpuinfo") {
            for line in cpuinfo.lines() {
                if line.starts_with("model name") {
                    if let Some((_, model)) = line.split_once(':') {
                        return model.trim().to_string();
                    }
                }
            }
        }
    }
    std::env::consts::ARCH.to_string()
}

fn get_mac_address() -> String {
    #[cfg(target_os = "linux")]
    {
        if let Ok(entries) = fs::read_dir("/sys/class/net") {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name == "lo" {
                    continue;
                }
                let addr_file = entry.path().join("address");
                if let Ok(addr) = fs::read_to_string(addr_file) {
                    let s = addr.trim();
                    if !s.is_empty() && s != "00:00:00:00:00:00" {
                        return s.replace(':', "").to_uppercase();
                    }
                }
            }
        }
    }
    "".to_string()
}

fn get_or_create_device_id() -> String {
    let data_dir = get_client_data_dir();
    let dev_file = data_dir.join(".device_id");

    if let Ok(content) = fs::read_to_string(&dev_file) {
        let clean = content.trim();
        if clean.len() >= 32 {
            return clean.to_string();
        }
    }

    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    let new_id = hex::encode(bytes);

    let _ = fs::create_dir_all(&data_dir);
    let _ = fs::write(&dev_file, &new_id);

    new_id
}
