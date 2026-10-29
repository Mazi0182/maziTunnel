#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[cfg(windows)]
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE},
    System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    },
    System::Registry::{
        RegCloseKey, RegDeleteKeyW, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER,
        KEY_READ, REG_SZ,
    },
    System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE},
    UI::Shell::{SHChangeNotify, SHGetFolderPathW},
    UI::WindowsAndMessaging::{MessageBoxW, MB_ICONINFORMATION, MB_OK},
};

#[cfg(windows)]
fn get_special_folder(csidl: i32) -> Option<PathBuf> {
    unsafe {
        let mut buf = [0u16; 260];
        let hr = SHGetFolderPathW(
            0 as _,
            csidl,
            0 as _,
            0,
            buf.as_mut_ptr(),
        );
        if hr == 0 {
            let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            if len > 0 {
                return Some(PathBuf::from(String::from_utf16_lossy(&buf[..len])));
            }
        }
    }
    None
}

#[cfg(not(windows))]
fn get_special_folder(_csidl: i32) -> Option<PathBuf> {
    None
}

fn kill_process_by_name(target_name: &str) {
    // 1. ToolHelp32 API ile kapatma
    #[cfg(windows)]
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot != -1 as isize as HANDLE {
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;

            if Process32FirstW(snapshot, &mut entry) != 0 {
                loop {
                    let exe_name = String::from_utf16_lossy(
                        &entry.szExeFile[..entry
                            .szExeFile
                            .iter()
                            .position(|&c| c == 0)
                            .unwrap_or(entry.szExeFile.len())],
                    );

                    if exe_name.eq_ignore_ascii_case(target_name) {
                        let proc_handle = OpenProcess(PROCESS_TERMINATE, 0, entry.th32ProcessID);
                        if !proc_handle.is_null() {
                            TerminateProcess(proc_handle, 1);
                            CloseHandle(proc_handle);
                        }
                    }

                    if Process32NextW(snapshot, &mut entry) == 0 {
                        break;
                    }
                }
            }
            CloseHandle(snapshot);
        }
    }

    // 2. Garanti taskkill cagrisi (konsol penceresi acmadan)
    let mut kill_cmd = Command::new("taskkill");
    kill_cmd.args(["/F", "/IM", target_name]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        kill_cmd.creation_flags(0x08000000);
    }
    let _ = kill_cmd.output();
}

#[cfg(windows)]
fn get_registry_install_dir() -> Option<PathBuf> {
    unsafe fn query_str(hkey_root: HKEY, subkey: &str, val_name: &str) -> Option<PathBuf> {
        let subkey_w: Vec<u16> = format!("{}\0", subkey).encode_utf16().collect();
        let val_w: Vec<u16> = format!("{}\0", val_name).encode_utf16().collect();
        let mut hkey: HKEY = std::mem::zeroed();
        if RegOpenKeyExW(hkey_root, subkey_w.as_ptr(), 0, KEY_READ, &mut hkey) == 0 && !hkey.is_null() {
            let mut val_type: u32 = 0;
            let mut data_size: u32 = 0;
            if RegQueryValueExW(hkey, val_w.as_ptr(), std::ptr::null_mut(), &mut val_type, std::ptr::null_mut(), &mut data_size) == 0
                && val_type == REG_SZ
                && data_size > 0
            {
                let mut buf = vec![0u8; data_size as usize];
                if RegQueryValueExW(hkey, val_w.as_ptr(), std::ptr::null_mut(), &mut val_type, buf.as_mut_ptr(), &mut data_size) == 0 {
                    RegCloseKey(hkey);
                    let u16_len = (data_size as usize) / 2;
                    let u16_slice = std::slice::from_raw_parts(buf.as_ptr() as *const u16, u16_len);
                    let end = u16_slice.iter().position(|&c| c == 0).unwrap_or(u16_slice.len());
                    let s = String::from_utf16_lossy(&u16_slice[..end]).trim().to_string();
                    if !s.is_empty() {
                        return Some(PathBuf::from(s));
                    }
                }
            }
            RegCloseKey(hkey);
        }
        None
    }

    unsafe {
        if let Some(p) = query_str(HKEY_CURRENT_USER, r"Software\maziTunnel", "InstallLocation") {
            return Some(p);
        }
        if let Some(p) = query_str(
            HKEY_CURRENT_USER,
            r"Software\Microsoft\Windows\CurrentVersion\Uninstall\maziTunnel",
            "InstallLocation",
        ) {
            return Some(p);
        }
    }
    None
}

