use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=../../assets/app.ico");
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" {
        return;
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let icon_path = manifest_dir.join("../../assets/app.ico");
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let rc_path = out_dir.join("app.rc");
    let obj_path = out_dir.join("app_res.o");

    let icon_clean = icon_path
        .canonicalize()
        .unwrap_or(icon_path)
        .to_string_lossy()
        .replace('\\', "/");

    let rc_content = format!(
        r#"1 ICON "{}"
1 VERSIONINFO
FILEVERSION 2,0,0,0
PRODUCTVERSION 2,0,0,0
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "CompanyName", "maziTunnel"
      VALUE "FileDescription", "maziTunnel"
      VALUE "FileVersion", "2.0.0"
      VALUE "InternalName", "maziTunnel"
      VALUE "OriginalFilename", "maziTunnel.exe"
      VALUE "ProductName", "maziTunnel"
      VALUE "ProductVersion", "2.0.0"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        icon_clean
    );

    if fs::write(&rc_path, rc_content).is_ok() {
        let compilers = [
            "/usr/bin/llvm-windres-19",
            "llvm-windres-19",
            "llvm-windres",
            "x86_64-w64-mingw32-windres",
            "windres",
        ];

        let mut compiled = false;
        for compiler in &compilers {
            let status = Command::new(compiler)
                .args([
                    rc_path.to_str().unwrap(),
                    "-O",
                    "coff",
                    "-o",
                    obj_path.to_str().unwrap(),
                ])
                .status();

            if let Ok(s) = status {
                if s.success() && obj_path.exists() {
                    compiled = true;
                    break;
                }
            }
        }

        if compiled {
            println!("cargo:rustc-link-arg-bin=maziTunnel={}", obj_path.display());
        }
    }
}
