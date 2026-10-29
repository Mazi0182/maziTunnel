use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=../../assets/setup_icon.ico");
    println!("cargo:rerun-if-changed=../../assets/uninstall_icon.ico");
    println!("cargo:rerun-if-changed=payload_client.bin");
    println!("cargo:rerun-if-changed=payload_uninstall.bin");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" {
        return;
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    let setup_ico = manifest_dir
        .join("../../assets/setup_icon.ico")
        .canonicalize()
        .unwrap_or_else(|_| manifest_dir.join("../../assets/setup_icon.ico"));
    let uninstall_ico = manifest_dir
        .join("../../assets/uninstall_icon.ico")
        .canonicalize()
        .unwrap_or_else(|_| manifest_dir.join("../../assets/uninstall_icon.ico"));

    let windres_bin = if Path::new("/usr/bin/llvm-windres-19").exists() {
        "/usr/bin/llvm-windres-19"
    } else {
        "llvm-windres"
    };

    // 1. setup_res.rc -> setup_res.o
    let setup_rc = out_dir.join("setup_res.rc");
    let setup_obj = out_dir.join("setup_res.o");
    let setup_ico_str = setup_ico
        .to_string_lossy()
        .replace('\\', "/");
    let setup_ico_clean = setup_ico_str
        .trim_start_matches("//?/")
        .trim_start_matches(r"\\?\");
    let setup_rc_content = format!("1 ICON \"{}\"\n", setup_ico_clean);

    if fs::write(&setup_rc, setup_rc_content).is_ok() {
        let status = Command::new(windres_bin)
            .args([
                "--input",
                setup_rc.to_str().unwrap(),
                "--output",
                setup_obj.to_str().unwrap(),
            ])
            .status();
        if let Ok(s) = status {
            if s.success() && setup_obj.exists() {
                println!(
                    "cargo:rustc-link-arg-bin=mazitunnel-setup={}",
                    setup_obj.display()
                );
            }
        }
    }

    // 2. uninst_res.rc -> uninst_res.o
    let uninst_rc = out_dir.join("uninst_res.rc");
    let uninst_obj = out_dir.join("uninst_res.o");
    let uninstall_ico_str = uninstall_ico
        .to_string_lossy()
        .replace('\\', "/");
    let uninstall_ico_clean = uninstall_ico_str
        .trim_start_matches("//?/")
        .trim_start_matches(r"\\?\");
    let uninst_rc_content = format!("1 ICON \"{}\"\n", uninstall_ico_clean);

    if fs::write(&uninst_rc, uninst_rc_content).is_ok() {
        let status = Command::new(windres_bin)
            .args([
                "--input",
                uninst_rc.to_str().unwrap(),
                "--output",
                uninst_obj.to_str().unwrap(),
            ])
            .status();
        if let Ok(s) = status {
            if s.success() && uninst_obj.exists() {
                println!(
                    "cargo:rustc-link-arg-bin=mazitunnel-uninstall={}",
                    uninst_obj.display()
                );
            }
        }
    }
}
