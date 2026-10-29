use std::fs::File;
use std::path::Path;
use tracing::{error, info};

mod acl;
mod ban_manager;
mod daemon;
mod history_manager;
mod logger;
mod panel_api;

pub use acl::ServerAcl;
pub use ban_manager::{BanEntry, BanManager};
pub use daemon::{resolve_server_config_path, ServerDaemon};
pub use history_manager::{HistoryManager, SessionHistoryEntry};
pub use logger::ServerLogger;
pub use panel_api::{ActiveClientSnapshot, PanelApiContext, PanelCommand, ServerConfig};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args: Vec<String> = std::env::args().collect();

    let mut custom_config: Option<String> = None;
    let mut admin_key_override: Option<String> = None;
    let mut panel_mode_override: Option<String> = None;
    let mut is_explicit_cli_start = false;
    let mut verbose = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--config" | "-c" => {
                if i + 1 < args.len() {
                    custom_config = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "--admin-key" | "-a" => {
                if i + 1 < args.len() {
                    admin_key_override = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "--mazipanel" => {
                panel_mode_override = Some("mazipanel".to_string());
                is_explicit_cli_start = true;
            }
            "--standalone" => {
                panel_mode_override = Some("standalone".to_string());
                is_explicit_cli_start = true;
            }
            "--verbose" | "-v" => {
                verbose = true;
            }
            "--help" | "-h" => {
                print_help();
                return Ok(());
            }
            _ => {
                if !args[i].starts_with('-') && custom_config.is_none() {
                    custom_config = Some(args[i].clone());
                }
            }
        }
        i += 1;
    }

    let config_path = resolve_server_config_path(custom_config.as_deref());
    let config_dir = config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();

    let _ = std::fs::create_dir_all(&config_dir);

    // 1. Check .stopped_by_panel flag (standby loop for systemd + maziPanel)
    let stopped_flag = config_dir.join(".stopped_by_panel");
    if stopped_flag.exists() {
        if !is_explicit_cli_start {
            info!(
                "Sunucu maziPanel tarafindan durdurulmus (.stopped_by_panel mevcut). Servis beklemede kaliyor."
            );
            while stopped_flag.exists() {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
            info!(".stopped_by_panel kaldirildi, maziTunnel sunucu servisi baslatiliyor...");
        } else {
            let _ = std::fs::remove_file(&stopped_flag);
            info!("Manuel baslatma algilandi; .stopped_by_panel kaldirildi.");
        }
    }

    // 2. Single-instance flock guard
    let _lock_file = match acquire_instance_lock(&config_dir) {
        Some(f) => f,
        None => {
            error!(
                "Baska bir maziTunnel sunucu servisi zaten calisiyor (server.lock kilitli)."
            );
            std::process::exit(1);
        }
    };

    info!("maziTunnel Sunucu Servisi baslatiliyor...");
    info!("Yapilandirma dosyasi: {:?}", config_path);

    let daemon = ServerDaemon::init(
        custom_config.as_deref(),
        admin_key_override.as_deref(),
        panel_mode_override.as_deref(),
        verbose,
    )
    .await?;

    daemon.run().await
}

fn acquire_instance_lock(config_dir: &Path) -> Option<File> {
    let lock_path = config_dir.join("server.lock");
    let file = File::create(&lock_path).ok()?;

    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let fd = file.as_raw_fd();
        let ret = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
        if ret != 0 {
            return None;
        }
    }

    Some(file)
}

fn print_help() {
    println!("maziTunnel Sunucu Servisi (Saf Rust)");
    println!();
    println!("Kullanim:");
    println!("  mazitunnel-server [SECENEKLER]");
    println!();
    println!("Secenekler:");
    println!("  -c, --config <YOL>       Ozel server_config.json yolu");
    println!("  -a, --admin-key <SIFRE>  Yonetici sifresini gecici olarak ez");
    println!("  --mazipanel              maziPanel entegrasyon modunda calis");
    println!("  --standalone             Bagimsiz web konsol modunda calis");
    println!("  -v, --verbose            Konsola detayli teknik loglari da yaz");
    println!("  -h, --help               Bu yardim ekranini goster");
}
