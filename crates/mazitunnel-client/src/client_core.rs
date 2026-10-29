use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::{mpsc, watch};
use tracing::{debug, error, info, warn};

use mazitunnel_core::crypto::{
    compute_auth_hmac, derive_credentials_from_key, derive_key,
    derive_session_key, normalize_server_key, PacketCipher,
};
use mazitunnel_core::hwid::get_client_identity;
use mazitunnel_core::nat_traversal::{
    discover_stun_endpoint, punch_udp_client_encrypted, rendezvous_client_full, StunEndpoint,
};
use mazitunnel_core::paths::get_client_config_path;
use mazitunnel_core::platform::{
    disable_udp_connreset_raw, prevent_windows_background_throttling, tune_udp_socket,
};
use mazitunnel_core::protocol::{
    AuthRequest, AuthResponse, Cmd, Frame, KcpEngine, Role,
};

use crate::local_resolver::LocalDomainInterceptor;
use crate::updater::{check_for_updates, UpdateManifest, APP_VERSION, DEFAULT_UPDATE_URL};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct SavedServer {
    pub name: String,
    pub code: String,
    pub admin_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientConfig {
    pub server_name: String,
    pub server_code: String,
    pub admin_key: String,
    #[serde(default, alias = "servers")]
    pub saved_servers: Vec<SavedServer>,
    pub active_server_idx: usize,
    pub local_game_port: u16,
    pub local_ssh_port: u16,
    pub theme: String,
    pub last_seen_version: String,
    pub update_url: String,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            server_name: "maziTunnel".to_string(),
            server_code: "".to_string(),
            admin_key: "".to_string(),
            saved_servers: Vec::new(),
            active_server_idx: 0,
            local_game_port: 25565,
            local_ssh_port: 22,
            theme: "dark".to_string(),
            last_seen_version: "".to_string(),
            update_url: DEFAULT_UPDATE_URL.to_string(),
        }
    }
}

impl ClientConfig {
    fn extract_servers_from_value(val: &serde_json::Value, out: &mut Vec<SavedServer>) -> bool {
        let mut added = false;
        let servers_array = val
            .get("saved_servers")
            .or_else(|| val.get("servers"))
            .and_then(|v| v.as_array());

        if let Some(arr) = servers_array {
            for item in arr {
                let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let code_raw = item.get("code").and_then(|v| v.as_str()).unwrap_or("");
                let admin_key = item.get("admin_key").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let norm = normalize_server_key(code_raw);
                if !norm.is_empty() && norm != "A1B2-C3D4-E5F6" {
                    out.push(SavedServer {
                        name: if name.is_empty() { "maziTunnel".to_string() } else { name },
                        code: norm,
                        admin_key,
                    });
                    added = true;
                }
            }
        } else if let Some(code_raw) = val.get("server_code").and_then(|v| v.as_str()) {
            let norm = normalize_server_key(code_raw);
            if !norm.is_empty() && norm != "A1B2-C3D4-E5F6" {
                let name = val.get("server_name").and_then(|v| v.as_str()).unwrap_or("maziTunnel").to_string();
                let admin_key = val.get("admin_key").and_then(|v| v.as_str()).unwrap_or("").to_string();
                out.push(SavedServer {
                    name,
                    code: norm,
                    admin_key,
                });
                added = true;
            }
        }
        added
    }

    pub fn load() -> Self {
        let primary_path = get_client_config_path();
        let primary_existed = primary_path.exists();
        let mut cfg = Self::default();
        let mut migrated = false;

        if primary_existed {
            if let Ok(content) = std::fs::read_to_string(&primary_path) {
                if let Ok(parsed) = serde_json::from_str::<ClientConfig>(&content) {
                    cfg = parsed;
                } else if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                    if Self::extract_servers_from_value(&val, &mut cfg.saved_servers) {
                        migrated = true;
                    }
                    if let Some(t) = val.get("theme").and_then(|v| v.as_str()) {
                        cfg.theme = t.to_string();
                    }
                    if let Some(v) = val.get("last_seen_version").and_then(|v| v.as_str()) {
                        cfg.last_seen_version = v.to_string();
                    }
                }
            }
        }

