#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

pub mod canvas;
pub mod client_core;
pub mod gui_app;
pub mod local_resolver;
pub mod updater;

use std::env;
use std::sync::Arc;

use client_core::{ClientConfig, MaziTunnelClientCore};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = env::args().collect();

    let mut use_cli = false;
    let mut config_path: Option<String> = None;
    let mut server_code: Option<String> = None;
    let mut admin_key: Option<String> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--cli" | "-c" => use_cli = true,
            "--config" if i + 1 < args.len() => {
                config_path = Some(args[i + 1].clone());
                i += 1;
            }
            "--admin" | "-a" if i + 1 < args.len() => {
                admin_key = Some(args[i + 1].clone());
                i += 1;
            }
            "--updated" | "--silent" => {}
            val if !val.starts_with('-') && server_code.is_none() => {
                server_code = Some(val.to_string());
                use_cli = true;
            }
            _ => {}
        }
        i += 1;
    }

    #[cfg(not(windows))]
    let _ = use_cli;
    #[cfg(not(windows))]
    let use_cli = true;

    if use_cli {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
            )
            .init();

        return MaziTunnelClientCore::run_headless_cli(config_path, server_code, admin_key).await;
    }

    let is_updated = args.iter().any(|a| a == "--updated");

    let cfg = ClientConfig::load();
    let core = Arc::new(MaziTunnelClientCore::new(cfg));
    core.start_background_workers();

    gui_app::run_gui(core, is_updated);

    Ok(())
}