#[cfg(not(windows))]
fn get_registry_install_dir() -> Option<PathBuf> {
    None
}

fn remove_registry_entry() {
    #[cfg(windows)]
    unsafe {
        let subkey_uninstall = "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\maziTunnel\0"
            .encode_utf16()
            .collect::<Vec<u16>>();
        RegDeleteKeyW(HKEY_CURRENT_USER, subkey_uninstall.as_ptr());

        let subkey_mazitunnel = "Software\\maziTunnel\0"
            .encode_utf16()
            .collect::<Vec<u16>>();
        RegDeleteKeyW(HKEY_CURRENT_USER, subkey_mazitunnel.as_ptr());
    }
}

fn show_info(title: &str, message: &str) {
    #[cfg(windows)]
    unsafe {
        let w_title: Vec<u16> = format!("{}\0", title).encode_utf16().collect();
        let w_msg: Vec<u16> = format!("{}\0", message).encode_utf16().collect();
        MessageBoxW(
            0 as _,
            w_msg.as_ptr(),
            w_title.as_ptr(),
            MB_OK | MB_ICONINFORMATION,
        );
    }
    #[cfg(not(windows))]
    {
        println!("[{}] {}", title, message);
    }
}

const KNOWN_APP_FILES: &[&str] = &[
    "maziTunnel.exe",
    "maziTunnel.exe.old",
    "maziTunnel_update.exe",
    "app.ico",
    "wintun.dll",
    "client_config.json",
    "config.json",
    ".device_id",
    "mazitunnel.log",
    "client.log",
];