        let mut fallback_paths = Vec::new();
        if let Some(parent) = primary_path.parent() {
            fallback_paths.push(parent.join("config.json"));
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(exe_dir) = exe.parent() {
                let portable_path = exe_dir.join("client_config.json");
                if portable_path != primary_path {
                    fallback_paths.push(portable_path);
                }
            }
        }
        #[cfg(target_os = "windows")]
        {
            if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
                let lad_dir = PathBuf::from(local_app_data).join("maziTunnel");
                let lad_cfg = lad_dir.join("client_config.json");
                if lad_cfg != primary_path {
                    fallback_paths.push(lad_cfg);
                }
                let lad_old = lad_dir.join("config.json");
                if lad_old != primary_path {
                    fallback_paths.push(lad_old);
                }
            }
            fallback_paths.push(PathBuf::from(r"C:\ProgramData\P2PTunnel\client_config.json"));
        }

        if cfg.saved_servers.is_empty() {
            for fb_path in fallback_paths {
                if fb_path.exists() {
                    if let Ok(content) = std::fs::read_to_string(&fb_path) {
                        if let Ok(parsed) = serde_json::from_str::<ClientConfig>(&content) {
                            if !parsed.saved_servers.is_empty() {
                                for s in parsed.saved_servers {
                                    let norm = normalize_server_key(&s.code);
                                    if !norm.is_empty() && norm != "A1B2-C3D4-E5F6" {
                                        cfg.saved_servers.push(SavedServer {
                                            name: if s.name.is_empty() { "maziTunnel".to_string() } else { s.name },
                                            code: norm,
                                            admin_key: s.admin_key,
                                        });
                                        migrated = true;
                                    }
                                }
                            } else if !parsed.server_code.trim().is_empty() {
                                let norm = normalize_server_key(&parsed.server_code);
                                if !norm.is_empty() && norm != "A1B2-C3D4-E5F6" {
                                    cfg.saved_servers.push(SavedServer {
                                        name: if parsed.server_name.is_empty() { "maziTunnel".to_string() } else { parsed.server_name },
                                        code: norm,
                                        admin_key: parsed.admin_key,
                                    });
                                    migrated = true;
                                }
                            }
                            if cfg.theme.is_empty() && !parsed.theme.is_empty() {
                                cfg.theme = parsed.theme;
                            }
                            if cfg.last_seen_version.is_empty() && !parsed.last_seen_version.is_empty() {
                                cfg.last_seen_version = parsed.last_seen_version;
                            }
                        } else if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                            if Self::extract_servers_from_value(&val, &mut cfg.saved_servers) {
                                migrated = true;
                            }
                        }
                    }
                    if !cfg.saved_servers.is_empty() {
                        break;
                    }
                }
            }
        }

        if cfg.saved_servers.is_empty() {
            let norm_sc = normalize_server_key(&cfg.server_code);
            if !norm_sc.is_empty() && norm_sc != "A1B2-C3D4-E5F6" {
                cfg.saved_servers.push(SavedServer {
                    name: if cfg.server_name.is_empty() {
                        "maziTunnel".to_string()
                    } else {
                        cfg.server_name.clone()
                    },
                    code: norm_sc,
                    admin_key: cfg.admin_key.clone(),
                });
                migrated = true;
            }
        }

        let mut seen = HashSet::new();
        let mut cleaned = Vec::new();
        for mut s in cfg.saved_servers {
            s.code = normalize_server_key(&s.code);
            if s.code.is_empty() || s.code == "A1B2-C3D4-E5F6" {
                migrated = true;
                continue;
            }
            if seen.insert(s.code.to_uppercase()) {
                cleaned.push(s);
            } else {
                migrated = true;
            }
        }
        cfg.saved_servers = cleaned;

        if !cfg.saved_servers.is_empty() {
            let norm_curr = normalize_server_key(&cfg.server_code);
            if let Some(pos) = cfg.saved_servers.iter().position(|s| s.code.eq_ignore_ascii_case(&norm_curr)) {
                cfg.active_server_idx = pos;
            } else if cfg.active_server_idx >= cfg.saved_servers.len() {
                cfg.active_server_idx = 0;
            }
            let active = &cfg.saved_servers[cfg.active_server_idx];
            if cfg.server_name != active.name || cfg.server_code != active.code || cfg.admin_key != active.admin_key {
                cfg.server_name = active.name.clone();
                cfg.server_code = active.code.clone();
                cfg.admin_key = active.admin_key.clone();
                migrated = true;
            }
        } else {
            cfg.active_server_idx = 0;
        }

        if !primary_existed || migrated {
            let _ = cfg.save();
        }

        cfg
    }

    pub fn save(&self) -> Result<()> {
        let path = get_client_config_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let json = serde_json::to_string_pretty(self)
            .context("Yapilandirma JSON serilestirilemedi")?;

        let temp_path = path.with_extension("json.tmp");
        if std::fs::write(&temp_path, json.as_bytes()).is_ok() {
            if std::fs::rename(&temp_path, &path).is_err() {
                let _ = std::fs::write(&path, json.as_bytes());
                let _ = std::fs::remove_file(&temp_path);
            }
        } else {
            let _ = std::fs::write(&path, json.as_bytes());
        }

        if let Ok(exe) = std::env::current_exe() {
            if let Some(exe_dir) = exe.parent() {
                let portable_path = exe_dir.join("client_config.json");
                if portable_path != path && portable_path.exists() {
                    let _ = std::fs::write(&portable_path, json.as_bytes());
                }
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TunnelState {
    Disconnected,
    Signaling,
    Punching(u32),
    Authenticating,
    Connected {
        role: Role,
        rtt_ms: f32,
        address: String,
        server_name: String,
    },
    Reconnecting(u32),
    Error(String),
}

pub struct ClientSharedState {
    pub state: Mutex<TunnelState>,
    pub ping_ms_x10: AtomicU32,
    pub active_address: Mutex<String>,
    pub is_connected: AtomicBool,
    pub is_connecting: AtomicBool,
    pub available_update: Mutex<Option<UpdateManifest>>,
    pub last_error: Mutex<Option<String>>,
    pub bytes_sent: AtomicU64,
    pub bytes_recv: AtomicU64,
}

impl Default for ClientSharedState {
    fn default() -> Self {
        Self::new()
    }
}

impl ClientSharedState {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(TunnelState::Disconnected),
            ping_ms_x10: AtomicU32::new(0),
            active_address: Mutex::new("baglanti.local".to_string()),
            is_connected: AtomicBool::new(false),
            is_connecting: AtomicBool::new(false),
            available_update: Mutex::new(None),
            last_error: Mutex::new(None),
            bytes_sent: AtomicU64::new(0),
            bytes_recv: AtomicU64::new(0),
        }
    }

    pub fn get_state(&self) -> TunnelState {
        self.state
            .lock()
            .map(|s| s.clone())
            .unwrap_or(TunnelState::Disconnected)
    }

    pub fn set_state(&self, new_state: TunnelState) {
        if let Ok(mut lock) = self.state.lock() {
            *lock = new_state;
        }
    }

    pub fn is_connected(&self) -> bool {
        self.is_connected.load(Ordering::Relaxed)
    }

    pub fn is_connecting(&self) -> bool {
        self.is_connecting.load(Ordering::Relaxed)
    }

    pub fn ping_ms(&self) -> f32 {
        let val = self.ping_ms_x10.load(Ordering::Relaxed);
        (val as f32) / 10.0
    }

    pub fn set_ping_ms(&self, ms: f32) {
        let val = (ms.max(0.0) * 10.0) as u32;
        self.ping_ms_x10.store(val, Ordering::Relaxed);
    }

    pub fn get_active_address(&self) -> String {
        self.active_address
            .lock()
            .map(|a| a.clone())
            .unwrap_or_else(|_| "baglanti.local".to_string())
    }

    pub fn set_active_address(&self, addr: &str) {
        if let Ok(mut lock) = self.active_address.lock() {
            *lock = addr.to_string();
        }
    }

    pub fn get_last_error(&self) -> Option<String> {
        self.last_error.lock().ok().and_then(|e| e.clone())
    }

    pub fn set_last_error(&self, err: Option<String>) {
        if let Ok(mut lock) = self.last_error.lock() {
            *lock = err;
        }
    }

    pub fn add_bytes_sent(&self, count: u64) {
        self.bytes_sent.fetch_add(count, Ordering::Relaxed);
    }

    pub fn add_bytes_recv(&self, count: u64) {
        self.bytes_recv.fetch_add(count, Ordering::Relaxed);
    }
}

pub struct MaziTunnelClientCore {
    pub shared_state: Arc<ClientSharedState>,
    pub local_resolver: Arc<LocalDomainInterceptor>,
    pub config: Arc<Mutex<ClientConfig>>,
    cancel_tx: Mutex<Option<watch::Sender<bool>>>,
}

impl MaziTunnelClientCore {
    pub fn new(config: ClientConfig) -> Self {
        let shared_state = Arc::new(ClientSharedState::new());
        let local_resolver = Arc::new(LocalDomainInterceptor::new(
            "baglanti.local",
            &config.server_name,
            config.local_game_port,
        ));

        Self {
            shared_state,
            local_resolver,
            config: Arc::new(Mutex::new(config)),
            cancel_tx: Mutex::new(None),
        }
    }

    pub fn start_background_workers(&self) {
        self.local_resolver.start_background_tasks();

        let shared = Arc::clone(&self.shared_state);
        let cfg_arc = Arc::clone(&self.config);

        tokio::spawn(async move {
            let update_url = {
                let lock = cfg_arc.lock().ok();
                let raw = lock
                    .map(|c| c.update_url.clone())
                    .unwrap_or_else(|| DEFAULT_UPDATE_URL.to_string());
                if raw.contains("mazitunnel_version.json") || raw.trim().is_empty() {
                    DEFAULT_UPDATE_URL.to_string()
                } else {
                    raw
                }
            };

            match check_for_updates(Some(&update_url)).await {
                Ok(Some(manifest)) => {
                    info!("Yeni surum bildirimi alindi: v{}", manifest.version);
                    if let Ok(mut lock) = shared.available_update.lock() {
                        *lock = Some(manifest.clone());
                    }

                    let temp_path = std::env::temp_dir().join("maziTunnelUpdateSetup.exe");
                    if let Err(e) = crate::updater::download_update_installer(
                        &manifest.url,
                        &temp_path,
                        manifest.sha256.as_deref(),
                    )
                    .await
                    {
                        warn!("Guncelleme paketi arka planda indirilemedi: {}", e);
                    } else {
                        info!("Guncelleme paketi basariyla indirildi: {:?}", temp_path);
                        if !shared.is_connected() {
                            let _ = crate::updater::apply_update_and_restart(&temp_path);
                        }
                    }
                }
                Ok(None) => {
                    debug!("Sistem guncel.");
                }
                Err(e) => {
                    debug!("Guncelleme kontrolu atlandi: {}", e);
                }
            }
        });
    }

    pub fn connect(&self, server_code: &str, admin_key: &str) -> Result<()> {
        self.disconnect();

        let (code_input, admin_override) = if let Some((c, a)) = server_code.split_once('#') {
            (c, a)
        } else {
            (server_code, "")
        };

        let clean_code = normalize_server_key(code_input);
        if clean_code.is_empty() {
            return Err(anyhow!("Baglanti kodu bos olamaz"));
        }

        let admin_str = if !admin_override.trim().is_empty() {
            admin_override.trim().to_string()
        } else {
            admin_key.trim().to_string()
        };

        let (cancel_sender, cancel_receiver) = watch::channel(false);
        if let Ok(mut lock) = self.cancel_tx.lock() {
            *lock = Some(cancel_sender);
        }

        let (game_port, ssh_port) = {
            let cfg = self.config.lock().map_err(|_| anyhow!("Ayar kilidi alinamadi"))?;
            (cfg.local_game_port, cfg.local_ssh_port)
        };

        let shared = Arc::clone(&self.shared_state);
        let resolver = Arc::clone(&self.local_resolver);
        let code_str = clean_code;

        tokio::spawn(async move {
            run_tunnel_session(
                shared,
                resolver,
                code_str,
                admin_str,
                game_port,
                ssh_port,
                cancel_receiver,
            )
            .await;
        });

        Ok(())
    }

    pub fn disconnect(&self) {
        if let Ok(mut lock) = self.cancel_tx.lock() {
            if let Some(tx) = lock.take() {
                let _ = tx.send(true);
            }
        }

        self.local_resolver.set_tunnel_active(false);
        self.shared_state.set_state(TunnelState::Disconnected);
        self.shared_state.is_connected.store(false, Ordering::SeqCst);
        self.shared_state.is_connecting.store(false, Ordering::SeqCst);
        self.shared_state.set_ping_ms(0.0);
        info!("Tunel baglantisi kesildi.");
    }

    pub async fn run_headless_cli(
        config_path: Option<String>,
        server_code: Option<String>,
        admin_key: Option<String>,
    ) -> Result<()> {
        info!("maziTunnel CLI Modu Baslatiliyor (v{})...", APP_VERSION);

        let mut config = if let Some(path_str) = config_path {
            let p = PathBuf::from(path_str);
            if p.exists() {
                let content = std::fs::read_to_string(&p)?;
                serde_json::from_str::<ClientConfig>(&content)?
            } else {
                ClientConfig::load()
            }
        } else {
            ClientConfig::load()
        };

        if let Some(code) = server_code {
            config.server_code = code;
        }
        if let Some(adm) = admin_key {
            config.admin_key = adm;
        }

        if config.server_code.trim().is_empty() {
            return Err(anyhow!(
                "Baglanti kodu belirtilmedi. Lutfen --code <KOD> parametresi verin."
            ));
        }

        let core = Arc::new(MaziTunnelClientCore::new(config.clone()));
        core.start_background_workers();

        info!(
            "Sunucuya baglaniliyor: {} (Oyun Portu: {})",
            config.server_code, config.local_game_port
        );
        core.connect(&config.server_code, &config.admin_key)?;

        let core_clone = Arc::clone(&core);
        tokio::spawn(async move {
            tokio::signal::ctrl_c().await.ok();
            info!("Ctrl+C sinyali alindi, kapatiliyor...");
            core_clone.disconnect();
            tokio::time::sleep(Duration::from_millis(300)).await;
            std::process::exit(0);
        });

        let mut last_state = TunnelState::Disconnected;
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let current = core.shared_state.get_state();
            if current != last_state {
                match &current {
                    TunnelState::Signaling => info!("Durum: Sinyal brokeri ile eslesiliyor..."),
                    TunnelState::Punching(attempt) => {
                        info!("Durum: CGNAT delme paketi gonderiliyor (Deneme: {})...", attempt)
                    }
                    TunnelState::Authenticating => info!("Durum: Sunucu kimligi dogrulaniyor..."),
                    TunnelState::Connected {
                        role,
                        rtt_ms,
                        address,
                        server_name,
                    } => {
                        info!(
                            "Baglanti basarili! Sunucu: {}, Rol: {:?}, RTT: {:.1}ms, Adres: {}:{}",
                            server_name, role, rtt_ms, address, config.local_game_port
                        );
                    }
                    TunnelState::Reconnecting(attempt) => {
                        warn!("Durum: Yeniden baglaniliyor (Deneme: {})...", attempt)
                    }
                    TunnelState::Error(err) => {
                        error!("Hata: {}", err);
                        break;
                    }
                    TunnelState::Disconnected => {
                        info!("Baglanti sonlandi.");
                        break;
                    }
                }
                last_state = current;
            }
        }

        Ok(())
    }
}

