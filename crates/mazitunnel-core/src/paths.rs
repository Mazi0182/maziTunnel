use std::path::{Path, PathBuf};

#[cfg(target_os = "windows")]
fn get_registry_install_dir() -> Option<PathBuf> {
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ, REG_SZ,
    };

    unsafe fn query_str_value(hkey_root: HKEY, subkey: &str, value_name: &str) -> Option<PathBuf> {
        let subkey_w: Vec<u16> = subkey.encode_utf16().chain(Some(0)).collect();
        let value_w: Vec<u16> = value_name.encode_utf16().chain(Some(0)).collect();

        let mut hkey: HKEY = std::mem::zeroed();
        let status = RegOpenKeyExW(hkey_root, subkey_w.as_ptr(), 0, KEY_READ, &mut hkey);
        if status != 0 || hkey.is_null() {
            return None;
        }

        let mut val_type: u32 = 0;
        let mut data_size: u32 = 0;
        let status = RegQueryValueExW(
            hkey,
            value_w.as_ptr(),
            std::ptr::null_mut(),
            &mut val_type,
            std::ptr::null_mut(),
            &mut data_size,
        );

        if status != 0 || val_type != REG_SZ || data_size == 0 {
            RegCloseKey(hkey);
            return None;
        }

        let mut buf = vec![0u8; data_size as usize];
        let status = RegQueryValueExW(
            hkey,
            value_w.as_ptr(),
            std::ptr::null_mut(),
            &mut val_type,
            buf.as_mut_ptr(),
            &mut data_size,
        );
        RegCloseKey(hkey);

        if status != 0 {
            return None;
        }

        let u16_len = (data_size as usize) / 2;
        let u16_slice = std::slice::from_raw_parts(buf.as_ptr() as *const u16, u16_len);
        let end = u16_slice.iter().position(|&c| c == 0).unwrap_or(u16_slice.len());
        let str_val = String::from_utf16_lossy(&u16_slice[..end]).trim().to_string();
        if !str_val.is_empty() {
            Some(PathBuf::from(str_val))
        } else {
            None
        }
    }

    unsafe {
        // 1. Once HKCU\Software\maziTunnel altindaki "InstallLocation" degerini oku
        if let Some(dir) = query_str_value(HKEY_CURRENT_USER, r"Software\maziTunnel", "InstallLocation") {
            return Some(dir);
        }
        // 2. Yoksa HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\maziTunnel altindaki degeri oku
        if let Some(dir) = query_str_value(
            HKEY_CURRENT_USER,
            r"Software\Microsoft\Windows\CurrentVersion\Uninstall\maziTunnel",
            "InstallLocation",
        ) {
            return Some(dir);
        }
    }

    None
}

#[cfg(target_os = "windows")]
fn get_windows_client_dir() -> PathBuf {
    // 1. Calisan Exe Dizini Kontrolu:
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            if exe_dir.join("client_config.json").exists()
                || exe_dir.join("config.json").exists()
                || exe_dir.join("Uninstall.exe").exists()
            {
                return exe_dir.to_path_buf();
            }
        }
    }

    // 2. Registry Kurulum Konumu Kontrolu (Portable dahil):
    if let Some(reg_dir) = get_registry_install_dir() {
        if reg_dir.exists() || std::fs::create_dir_all(&reg_dir).is_ok() {
            return reg_dir;
        }
    }

    // 3. Varsayilan Fallback: %LOCALAPPDATA%\maziTunnel
    if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
        return PathBuf::from(local_app_data).join("maziTunnel");
    }
    PathBuf::from(r"C:\Users\Default\AppData\Local\maziTunnel")
}

pub fn get_client_data_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        get_windows_client_dir()
    }
    #[cfg(not(target_os = "windows"))]
    {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            if !xdg.trim().is_empty() {
                return PathBuf::from(xdg).join("mazitunnel");
            }
        }
        if let Ok(home) = std::env::var("HOME") {
            if !home.trim().is_empty() {
                return PathBuf::from(home).join(".config").join("mazitunnel");
            }
        }
        PathBuf::from("/tmp/mazitunnel")
    }
}

pub fn get_client_install_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        get_windows_client_dir()
    }
    #[cfg(not(target_os = "windows"))]
    {
        if let Ok(home) = std::env::var("HOME") {
            if !home.trim().is_empty() {
                return PathBuf::from(home).join(".local").join("share").join("mazitunnel");
            }
        }
        PathBuf::from("/opt/mazitunnel")
    }
}

pub fn get_client_config_path() -> PathBuf {
    get_client_data_dir().join("client_config.json")
}

pub fn get_client_log_path() -> PathBuf {
    get_client_data_dir().join("client.log")
}

pub fn get_server_config_path(custom: Option<&str>) -> PathBuf {
    if let Some(custom_path) = custom {
        if !custom_path.trim().is_empty() {
            return PathBuf::from(custom_path.trim());
        }
    }

    if let Ok(env_path) = std::env::var("MAZITUNNEL_CONFIG_PATH") {
        if !env_path.trim().is_empty() {
            return PathBuf::from(env_path.trim());
        }
    }

    let default_server_config = Path::new("/mazideveloper/mazitunnel/config/server_config.json");
    default_server_config.to_path_buf()
}

pub fn normalize_local_domain(addr: &str) -> String {
    let clean = addr.trim();
    if clean.is_empty() {
        return "baglanti.local".to_string();
    }

    if clean == "127.0.0.1" || clean.eq_ignore_ascii_case("localhost") {
        return clean.to_string();
    }

    let lower = clean.to_ascii_lowercase();
    if lower.ends_with(".localhost") {
        let prefix = &clean[..clean.len() - 10];
        if prefix.to_ascii_lowercase().ends_with(".local") {
            return prefix.to_string();
        }
        return format!("{}.local", prefix);
    }

    if lower.ends_with(".local") {
        return clean.to_string();
    }

    if !clean.contains('.') {
        return format!("{}.local", clean);
    }

    clean.to_string()
}