fn main() {
    let args: Vec<String> = env::args().collect();
    let is_silent = args.iter().any(|a| a == "--silent" || a == "/S" || a == "-s");

    // 1. Acik olan maziTunnel istemcisini sonlandir
    kill_process_by_name("maziTunnel.exe");

    // 2. Masaustu kisayollarini sil
    if let Some(desktop_dir) = get_special_folder(0x0010) {
        let _ = fs::remove_file(desktop_dir.join("maziTunnel.lnk"));
    }
    if let Ok(userprofile) = env::var("USERPROFILE") {
        let _ = fs::remove_file(PathBuf::from(&userprofile).join("Desktop").join("maziTunnel.lnk"));
    }
    if let Ok(public_dir) = env::var("PUBLIC") {
        let _ = fs::remove_file(PathBuf::from(&public_dir).join("Desktop").join("maziTunnel.lnk"));
    }
    if let Some(common_desktop) = get_special_folder(0x0019) {
        let _ = fs::remove_file(common_desktop.join("maziTunnel.lnk"));
    }

    // 3. Baslat Menusu kisayollarini ve klasorlerini sil
    if let Some(programs_dir) = get_special_folder(0x0002) {
        let _ = fs::remove_file(programs_dir.join("maziTunnel.lnk"));
        let _ = fs::remove_dir_all(programs_dir.join("maziTunnel"));
    }
    if let Ok(appdata) = env::var("APPDATA") {
        let user_programs = PathBuf::from(&appdata)
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs");
        let _ = fs::remove_file(user_programs.join("maziTunnel.lnk"));
        let _ = fs::remove_dir_all(user_programs.join("maziTunnel"));
    }
    if let Ok(programdata) = env::var("PROGRAMDATA") {
        let allusers_programs = PathBuf::from(&programdata)
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs");
        let _ = fs::remove_file(allusers_programs.join("maziTunnel.lnk"));
        let _ = fs::remove_dir_all(allusers_programs.join("maziTunnel"));
    }
    if let Some(common_programs) = get_special_folder(0x0017) {
        let _ = fs::remove_file(common_programs.join("maziTunnel.lnk"));
        let _ = fs::remove_dir_all(common_programs.join("maziTunnel"));
    }

    // 4. Windows Shell onbellegini yenile
    #[cfg(windows)]
    unsafe {
        SHChangeNotify(
            0x08000000, // SHCNE_ASSOCCHANGED
            0x0000,     // SHCNF_IDLIST
            std::ptr::null(),
            std::ptr::null(),
        );
    }

    // 5. Registry kayitlari silinmeden ONCE kurulum dizinlerini topla
    let mut target_dirs: Vec<PathBuf> = Vec::new();

    // Registry'den kayitli InstallLocation
    if let Some(reg_dir) = get_registry_install_dir() {
        if !target_dirs.iter().any(|d| d == &reg_dir) {
            target_dirs.push(reg_dir);
        }
    }

    // Calisan Uninstall.exe dizini
    if let Ok(cur_exe) = env::current_exe() {
        if let Some(parent) = cur_exe.parent() {
            let is_distribution_or_dev_dir = parent.join("maziTunnel_Setup.exe").exists()
                || parent.join("maziTunnel_Setup.msi").exists()
                || parent.join("maziTunnel_Client_Portable.exe").exists()
                || parent.join("Cargo.toml").exists();

            if !is_distribution_or_dev_dir
                && (parent.join("maziTunnel.exe").exists() || parent.join("client_config.json").exists())
            {
                let p_buf = parent.to_path_buf();
                if !target_dirs.iter().any(|d| d == &p_buf) {
                    target_dirs.push(p_buf);
                }
            }
        }
    }

    // Varsayilan %LOCALAPPDATA%\maziTunnel
    if let Ok(local_appdata) = env::var("LOCALAPPDATA") {
        let default_dir = PathBuf::from(local_appdata).join("maziTunnel");
        if !target_dirs.iter().any(|d| d == &default_dir) {
            target_dirs.push(default_dir);
        }
    }

    // 6. Registry kayitlarini sil
    remove_registry_entry();

    // 7. Tespit edilen kurulum dizinlerindeki yalnizca bilinen uygulama dosyalarini tek tek sil
    for dir in &target_dirs {
        if dir.exists() {
            for file_name in KNOWN_APP_FILES {
                let file_path = dir.join(file_name);
                if file_path.exists() {
                    let _ = fs::remove_file(&file_path);
                }
            }
        }
    }

    // 8. Sessiz mod degilse kullaniciya bilgi kutusunu goster
    if !is_silent {
        show_info(
            "maziTunnel Kaldırma Aracı",
            "maziTunnel bilgisayarınızdan başarıyla kaldırıldı.",
        );
    }

    // 9. Calisma dizinini gecici dizine tasiyarak klasor kilidini kaldir
    let _ = env::set_current_dir(env::temp_dir());

    // 10. Arka planda gecikmeli komutla Uninstall.exe'yi sil ve bos kalirsa dizini kaldir (/s olmadan)
    for dir in target_dirs {
        if dir.exists() {
            #[cfg(windows)]
            let uninstall_exe = dir.join("Uninstall.exe");
            #[cfg(windows)]
            let cmd_args = format!(
                "/C ping 127.0.0.1 -n 2 >nul & del /f /q \"{}\" >nul 2>&1 & rmdir \"{}\" >nul 2>&1 & if exist \"{}\" (ping 127.0.0.1 -n 2 >nul & del /f /q \"{}\" >nul 2>&1 & rmdir \"{}\" >nul 2>&1)",
                uninstall_exe.to_string_lossy(),
                dir.to_string_lossy(),
                dir.to_string_lossy(),
                uninstall_exe.to_string_lossy(),
                dir.to_string_lossy()
            );

            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                let mut cmd = Command::new("cmd.exe");
                cmd.raw_arg(&cmd_args);
                cmd.current_dir(env::temp_dir());
                cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
                let _ = cmd.spawn();
            }
        }
    }
}