pub async fn run_client_tunnel_task(
    shared: Arc<ClientSharedState>,
    local_resolver: Arc<LocalDomainInterceptor>,
    server_code: String,
    admin_key: String,
    local_game_port: u16,
    local_ssh_port: u16,
    cancel_rx: watch::Receiver<bool>,
) {
    run_tunnel_session(
        shared,
        local_resolver,
        server_code,
        admin_key,
        local_game_port,
        local_ssh_port,
        cancel_rx,
    )
    .await;
}

pub async fn run_tunnel_session(
    shared: Arc<ClientSharedState>,
    local_resolver: Arc<LocalDomainInterceptor>,
    server_code: String,
    mut admin_key: String,
    local_game_port: u16,
    _local_ssh_port: u16,
    mut cancel_rx: watch::Receiver<bool>,
) {
    prevent_windows_background_throttling();

    if *cancel_rx.borrow() {
        return;
    }

    let (code_raw, admin_raw) = if let Some((c, a)) = server_code.split_once('#') {
        (c, if !a.trim().is_empty() { a.trim() } else { admin_key.as_str() })
    } else {
        (server_code.as_str(), admin_key.as_str())
    };
    if !admin_raw.trim().is_empty() {
        admin_key = admin_raw.trim().to_string();
    }
    let clean_code = normalize_server_key(code_raw);

    shared.is_connecting.store(true, Ordering::SeqCst);
    shared.is_connected.store(false, Ordering::SeqCst);
    shared.set_state(TunnelState::Signaling);
    shared.set_last_error(None);

    let (_room_id, room_secret, player_token) = match derive_credentials_from_key(&clean_code) {
        Ok(c) => c,
        Err(e) => {
            let err = format!("Gecersiz baglanti kodu: {}", e);
            shared.set_state(TunnelState::Error(err.clone()));
            shared.set_last_error(Some(err));
            shared.is_connecting.store(false, Ordering::SeqCst);
            return;
        }
    };
    let auth_token = if !admin_key.trim().is_empty() {
        admin_key.trim().to_string()
    } else {
        player_token.clone()
    };
    let room_cipher = Arc::new(PacketCipher::new(&derive_key(&room_secret), true));

    let std_sock = match socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    ) {
        Ok(s) => s,
        Err(e) => {
            let err = format!("UDP soketi olusturulamadi: {}", e);
            shared.set_state(TunnelState::Error(err.clone()));
            shared.set_last_error(Some(err));
            shared.is_connecting.store(false, Ordering::SeqCst);
            return;
        }
    };

    tune_udp_socket(&std_sock);
    disable_udp_connreset_raw(&std_sock);

    let bind_addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0);
    if let Err(e) = std_sock.bind(&bind_addr.into()) {
        let err = format!("UDP soket baglanamadi: {}", e);
        shared.set_state(TunnelState::Error(err.clone()));
        shared.set_last_error(Some(err));
        shared.is_connecting.store(false, Ordering::SeqCst);
        return;
    }

    let _ = std_sock.set_nonblocking(true);

    let udp_socket = match UdpSocket::from_std(std_sock.into()) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            let err = format!("Tokio UDP soket olusturulamadi: {}", e);
            shared.set_state(TunnelState::Error(err.clone()));
            shared.set_last_error(Some(err));
            shared.is_connecting.store(false, Ordering::SeqCst);
            return;
        }
    };

    let local_port = udp_socket.local_addr().map(|a| a.port()).unwrap_or(0);

    let lan_peer_opt = mazitunnel_core::nat_traversal::discover_lan_server(&udp_socket, &clean_code, 25566).await;

    let (peer_addr, my_public_ip) = if let Some(lan_peer) = lan_peer_opt {
        info!("Yerel agda (LAN) sunucu bulundu: {}", lan_peer);
        (lan_peer, mazitunnel_core::nat_traversal::get_local_lan_ip())
    } else {
        let my_ep = discover_stun_endpoint(&udp_socket).await.unwrap_or(StunEndpoint::new(
            Ipv4Addr::LOCALHOST,
            local_port,
            false,
        ));

        if *cancel_rx.borrow() {
            return;
        }

        let candidates = rendezvous_client_full(&clean_code, &my_ep, local_port).await;
        if candidates.is_empty() {
            let err = "Sunucu cevrimdisi veya baglanti anahtari gecersiz.".to_string();
            shared.set_state(TunnelState::Error(err.clone()));
            shared.set_last_error(Some(err));
            shared.is_connecting.store(false, Ordering::SeqCst);
            return;
        }

        if *cancel_rx.borrow() {
            return;
        }

        shared.set_state(TunnelState::Punching(1));

        let addr = match punch_udp_client_encrypted(&udp_socket, &candidates, &clean_code, 5500).await {
            Some(a) => a,
            None => candidates[0],
        };
        info!("Hedef sunucu adresi belirlendi: {}", addr);
        (addr, my_ep.ip.to_string())
    };

    if *cancel_rx.borrow() {
        return;
    }

    shared.set_state(TunnelState::Authenticating);

    let kcp = Arc::new(Mutex::new(KcpEngine::new(2026)));

    let client_nonce = {
        let mut bytes = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut bytes);
        hex::encode(bytes)
    };

    let identity = get_client_identity("4.2");
    let wire_token = if auth_token.starts_with("TOK_") || auth_token.starts_with("tok_") {
        auth_token.clone()
    } else {
        "HMAC_PROTECTED".to_string()
    };
    let auth_req = AuthRequest {
        token: wire_token,
        token_hmac: compute_auth_hmac(&auth_token, &room_secret, &client_nonce),
        nonce: client_nonce.clone(),
        client_nonce: client_nonce.clone(),
        version: "4.2".to_string(),
        hwid: identity.hwid.clone(),
        hostname: identity.hostname.clone(),
        computer_name: identity.hostname.clone(),
        username: identity.username.clone(),
        os: identity.os.clone(),
        public_ip: my_public_ip,
        mtu: 1350,
        vpn: false,
        hwid_details: identity.hwid_details.clone(),
    };

    let auth_payload = serde_json::to_vec(&auth_req).unwrap_or_default();
    let auth_frame = Frame::new(Cmd::Auth, 0, 0, Bytes::from(auth_payload));

    let mut session_cipher: Option<Arc<PacketCipher>> = None;
    let mut auth_resp: Option<AuthResponse> = None;
    let mut frame_buf = Vec::new();
    let mut recv_buf = [0u8; 4096];

    for step in 0..80 {
        if *cancel_rx.borrow() {
            return;
        }

        let now_ms = {
            let k = kcp.lock().unwrap();
            k.now_ms()
        };

        if step % 8 == 0 {
            let dgrams = {
                let mut k = kcp.lock().unwrap();
                k.send(&auth_frame.encode());
                k.flush(now_ms)
            };
            for dg in dgrams {
                let enc = room_cipher.encrypt(&dg);
                let _ = udp_socket.send_to(&enc, peer_addr).await;
            }
        }

        let retrans = {
            let mut k = kcp.lock().unwrap();
            k.update(now_ms)
        };
        for dg in retrans {
            let enc = room_cipher.encrypt(&dg);
            let _ = udp_socket.send_to(&enc, peer_addr).await;
        }

        let deadline = Instant::now() + Duration::from_millis(100);
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }

            match tokio::time::timeout(remaining, udp_socket.recv_from(&mut recv_buf)).await {
                Ok(Ok((len, src))) => {
                    let pkt = &recv_buf[..len];
                    if let Some(plain) = room_cipher.decrypt(pkt) {
                        if plain.starts_with(b"P2P_TUN_PUNCH") || plain.starts_with(b"MAZITUN_PUNCH") {
                            continue;
                        }
                        if plain.starts_with(b"P2P_TUN_OOB_PING:") {
                            continue;
                        }

                        if plain.len() >= 24 {
                            let (acks, chunks) = {
                                let mut k = kcp.lock().unwrap();
                                let now_ms = k.now_ms();
                                let a = k.input(&plain, now_ms);
                                let mut chs = Vec::new();
                                while let Some(c) = k.recv() {
                                    chs.push(c);
                                }
                                (a, chs)
                            };
                            for ack in acks {
                                let enc = room_cipher.encrypt(&ack);
                                let _ = udp_socket.send_to(&enc, src).await;
                            }
                            for c in chunks {
                                frame_buf.extend_from_slice(&c);
                            }

                            let mut done = false;
                            while let Some((f, consumed)) = Frame::decode(&frame_buf) {
                                frame_buf.drain(..consumed);
                                if f.cmd == Cmd::AuthOk {
                                    let mut parsed: AuthResponse = serde_json::from_slice(&f.payload)
                                        .unwrap_or(AuthResponse {
                                            ok: true,
                                            role: if f.target_port == 2 {
                                                "ADMIN".to_string()
                                            } else {
                                                "PLAYER".to_string()
                                            },
                                            nonce: String::new(),
                                            proof: String::new(),
                                            address: "baglanti.localhost".to_string(),
                                            port: local_game_port,
                                            reason: String::new(),
                                            server_name: String::new(),
                                        });
                                    if parsed.role.is_empty() {
                                        parsed.role = if f.target_port == 2 {
                                            "ADMIN".to_string()
                                        } else {
                                            "PLAYER".to_string()
                                        };
                                    }
                                    let s_nonce = if !parsed.nonce.is_empty() {
                                        &parsed.nonce
                                    } else {
                                        &client_nonce
                                    };
                                    let session_key = derive_session_key(
                                        &room_secret,
                                        &client_nonce,
                                        s_nonce,
                                    );
                                    let mut sc = PacketCipher::new(&derive_key(&room_secret), true);
                                    sc.set_session_key(session_key);
                                    session_cipher = Some(Arc::new(sc));
                                    auth_resp = Some(parsed);
                                    done = true;
                                    break;
                                } else if f.cmd == Cmd::AuthFail {
                                    let reason = String::from_utf8_lossy(&f.payload).to_string();
                                    let err = format!("Kimlik dogrulama reddedildi: {}", reason);
                                    shared.set_state(TunnelState::Error(err.clone()));
                                    shared.set_last_error(Some(err));
                                    shared.is_connecting.store(false, Ordering::SeqCst);
                                    return;
                                }
                            }
                            if done {
                                break;
                            }
                        }
                    }
                }
                _ => break,
            }
        }

        if session_cipher.is_some() {
            break;
        }
    }

    let session_cipher = match session_cipher {
        Some(sc) => sc,
        None => {
            let err = "Kimlik dogrulama zaman asimina ugradi.".to_string();
            shared.set_state(TunnelState::Error(err.clone()));
            shared.set_last_error(Some(err));
            shared.is_connecting.store(false, Ordering::SeqCst);
            return;
        }
    };
    let resp = auth_resp.unwrap();

    let session_start = Instant::now();
    let mut smooth_rtt_ms: f32 = 0.0;

    // Immediately send an initial P2P_TUN_OOB_PING: packet AND an P2P_TUN_OOB_MTU:1350 packet
    {
        let now_ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        let now_us = session_start.elapsed().as_micros() as u64;
        let mut ping_pkt = Vec::with_capacity(37);
        ping_pkt.extend_from_slice(b"P2P_TUN_OOB_PING:");
        ping_pkt.extend_from_slice(&now_ts.to_be_bytes());
        ping_pkt.extend_from_slice(b"MZCL");
        ping_pkt.extend_from_slice(&now_us.to_be_bytes());
        let enc_ping = session_cipher.encrypt(&ping_pkt);
        let _ = udp_socket.send_to(&enc_ping, peer_addr).await;

        let mtu_pkt = b"P2P_TUN_OOB_MTU:1350";
        let enc_mtu = session_cipher.encrypt(mtu_pkt);
        let _ = udp_socket.send_to(&enc_mtu, peer_addr).await;

        let mut pmtu_pong = Vec::with_capacity(1297);
        pmtu_pong.extend_from_slice(b"P2P_TUN_OOB_PONG:");
        pmtu_pong.extend_from_slice(&now_ts.to_be_bytes());
        pmtu_pong.extend_from_slice(&[b'M'; 1272]);
        let enc_pmtu = session_cipher.encrypt(&pmtu_pong);
        let _ = udp_socket.send_to(&enc_pmtu, peer_addr).await;
    }

    let assigned_role = Role::from_str_role(&resp.role).unwrap_or(Role::Player);
    let assigned_port = if resp.port > 0 {
        resp.port
    } else {
        local_game_port
    };
    let assigned_address = if resp.address.is_empty() {
        "baglanti.localhost".to_string()
    } else {
        resp.address.clone()
    };

    let srv_name = if !resp.server_name.trim().is_empty() {
        resp.server_name.trim().to_string()
    } else {
        resp.address.trim_end_matches(".local").to_string()
    };
    local_resolver.set_server_name(&srv_name);

    shared.set_state(TunnelState::Connected {
        role: assigned_role,
        rtt_ms: 1.0,
        address: assigned_address.clone(),
        server_name: srv_name,
    });
    shared.is_connected.store(true, Ordering::SeqCst);
    shared.is_connecting.store(false, Ordering::SeqCst);
    shared.set_active_address(&assigned_address);

    local_resolver.set_active_domain(&assigned_address);
    local_resolver.set_game_port(assigned_port);
    local_resolver.set_tunnel_active(true);

    info!(
        "Tunel baglantisi aktif! Rol: {:?}, Adres: {}:{}",
        assigned_role, assigned_address, assigned_port
    );

    let (outbound_tx, mut outbound_rx) = mpsc::channel::<Frame>(512);
    let active_streams: Arc<Mutex<HashMap<u32, mpsc::Sender<Bytes>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let next_stream_id = Arc::new(AtomicU32::new(100));

    let mut ports_to_listen = vec![(assigned_port, assigned_port)];
    if assigned_role == Role::Admin {
        for &(lp, rp) in &[(2222, 22), (80, 80), (8080, 8080), (8443, 8443), (9090, 9090)] {
            if lp != assigned_port {
                ports_to_listen.push((lp, rp));
            }
        }
    }

    let mut listener_handles = Vec::new();
    for (listen_port, target_port) in ports_to_listen {
        let otx = outbound_tx.clone();
        let astreams = Arc::clone(&active_streams);
        let nsid = Arc::clone(&next_stream_id);
        let handle = tokio::spawn(async move {
            if let Ok(listener) = TcpListener::bind(format!("127.0.0.1:{}", listen_port)).await {
                info!("Yerel TCP dinleyici baslatildi: 127.0.0.1:{} -> :{}", listen_port, target_port);
                while let Ok((socket, _)) = listener.accept().await {
                    let _ = socket.set_nodelay(true);
                    let sid = nsid.fetch_add(1, Ordering::SeqCst);
                    let (stx, mut srx) = mpsc::channel::<Bytes>(128);
                    if let Ok(mut lock) = astreams.lock() {
                        lock.insert(sid, stx);
                    }
                    let _ = otx
                        .send(Frame::new(Cmd::Connect, sid, target_port, Bytes::new()))
                        .await;

                    let (mut rd, mut wr) = socket.into_split();
                    let otx_clone = otx.clone();
                    let astreams_clone = Arc::clone(&astreams);

                    tokio::spawn(async move {
                        while let Some(data) = srx.recv().await {
                            if wr.write_all(&data).await.is_err() {
                                break;
                            }
                        }
                        let _ = wr.shutdown().await;
                    });

                    tokio::spawn(async move {
                        let mut buf = [0u8; 4096];
                        loop {
                            match rd.read(&mut buf).await {
                                Ok(0) => break,
                                Ok(n) => {
                                    let mut failed = false;
                                    for chunk in buf[..n].chunks(1200) {
                                        let f = Frame::new(
                                            Cmd::Data,
                                            sid,
                                            target_port,
                                            Bytes::copy_from_slice(chunk),
                                        );
                                        if otx_clone.send(f).await.is_err() {
                                            failed = true;
                                            break;
                                        }
                                    }
                                    if failed {
                                        break;
                                    }
                                }
                                Err(_) => break,
                            }
                        }
                        if let Ok(mut lock) = astreams_clone.lock() {
                            lock.remove(&sid);
                        }
                        let _ = otx_clone
                            .send(Frame::new(Cmd::Close, sid, target_port, Bytes::new()))
                            .await;
                    });
                }
            } else {
                warn!("Yerel TCP portu (127.0.0.1:{}) dinlenemedi", listen_port);
            }
        });
        listener_handles.push(handle);
    }

    let mut udp_buf = [0u8; 4096];
    let mut oob_ping_interval = tokio::time::interval(Duration::from_millis(1000));
    let mut kcp_tick_interval = tokio::time::interval(Duration::from_millis(5));
    let mut last_rx = Instant::now();

    loop {
        if *cancel_rx.borrow() {
            info!("Mux dongusu iptal sinyali aldi.");
            let disc_frame = Frame::new(Cmd::Disconnect, 0, 0, Bytes::new());
            let dgrams = {
                let mut k = kcp.lock().unwrap();
                k.send_frame(&disc_frame)
            };
            for dg in dgrams {
                let enc = session_cipher.encrypt(&dg);
                let _ = udp_socket.send_to(&enc, peer_addr).await;
            }
            break;
        }

        tokio::select! {
            _ = cancel_rx.changed() => {
                if *cancel_rx.borrow() {
                    let disc_frame = Frame::new(Cmd::Disconnect, 0, 0, Bytes::new());
                    let dgrams = {
                        let mut k = kcp.lock().unwrap();
                        k.send_frame(&disc_frame)
                    };
                    for dg in dgrams {
                        let enc = session_cipher.encrypt(&dg);
                        let _ = udp_socket.send_to(&enc, peer_addr).await;
                    }
                    break;
                }
            }

            Some(frame) = outbound_rx.recv() => {
                let mut frames = vec![frame];
                while frames.len() < 16 {
                    match outbound_rx.try_recv() {
                        Ok(f) => frames.push(f),
                        Err(_) => break,
                    }
                }
                let dgrams = {
                    let mut k = kcp.lock().unwrap();
                    for f in frames {
                        k.send(&f.encode());
                    }
                    let now = k.now_ms();
                    k.flush(now)
                };
                for dg in dgrams {
                    let enc = session_cipher.encrypt(&dg);
                    shared.add_bytes_sent(enc.len() as u64);
                    let _ = udp_socket.send_to(&enc, peer_addr).await;
                }
            }

            udp_res = udp_socket.recv_from(&mut udp_buf) => {
                match udp_res {
                    Ok((n, _src)) => {
                        if n > 0 {
                            last_rx = Instant::now();
                            shared.add_bytes_recv(n as u64);
                            let pkt = &udp_buf[..n];
                            let plain = session_cipher.decrypt(pkt).or_else(|| room_cipher.decrypt(pkt));
                            if let Some(plain) = plain {
                                if plain.starts_with(b"P2P_TUN_OOB_PING:") {
                                    let pong = session_cipher.encrypt([b"P2P_TUN_OOB_PONG:", &plain[b"P2P_TUN_OOB_PING:".len()..]].concat().as_slice());
                                    let _ = udp_socket.send_to(&pong, peer_addr).await;
                                    continue;
                                }
                                if plain.starts_with(b"P2P_TUN_OOB_PONG:") {
                                    let body = &plain[b"P2P_TUN_OOB_PONG:".len()..];
                                    if body.len() >= 20 && &body[8..12] == b"MZCL" {
                                        if let Ok(sent_us_bytes) = body[12..20].try_into() {
                                            let sent_us = u64::from_be_bytes(sent_us_bytes);
                                            let now_us = session_start.elapsed().as_micros() as u64;
                                            let sample_ms = ((now_us.saturating_sub(sent_us)) as f32 / 1000.0).max(0.5);
                                            smooth_rtt_ms = if smooth_rtt_ms <= 0.0 {
                                                sample_ms
                                            } else {
                                                0.7 * smooth_rtt_ms + 0.3 * sample_ms
                                            };
                                            shared.set_ping_ms(smooth_rtt_ms);
                                            continue;
                                        }
                                    }
                                    let now_ts = SystemTime::now()
                                        .duration_since(UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_secs_f64();
                                    if body.len() >= 8 {
                                        if let Ok(arr) = body[0..8].try_into() {
                                            let sent_ts = f64::from_be_bytes(arr);
                                            if sent_ts > 1_000_000_000.0 && sent_ts <= now_ts + 5.0 {
                                                let rtt = ((now_ts - sent_ts) * 1000.0).max(0.5) as f32;
                                                smooth_rtt_ms = if smooth_rtt_ms <= 0.0 {
                                                    rtt
                                                } else {
                                                    0.7 * smooth_rtt_ms + 0.3 * rtt
                                                };
                                                shared.set_ping_ms(smooth_rtt_ms);
                                                continue;
                                            }
                                        }
                                    }
                                    if let Ok(s) = std::str::from_utf8(body) {
                                        if let Ok(sent_ts) = s.parse::<f64>() {
                                            let rtt = ((now_ts - sent_ts) * 1000.0).max(0.5) as f32;
                                            smooth_rtt_ms = if smooth_rtt_ms <= 0.0 {
                                                rtt
                                            } else {
                                                0.7 * smooth_rtt_ms + 0.3 * rtt
                                            };
                                            shared.set_ping_ms(smooth_rtt_ms);
                                        }
                                    }
                                    continue;
                                }
                                if plain.starts_with(b"P2P_TUN_OOB_MTU:") {
                                    continue;
                                }
                                if plain.starts_with(b"P2P_TUN_PUNCH") || plain.starts_with(b"MAZITUN_PUNCH") {
                                    continue;
                                }
                                if plain.len() >= 24 {
                                    let (acks, chunks) = {
                                        let mut k = kcp.lock().unwrap();
                                        let now_ms = k.now_ms();
                                        let a = k.input(&plain, now_ms);
                                        let mut chs = Vec::new();
                                        while let Some(c) = k.recv() {
                                            chs.push(c);
                                        }
                                        (a, chs)
                                    };
                                    for ack in acks {
                                        let enc = session_cipher.encrypt(&ack);
                                        let _ = udp_socket.send_to(&enc, peer_addr).await;
                                    }
                                    for c in chunks {
                                        frame_buf.extend_from_slice(&c);
                                    }

                                    while let Some((f, consumed)) = Frame::decode(&frame_buf) {
                                        frame_buf.drain(..consumed);
                                        match f.cmd {
                                            Cmd::Data => {
                                                let maybe_tx = active_streams.lock().unwrap().get(&f.stream_id).cloned();
                                                if let Some(tx) = maybe_tx {
                                                    let _ = tx.send(f.payload).await;
                                                }
                                            }
                                            Cmd::Close => {
                                                active_streams.lock().unwrap().remove(&f.stream_id);
                                            }
                                            Cmd::Disconnect => {
                                                info!("Sunucu baglantiyi sonlandirdi.");
                                                break;
                                            }
                                            Cmd::Ping => {
                                                let pong = Frame::new(Cmd::Pong, f.stream_id, 0, f.payload);
                                                let _ = outbound_tx.try_send(pong);
                                            }
                                            Cmd::Pong => {}
                                            _ => {}
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        debug!("UDP alma hatasi: {}", e);
                    }
                }
            }

            _ = kcp_tick_interval.tick() => {
                let dgrams = {
                    let mut k = kcp.lock().unwrap();
                    let now = k.now_ms();
                    k.update(now)
                };
                for dg in dgrams {
                    let enc = session_cipher.encrypt(&dg);
                    shared.add_bytes_sent(enc.len() as u64);
                    let _ = udp_socket.send_to(&enc, peer_addr).await;
                }
            }

            _ = oob_ping_interval.tick() => {
                if last_rx.elapsed() > Duration::from_secs(25) {
                    warn!("Sunucu yanit vermiyor (25sn zaman asimi), baglanti kapatiliyor.");
                    break;
                }
                let now_ts = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs_f64();
                let now_us = session_start.elapsed().as_micros() as u64;
                let mut ping_pkt = Vec::with_capacity(37);
                ping_pkt.extend_from_slice(b"P2P_TUN_OOB_PING:");
                ping_pkt.extend_from_slice(&now_ts.to_be_bytes());
                ping_pkt.extend_from_slice(b"MZCL");
                ping_pkt.extend_from_slice(&now_us.to_be_bytes());
                let enc = session_cipher.encrypt(&ping_pkt);
                let _ = udp_socket.send_to(&enc, peer_addr).await;
            }
        }
    }

    local_resolver.set_tunnel_active(false);
    shared.is_connected.store(false, Ordering::SeqCst);
    shared.is_connecting.store(false, Ordering::SeqCst);
    shared.set_ping_ms(0.0);
    shared.set_state(TunnelState::Disconnected);
    for h in listener_handles {
        h.abort();
    }
}
