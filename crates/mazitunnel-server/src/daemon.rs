use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use bytes::Bytes;
use mazitunnel_core::crypto::{
    compute_server_proof, derive_credentials_from_key, derive_key, derive_session_key,
    generate_server_key, normalize_server_key, PacketCipher,
};
use mazitunnel_core::nat_traversal::{
    discover_public_endpoint, SignalingBroker, DEFAULT_BROKERS,
};
use mazitunnel_core::paths::normalize_local_domain;
use mazitunnel_core::protocol::{
    AuthRequest, AuthResponse, Cmd, Frame, KcpEngine, Role,
    LEGACY_PUNCH_ACK, LEGACY_PUNCH_SYN, PUNCH_ACK, PUNCH_SYN,
};
use rand::RngCore;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, RwLock};
use tracing::{info, warn};

use crate::acl::ServerAcl;
use crate::ban_manager::BanManager;
use crate::history_manager::HistoryManager;
use crate::panel_api::{
    run_panel_api, save_config_atomic, ActiveClientSnapshot, PanelApiContext, PanelCommand,
    ServerConfig,
};

pub struct ClientSession {
    #[allow(dead_code)]
    pub addr: SocketAddr,
    pub kcp: KcpEngine,
    pub frame_buf: Vec<u8>,
    pub cipher: PacketCipher,
    pub role: Role,
    pub authenticated: bool,
    pub username: String,
    pub hostname: String,
    pub hwid: String,
    pub hwid_details: serde_json::Value,
    pub os: String,
    pub version: String,
    pub virtual_ip: String,
    pub joined_at: Instant,
    pub last_activity: Instant,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub ping_ms: u32,
    pub tcp_streams: HashMap<u32, mpsc::Sender<Bytes>>,
    pub has_joined_game: bool,
}

pub struct ServerDaemon {
    pub config_path: PathBuf,
    pub config: Arc<RwLock<ServerConfig>>,
    pub acl: Arc<ServerAcl>,
    pub ban_manager: Arc<std::sync::RwLock<BanManager>>,
    pub history_manager: Arc<RwLock<HistoryManager>>,
    pub public_endpoint: Arc<RwLock<Option<SocketAddr>>>,
    pub active_clients: Arc<RwLock<HashMap<SocketAddr, ActiveClientSnapshot>>>,
    pub client_candidates: Arc<RwLock<HashMap<SocketAddr, Instant>>>,
    pub start_time: Instant,
    pub udp_sock: Arc<UdpSocket>,
    pub cmd_rx: mpsc::Receiver<PanelCommand>,
    pub cmd_tx: mpsc::Sender<PanelCommand>,
    pub logger: Arc<crate::logger::ServerLogger>,
    pub failed_auths: Arc<tokio::sync::Mutex<HashMap<IpAddr, Vec<Instant>>>>,
    pub blocked_auth_ips: Arc<tokio::sync::Mutex<HashMap<IpAddr, Instant>>>,
}

impl ServerDaemon {
    pub async fn init(
        custom_config_path: Option<&str>,
        admin_key_override: Option<&str>,
        panel_mode_override: Option<&str>,
        verbose: bool,
    ) -> Result<Self> {
        let config_path = resolve_server_config_path(custom_config_path);
        let config_dir = config_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();

        let mut config = load_or_create_server_config(&config_path)?;

        if let Some(admin_override) = admin_key_override {
            if !admin_override.trim().is_empty() {
                config.admin_secret = admin_override.trim().to_string();
                save_config_atomic(&config_path, &config);
            }
        }

        if let Some(pm_override) = panel_mode_override {
            config.panel_mode = pm_override.trim().to_string();
            save_config_atomic(&config_path, &config);
        }

        ensure_virtual_ip_subnet();

        let logger = Arc::new(crate::logger::ServerLogger::new(config_dir.clone(), verbose));
        let ban_manager = Arc::new(std::sync::RwLock::new(BanManager::new(config_dir.clone())));
        let history_manager = Arc::new(RwLock::new(HistoryManager::new(config_dir.clone())));

        let acl = Arc::new(ServerAcl::new(
            &config.player_token,
            &config.admin_secret,
            config.game_port,
            &config.connection_address,
            ban_manager.clone(),
        ));

        let local_udp_addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, config.server_udp_port);
        let std_sock = bind_udp_socket(local_udp_addr)?;
        let udp_sock = Arc::new(UdpSocket::from_std(std_sock)?);

        let (cmd_tx, cmd_rx) = mpsc::channel(64);
        let failed_auths = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
        let blocked_auth_ips = Arc::new(tokio::sync::Mutex::new(HashMap::new()));

        Ok(Self {
            config_path,
            config: Arc::new(RwLock::new(config)),
            acl,
            ban_manager,
            history_manager,
            public_endpoint: Arc::new(RwLock::new(None)),
            active_clients: Arc::new(RwLock::new(HashMap::new())),
            client_candidates: Arc::new(RwLock::new(HashMap::new())),
            start_time: Instant::now(),
            udp_sock,
            cmd_rx,
            cmd_tx,
            logger,
            failed_auths,
            blocked_auth_ips,
        })
    }

    pub async fn run(mut self) -> Result<()> {
        let mut stun_addrs: Vec<SocketAddr> = Vec::new();
        for host in &["stun.l.google.com:19302", "stun1.l.google.com:19302", "stun.cloudflare.com:3478"] {
            if let Ok(mut iter) = tokio::net::lookup_host(host).await {
                if let Some(addr) = iter.next() {
                    if !stun_addrs.contains(&addr) {
                        stun_addrs.push(addr);
                    }
                }
            }
        }
        if stun_addrs.is_empty() {
            if let Ok(addr) = "74.125.250.129:19302".parse::<SocketAddr>() {
                stun_addrs.push(addr);
            }
        }

        let (pub_ep, is_sym) = match discover_public_endpoint(&self.udp_sock).await {
            Some((ep, sym)) => (Some(ep), sym),
            None => {
                let local_fallback = SocketAddr::V4(SocketAddrV4::new(
                    get_local_lan_ip(),
                    self.config.read().await.server_udp_port,
                ));
                (Some(local_fallback), false)
            }
        };

        if let Some(ep) = pub_ep {
            info!("Dış Ağ Public Endpoint çözüldü (STUN): {} (Simetrik: {})", ep, is_sym);
            self.logger.log_stun(&ep.to_string(), is_sym);
            *self.public_endpoint.write().await = Some(ep);
        }

        self.print_startup_banner().await;

        let panel_ctx = Arc::new(PanelApiContext {
            config_path: self.config_path.clone(),
            config: self.config.clone(),
            acl: self.acl.clone(),
            ban_manager: self.ban_manager.clone(),
            history_manager: self.history_manager.clone(),
            active_clients: self.active_clients.clone(),
            public_endpoint: self.public_endpoint.clone(),
            start_time: self.start_time,
            cmd_tx: self.cmd_tx.clone(),
            logger: self.logger.clone(),
        });

        let web_port = self.config.read().await.web_port;
        tokio::spawn(run_panel_api(panel_ctx, web_port));

        let mut signaling_handle = self.spawn_signaling_broker().await;

        let sessions: Arc<RwLock<HashMap<SocketAddr, ClientSession>>> =
            Arc::new(RwLock::new(HashMap::new()));

        let mut rx_buf = [0u8; 4096];
        let mut kcp_interval = tokio::time::interval(Duration::from_millis(10));
        let mut cleanup_interval = tokio::time::interval(Duration::from_secs(5));
        let mut stun_interval = tokio::time::interval(Duration::from_secs(5));
        let mut stun_idx: usize = 0;
        let mut punch_interval = tokio::time::interval(Duration::from_millis(120));
        let mut punch_tick: u64 = 0;

        loop {
            tokio::select! {
                // 1. Process panel control commands (Kick / Reload / Drop)
                Some(cmd) = self.cmd_rx.recv() => {
                    match cmd {
                        PanelCommand::Kick { addr, hwid } => {
                            self.handle_panel_kick(&addr, &hwid, &sessions).await;
                        }
                        PanelCommand::ReloadSignaling => {
                            info!("Signaling broker parametreleri güncelleniyor...");
                            signaling_handle.abort();
                            signaling_handle = self.spawn_signaling_broker().await;
                        }
                        PanelCommand::DropPlayerSessions => {
                            self.handle_drop_player_sessions(&sessions).await;
                        }
                    }
                }

                // 2. Inbound UDP packets from clients
                recv_res = self.udp_sock.recv_from(&mut rx_buf) => {
                    match recv_res {
                        Ok((len, peer_addr)) => {
                            let allow_wan = std::env::var("MAZITUNNEL_QUARANTINE")
                                .map(|v| v != "1" && !v.eq_ignore_ascii_case("true"))
                                .unwrap_or(true);
                            if !allow_wan && !mazitunnel_core::nat_traversal::is_lan_addr(peer_addr.ip()) {
                                continue;
                            }
                            let data = &rx_buf[..len];
                            self.handle_inbound_udp_datagram(peer_addr, data, &sessions).await;
                        }
                        Err(e) => {
                            warn!("UDP soket okuma hatası: {}", e);
                        }
                    }
                }

                // 3. KCP fast tick & retransmissions (every 10ms)
                _ = kcp_interval.tick() => {
                    self.tick_kcp_sessions(&sessions).await;
                }

                // 4. Session health check & metric snapshots (every 5s)
                _ = cleanup_interval.tick() => {
                    self.cleanup_dead_sessions(&sessions).await;
                    self.update_active_clients_snapshot(&sessions).await;
                }

                // 5. STUN keepalive & dynamic endpoint refresh (every 5s)
                _ = stun_interval.tick() => {
                    if !stun_addrs.is_empty() {
                        let (req, _) = mazitunnel_core::nat_traversal::create_stun_request();
                        let target = stun_addrs[stun_idx % stun_addrs.len()];
                        let _ = self.udp_sock.send_to(&req, target).await;
                        stun_idx = stun_idx.wrapping_add(1);
                    }
                }

                // 6. Proactive 120ms Encrypted Hole Punching
                _ = punch_interval.tick() => {
                    punch_tick = punch_tick.wrapping_add(1);
                    let mut cands = Vec::new();
                    {
                        let mut guard = self.client_candidates.write().await;
                        let now = Instant::now();
                        guard.retain(|_, exp| *exp > now);

                        let sess_guard = sessions.read().await;
                        guard.retain(|addr, _| !sess_guard.contains_key(addr));

                        for &addr in guard.keys() {
                            cands.push(addr);
                        }
                    }
                    if !cands.is_empty() {
                        let room_secret = self.config.read().await.room_secret.clone();
                        let trial_cipher = PacketCipher::new(&derive_key(&room_secret), true);
                        let syn_pkt = trial_cipher.encrypt(b"P2P_TUN_PUNCH_SYN");

                        let probe_delta = (punch_tick % 3) == 0;
                        for cand in cands {
                            let _ = self.udp_sock.send_to(&syn_pkt, cand).await;
                            if probe_delta && !mazitunnel_core::nat_traversal::is_lan_addr(cand.ip()) {
                                let base = cand.port() as i32;
                                for delta in -5..=40 {
                                    if delta == 0 {
                                        continue;
                                    }
                                    let p = base + delta;
                                    if (1024..=65535).contains(&p) {
                                        let mut dest = cand;
                                        dest.set_port(p as u16);
                                        let _ = self.udp_sock.send_to(&syn_pkt, dest).await;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    async fn handle_inbound_udp_datagram(
        &self,
        peer_addr: SocketAddr,
        data: &[u8],
        sessions: &Arc<RwLock<HashMap<SocketAddr, ClientSession>>>,
    ) {
        // 0. STUN response check
        if let Some(new_ep) = mazitunnel_core::nat_traversal::parse_stun_response(data, None) {
            let prev_ep = *self.public_endpoint.read().await;
            if prev_ep != Some(new_ep) {
                info!("Dış ağ public endpoint güncellendi (STUN): {:?} -> {}", prev_ep, new_ep);
                self.logger.log_stun(&new_ep.to_string(), false);
                *self.public_endpoint.write().await = Some(new_ep);
            }
            return;
        }

        // A. Direct UDP Punch response
        if data.starts_with(PUNCH_SYN) || data.starts_with(LEGACY_PUNCH_SYN) {
            let _ = self.udp_sock.send_to(PUNCH_ACK, peer_addr).await;
            let _ = self.udp_sock.send_to(LEGACY_PUNCH_ACK, peer_addr).await;
            return;
        }

        // B. Active session lookup (fast-path: bypasses config read & key derivation)
        let mut sessions_guard = sessions.write().await;
        if let Some(session) = sessions_guard.get_mut(&peer_addr) {
            session.rx_bytes = session.rx_bytes.saturating_add(data.len() as u64);
            session.last_activity = Instant::now();

            if let Some(decrypted) = session.cipher.decrypt(data) {
                if decrypted.starts_with(b"P2P_TUN_PUNCH_SYN") || decrypted.starts_with(b"MAZITUN_PUNCH_SYN") {
                    let _ = self
                        .udp_sock
                        .send_to(&session.cipher.encrypt(b"P2P_TUN_PUNCH_ACK"), peer_addr)
                        .await;
                    return;
                }
                if decrypted.starts_with(b"P2P_TUN_PUNCH") || decrypted.starts_with(b"MAZITUN_PUNCH") {
                    return;
                }
                if decrypted.starts_with(b"P2P_TUN_OOB_PING:") {
                    let pong = session.cipher.encrypt(
                        [b"P2P_TUN_OOB_PONG:", &decrypted[b"P2P_TUN_OOB_PING:".len()..]]
                            .concat()
                            .as_slice(),
                    );
                    let _ = self.udp_sock.send_to(&pong, peer_addr).await;
                    return;
                }
                if decrypted.starts_with(b"P2P_TUN_OOB_PONG:") {
                    let body = &decrypted[b"P2P_TUN_OOB_PONG:".len()..];
                    if let Ok(s) = std::str::from_utf8(body) {
                        if let Ok(sent_ts) = s.parse::<f64>() {
                            let now_ts = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs_f64();
                            session.ping_ms = ((now_ts - sent_ts) * 1000.0).max(0.5) as u32;
                        }
                    }
                    return;
                }

                if decrypted.len() >= 24 {
                    let now_ms = session.kcp.now_ms();
                    let acks = session.kcp.input(&decrypted, now_ms);

                    for ack in acks {
                        let enc = session.cipher.encrypt(&ack);
                        let _ = self.udp_sock.send_to(&enc, peer_addr).await;
                    }

                    while let Some(chunk) = session.kcp.recv() {
                        session.frame_buf.extend_from_slice(&chunk);
                    }

                    let mut ready_frames = Vec::new();
                    while let Some((frame, consumed)) = Frame::decode(&session.frame_buf) {
                        session.frame_buf.drain(..consumed);
                        ready_frames.push(frame);
                    }

                    drop(sessions_guard);
                    for frame in ready_frames {
                        self.process_client_frame(peer_addr, frame, sessions).await;
                    }
                }
                return;
            }

            // Decryption with active session cipher failed.
            // If packet is larger than 64 bytes, it is not a punch or handshake probe.
            if data.len() > 64 {
                return;
            }
        }
        drop(sessions_guard);

        // C. Slow-path: Handshake / Punch trial decryption with base_key
        let room_secret = self.config.read().await.room_secret.clone();
        let base_key = derive_key(&room_secret);
        let trial_cipher = PacketCipher::new(&base_key, true);

        if let Some(plain) = trial_cipher.decrypt(data) {
            if plain.starts_with(b"P2P_TUN_PUNCH_SYN") || plain.starts_with(b"MAZITUN_PUNCH_SYN") {
                let ack_pkt = trial_cipher.encrypt(b"P2P_TUN_PUNCH_ACK");
                for _ in 0..4 {
                    let _ = self.udp_sock.send_to(&ack_pkt, peer_addr).await;
                }
                return;
            }
            if plain.starts_with(b"P2P_TUN_PUNCH") || plain.starts_with(b"MAZITUN_PUNCH") {
                return;
            }
            if plain.starts_with(b"P2P_TUN_OOB_PING:") {
                let pong = trial_cipher.encrypt(
                    [b"P2P_TUN_OOB_PONG:", &plain[b"P2P_TUN_OOB_PING:".len()..]]
                        .concat()
                        .as_slice(),
                );
                let _ = self.udp_sock.send_to(&pong, peer_addr).await;
                return;
            }

            let mut sessions_guard = sessions.write().await;
            let unauth_count = sessions_guard.values().filter(|s| !s.authenticated).count();
            let total_count = sessions_guard.len();
            if unauth_count >= 64 || total_count >= 128 {
                let oldest_unauth = sessions_guard
                    .iter()
                    .filter(|(_, s)| !s.authenticated)
                    .min_by_key(|(_, s)| s.last_activity)
                    .map(|(a, _)| *a);

                if let Some(old_addr) = oldest_unauth {
                    sessions_guard.remove(&old_addr);
                    self.logger.log_detailed(
                        "SESSION_LIMIT",
                        &format!("Eski doğrulanmamış oturum temizlendi: {}", old_addr),
                    );
                } else {
                    self.logger.log_detailed(
                        "FLOOD_BLOCKED",
                        &format!(
                            "Maksimum oturum limitine ulaşıldı (128), yeni istek reddedildi: {}",
                            peer_addr
                        ),
                    );
                    return;
                }
            }

            let mut kcp = KcpEngine::new(2026);
            let now_ms = kcp.now_ms();
            let acks = kcp.input(&plain, now_ms);

            for ack in acks {
                let enc = trial_cipher.encrypt(&ack);
                let _ = self.udp_sock.send_to(&enc, peer_addr).await;
            }

            let mut frame_buf = Vec::new();
            while let Some(chunk) = kcp.recv() {
                frame_buf.extend_from_slice(&chunk);
            }

            let mut ready_frames = Vec::new();
            while let Some((frame, consumed)) = Frame::decode(&frame_buf) {
                frame_buf.drain(..consumed);
                ready_frames.push(frame);
            }

            let new_session = ClientSession {
                addr: peer_addr,
                kcp,
                frame_buf,
                cipher: trial_cipher,
                role: Role::Player,
                authenticated: false,
                username: "Bilinmeyen".to_string(),
                hostname: "-".to_string(),
                hwid: "".to_string(),
                hwid_details: serde_json::Value::Null,
                os: "Bilinmiyor".to_string(),
                version: "-".to_string(),
                virtual_ip: "".to_string(),
                joined_at: Instant::now(),
                last_activity: Instant::now(),
                rx_bytes: data.len() as u64,
                tx_bytes: 0,
                ping_ms: 0,
                tcp_streams: HashMap::new(),
                has_joined_game: false,
            };

            sessions_guard.insert(peer_addr, new_session);
            drop(sessions_guard);

            for frame in ready_frames {
                self.process_client_frame(peer_addr, frame, sessions).await;
            }
            return;
        }
    }

    async fn process_client_frame(
        &self,
        peer_addr: SocketAddr,
        frame: Frame,
        sessions: &Arc<RwLock<HashMap<SocketAddr, ClientSession>>>,
    ) {
        match frame.cmd {
            Cmd::Auth => {
                let ip = peer_addr.ip();
                let now = Instant::now();

                // Check if IP is currently rate-limited/blocked
                {
                    let mut blocked = self.blocked_auth_ips.lock().await;
                    if let Some(&until) = blocked.get(&ip) {
                        if now < until {
                            self.logger.log_detailed(
                                "AUTH_BLOCKED",
                                &format!("Bloklu IP'den Auth denemesi engellendi: {}", ip),
                            );
                            let auth_resp = AuthResponse {
                                ok: false,
                                role: String::new(),
                                nonce: String::new(),
                                proof: String::new(),
                                address: String::new(),
                                port: 0,
                                reason: "Çok fazla başarısız deneme. 60 saniye bekleyin.".to_string(),
                                server_name: String::new(),
                            };
                            let resp_payload = serde_json::to_vec(&auth_resp).unwrap_or_default();
                            let resp_frame = Frame::new(
                                Cmd::AuthFail,
                                frame.stream_id,
                                0,
                                Bytes::from(resp_payload),
                            );
                            self.send_frame_to_peer(peer_addr, resp_frame, sessions).await;
                            return;
                        } else {
                            blocked.remove(&ip);
                        }
                    }
                }

                let auth_req: AuthRequest = match serde_json::from_slice(&frame.payload) {
                    Ok(r) => r,
                    Err(e) => {
                        warn!("Geçersiz AuthRequest JSON ({:?}): {}", peer_addr, e);
                        return;
                    }
                };

                let room_secret = self.config.read().await.room_secret.clone();
                let auth_res = self.acl.authenticate(&auth_req, &room_secret, None);

                let mut sessions_guard = sessions.write().await;
                let Some(session) = sessions_guard.get_mut(&peer_addr) else {
                    return;
                };

                match auth_res {
                    Ok(role) => {
                        session.role = role;
                        session.authenticated = true;
                        session.username = auth_req.username.clone();
                        session.hostname = auth_req.hostname.clone();
                        session.hwid = auth_req.hwid.clone();
                        session.hwid_details = auth_req.hwid_details.clone();
                        session.os = auth_req.os.clone();
                        session.version = auth_req.version.clone();

                        let mut hist_mgr = self.history_manager.write().await;
                        let v_ip = hist_mgr.record_join(
                            &session.hwid,
                            &session.username,
                            &session.hostname,
                            &peer_addr.to_string(),
                            &peer_addr.ip().to_string(),
                            &session.os,
                            &session.version,
                            role.as_str(),
                            session.hwid_details.clone(),
                        );
                        session.virtual_ip = v_ip.clone();

                        let mut rand_bytes = [0u8; 16];
                        rand::thread_rng().fill_bytes(&mut rand_bytes);
                        let server_nonce = hex::encode(rand_bytes);

                        let session_key = derive_session_key(
                            &room_secret,
                            &auth_req.client_nonce,
                            &server_nonce,
                        );

                        let server_proof = compute_server_proof(
                            &room_secret,
                            &auth_req.client_nonce,
                            &server_nonce,
                        );

                        let cfg = self.config.read().await;
                        let conn_addr = cfg.connection_address.clone();
                        let g_port = cfg.game_port;

                        let srv_name = if !cfg.server_name.trim().is_empty() && cfg.server_name.trim() != "maziTunnel" {
                            cfg.server_name.trim().to_string()
                        } else {
                            let clean = conn_addr.trim_end_matches(".local").trim_end_matches(".localhost");
                            if clean.is_empty() {
                                if !cfg.server_name.trim().is_empty() {
                                    cfg.server_name.trim().to_string()
                                } else {
                                    "maziTunnel".to_string()
                                }
                            } else {
                                let mut chars = clean.chars();
                                match chars.next() {
                                    None => clean.to_string(),
                                    Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                                }
                            }
                        };

                        let auth_resp = AuthResponse {
                            ok: true,
                            role: role.as_str().to_string(),
                            nonce: server_nonce,
                            proof: server_proof,
                            address: conn_addr,
                            port: g_port,
                            reason: String::new(),
                            server_name: srv_name,
                        };

                        let resp_payload = serde_json::to_vec(&auth_resp).unwrap_or_default();
                        let resp_frame = Frame::new(
                            Cmd::AuthOk,
                            frame.stream_id,
                            role as u8 as u16,
                            Bytes::from(resp_payload),
                        );

                        session.kcp.send(&resp_frame.encode());
                        let dgrams = session.kcp.flush(session.kcp.now_ms());
                        for d in dgrams {
                            let enc = session.cipher.encrypt(&d);
                            let _ = self.udp_sock.send_to(&enc, peer_addr).await;
                        }

                        session.cipher.set_session_key(session_key);

                        info!(
                            "Oyuncu başarıyla doğrulandı: {} ({}) [{}] -> Sanal IP: {}",
                            session.username,
                            session.hwid,
                            role.as_str(),
                            session.virtual_ip
                        );

                        self.logger.log_connect(
                            &session.username,
                            role == Role::Admin,
                            &peer_addr.to_string(),
                            &session.virtual_ip,
                            &session.hwid,
                            &session.os,
                            &session.version,
                        );

                        drop(sessions_guard);
                        self.client_candidates.write().await.remove(&peer_addr);
                        self.update_active_clients_snapshot(sessions).await;
                    }
                    Err(reason) => {
                        let is_banned = reason.starts_with("BANNED:");
                        if is_banned {
                            self.logger.log_banned_attempt(
                                &auth_req.username,
                                &auth_req.hwid,
                                &peer_addr.to_string(),
                            );
                        } else {
                            self.logger.log_detailed(
                                "AUTH_FAIL",
                                &format!(
                                    "Giriş başarısız: {} ({:?}) - {}",
                                    auth_req.username, peer_addr, reason
                                ),
                            );
                        }

                        // Track failed attempts per IP
                        {
                            let mut failed = self.failed_auths.lock().await;
                            let attempts = failed.entry(ip).or_insert_with(Vec::new);
                            attempts.retain(|&t| now.duration_since(t) < Duration::from_secs(60));
                            attempts.push(now);

                            if attempts.len() > 8 {
                                let mut blocked = self.blocked_auth_ips.lock().await;
                                blocked.insert(ip, now + Duration::from_secs(60));
                                self.logger.log_detailed(
                                    "FLOOD_BLOCKED",
                                    &format!("IP 60 saniye engellendi (>8 başarısız deneme): {}", ip),
                                );
                            }
                        }

                        let auth_resp = AuthResponse {
                            ok: false,
                            role: String::new(),
                            nonce: String::new(),
                            proof: String::new(),
                            address: String::new(),
                            port: 0,
                            reason,
                            server_name: String::new(),
                        };

                        let resp_payload = serde_json::to_vec(&auth_resp).unwrap_or_default();
                        let resp_frame = Frame::new(
                            Cmd::AuthFail,
                            frame.stream_id,
                            0,
                            Bytes::from(resp_payload),
                        );

                        session.kcp.send(&resp_frame.encode());
                        let dgrams = session.kcp.flush(session.kcp.now_ms());
                        for d in dgrams {
                            let enc = session.cipher.encrypt(&d);
                            let _ = self.udp_sock.send_to(&enc, peer_addr).await;
                        }
                    }
                }
            }

            Cmd::Ping => {
                let mut sessions_guard = sessions.write().await;
                if let Some(session) = sessions_guard.get_mut(&peer_addr) {
                    if frame.payload.len() >= 8 {
                        if let Ok(ts_bytes) = frame.payload[0..8].try_into() {
                            let client_ts = u64::from_le_bytes(ts_bytes);
                            let now_epoch_ms = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis() as u64;
                            if now_epoch_ms >= client_ts {
                                session.ping_ms = (now_epoch_ms - client_ts).min(9999) as u32;
                            }
                        }
                    }

                    let pong_frame = Frame::new(
                        Cmd::Pong,
                        frame.stream_id,
                        0,
                        frame.payload,
                    );
                    let dgrams = session.kcp.send_frame(&pong_frame);
                    for d in dgrams {
                        let enc = session.cipher.encrypt(&d);
                        let _ = self.udp_sock.send_to(&enc, peer_addr).await;
                    }
                }
            }

            Cmd::Connect => {
                let target_port = frame.target_port;
                let stream_id = frame.stream_id;

                let (role, virtual_ip, authenticated, stream_count, has_joined, username) = {
                    let guard = sessions.read().await;
                    match guard.get(&peer_addr) {
                        Some(s) => (
                            s.role,
                            s.virtual_ip.clone(),
                            s.authenticated,
                            s.tcp_streams.len(),
                            s.has_joined_game,
                            s.username.clone(),
                        ),
                        None => return,
                    }
                };

                if !authenticated || !self.acl.is_port_allowed(role, target_port) {
                    warn!(
                        "Port erişimi reddedildi: {:?} -> Port {} (Rol: {:?})",
                        peer_addr,
                        target_port,
                        role
                    );
                    let close_frame = Frame::new(
                        Cmd::Close,
                        stream_id,
                        target_port,
                        Bytes::new(),
                    );
                    self.send_frame_to_peer(peer_addr, close_frame, sessions).await;
                    return;
                }

                // Player stream exhaustion protection (max 16)
                if role == Role::Player && stream_count >= 16 {
                    self.logger.log_detailed(
                        "STREAM_LIMIT",
                        &format!("Oyuncu eşzamanlı akış limiti (16) aşıldı: {:?}", peer_addr),
                    );
                    let close_frame = Frame::new(
                        Cmd::Close,
                        stream_id,
                        target_port,
                        Bytes::new(),
                    );
                    self.send_frame_to_peer(peer_addr, close_frame, sessions).await;
                    return;
                }

                if !has_joined {
                    let mut guard = sessions.write().await;
                    if let Some(s) = guard.get_mut(&peer_addr) {
                        if !s.has_joined_game {
                            s.has_joined_game = true;
                            self.logger.log_game_join(&username, &peer_addr.to_string(), stream_id, target_port);
                        }
                    }
                }

                let (tx, rx) = mpsc::channel::<Bytes>(128);
                {
                    let mut guard = sessions.write().await;
                    if let Some(s) = guard.get_mut(&peer_addr) {
                        s.tcp_streams.insert(stream_id, tx);
                    }
                }

                let sessions_clone = sessions.clone();
                let udp_sock_clone = self.udp_sock.clone();

                tokio::spawn(async move {
                    handle_tcp_bridge(
                        peer_addr,
                        stream_id,
                        target_port,
                        virtual_ip,
                        rx,
                        sessions_clone,
                        udp_sock_clone,
                    )
                    .await;
                });
            }

            Cmd::Data => {
                let stream_id = frame.stream_id;
                let target_port = frame.target_port;
                let (existing_sender, has_joined, username) = {
                    let guard = sessions.read().await;
                    match guard.get(&peer_addr) {
                        Some(s) => (s.tcp_streams.get(&stream_id).cloned(), s.has_joined_game, s.username.clone()),
                        None => (None, false, String::new()),
                    }
                };

                if !has_joined && !username.is_empty() {
                    let mut guard = sessions.write().await;
                    if let Some(s) = guard.get_mut(&peer_addr) {
                        if s.authenticated && !s.has_joined_game {
                            s.has_joined_game = true;
                            self.logger.log_game_join(&username, &peer_addr.to_string(), stream_id, target_port);
                        }
                    }
                }

                if let Some(sender) = existing_sender {
                    let _ = sender.send(frame.payload).await;
                } else {
                    let (role, virtual_ip, authenticated, stream_count) = {
                        let guard = sessions.read().await;
                        match guard.get(&peer_addr) {
                            Some(s) => (s.role, s.virtual_ip.clone(), s.authenticated, s.tcp_streams.len()),
                            None => return,
                        }
                    };

                    if !authenticated || !self.acl.is_port_allowed(role, target_port) {
                        let close_frame = Frame::new(Cmd::Close, stream_id, target_port, Bytes::new());
                        self.send_frame_to_peer(peer_addr, close_frame, sessions).await;
                        return;
                    }

                    if role == Role::Player && stream_count >= 16 {
                        let close_frame = Frame::new(Cmd::Close, stream_id, target_port, Bytes::new());
                        self.send_frame_to_peer(peer_addr, close_frame, sessions).await;
                        return;
                    }

                    let (tx, rx) = mpsc::channel::<Bytes>(128);
                    let _ = tx.send(frame.payload).await;
                    {
                        let mut guard = sessions.write().await;
                        if let Some(s) = guard.get_mut(&peer_addr) {
                            s.tcp_streams.insert(stream_id, tx);
                        }
                    }

                    let sessions_clone = sessions.clone();
                    let udp_sock_clone = self.udp_sock.clone();
                    tokio::spawn(async move {
                        handle_tcp_bridge(
                            peer_addr,
                            stream_id,
                            target_port,
                            virtual_ip,
                            rx,
                            sessions_clone,
                            udp_sock_clone,
                        )
                        .await;
                    });
                }
            }

            Cmd::Close => {
                let stream_id = frame.stream_id;
                let mut guard = sessions.write().await;
                if let Some(s) = guard.get_mut(&peer_addr) {
                    s.tcp_streams.remove(&stream_id);
                }
            }

            Cmd::Disconnect => {
                let mut guard = sessions.write().await;
                if let Some(sess) = guard.remove(&peer_addr) {
                    let mut hist = self.history_manager.write().await;
                    hist.record_leave(
                        &peer_addr.to_string(),
                        sess.joined_at.elapsed().as_secs(),
                        sess.rx_bytes,
                        sess.tx_bytes,
                    );
                    info!("İstemci bağlantıyı sonlandırdı: {:?}", peer_addr);
                    if sess.authenticated {
                        self.logger.log_disconnect(
                            &sess.username,
                            sess.joined_at.elapsed().as_secs(),
                            sess.rx_bytes,
                            sess.tx_bytes,
                            &peer_addr.to_string(),
                            false,
                        );
                    }
                }
            }

            _ => {}
        }
    }

    async fn send_frame_to_peer(
        &self,
        peer_addr: SocketAddr,
        frame: Frame,
        sessions: &Arc<RwLock<HashMap<SocketAddr, ClientSession>>>,
    ) {
        let mut guard = sessions.write().await;
        if let Some(session) = guard.get_mut(&peer_addr) {
            let dgrams = session.kcp.send_frame(&frame);
            for d in dgrams {
                let enc = session.cipher.encrypt(&d);
                let _ = self.udp_sock.send_to(&enc, peer_addr).await;
            }
        }
    }

    async fn tick_kcp_sessions(&self, sessions: &Arc<RwLock<HashMap<SocketAddr, ClientSession>>>) {
        let mut guard = sessions.write().await;
        for (addr, session) in guard.iter_mut() {
            let now_ms = session.kcp.now_ms();
            let retransmissions = session.kcp.update(now_ms);
            for d in retransmissions {
                let enc = session.cipher.encrypt(&d);
                let _ = self.udp_sock.send_to(&enc, *addr).await;
            }
        }
    }

    async fn cleanup_dead_sessions(
        &self,
        sessions: &Arc<RwLock<HashMap<SocketAddr, ClientSession>>>,
    ) {
        let mut dead = Vec::new();
        {
            let guard = sessions.read().await;
            let now = Instant::now();
            for (addr, session) in guard.iter() {
                let silence_limit = if session.authenticated {
                    Duration::from_secs(25)
                } else {
                    Duration::from_secs(6)
                };
                if now.duration_since(session.last_activity) > silence_limit {
                    dead.push((
                        *addr,
                        session.hwid.clone(),
                        session.joined_at.elapsed().as_secs(),
                        session.rx_bytes,
                        session.tx_bytes,
                        session.username.clone(),
                        session.authenticated,
                    ));
                }
            }
        }

        if !dead.is_empty() {
            let mut guard = sessions.write().await;
            let mut hist = self.history_manager.write().await;
            for (addr, hwid, dur, rx, tx, username, auth) in dead {
                guard.remove(&addr);
                hist.record_leave(&addr.to_string(), dur, rx, tx);
                info!("Sessiz kalan istemci oturumu temizlendi: {:?} ({})", addr, hwid);
                if auth {
                    self.logger.log_disconnect(&username, dur, rx, tx, &addr.to_string(), true);
                }
            }
        }
    }

    async fn handle_drop_player_sessions(
        &self,
        sessions: &Arc<RwLock<HashMap<SocketAddr, ClientSession>>>,
    ) {
        let mut players_to_drop = Vec::new();
        {
            let guard = sessions.read().await;
            for (addr, session) in guard.iter() {
                if session.role == Role::Player {
                    players_to_drop.push((
                        *addr,
                        session.username.clone(),
                        session.joined_at.elapsed().as_secs(),
                        session.rx_bytes,
                        session.tx_bytes,
                    ));
                }
            }
        }

        if !players_to_drop.is_empty() {
            let kick_frame = Frame::new(
                Cmd::Disconnect,
                0,
                0,
                Bytes::from_static(b"ANAHTAR_YENILENDI"),
            );

            let mut guard = sessions.write().await;
            let mut hist = self.history_manager.write().await;
            for (addr, username, dur, rx, tx) in players_to_drop {
                if let Some(mut session) = guard.remove(&addr) {
                    let dgrams = session.kcp.send_frame(&kick_frame);
                    for d in dgrams {
                        let enc = session.cipher.encrypt(&d);
                        let _ = self.udp_sock.send_to(&enc, addr).await;
                    }
                    hist.record_leave(&addr.to_string(), dur, rx, tx);
                    self.logger.log_summary(&format!("{} bağlantısı kesildi (Anahtar yenilendi).", username));
                }
            }
        }
    }

    async fn update_active_clients_snapshot(
        &self,
        sessions: &Arc<RwLock<HashMap<SocketAddr, ClientSession>>>,
    ) {
        let mut snapshot_map = HashMap::new();
        {
            let guard = sessions.read().await;
            for (addr, session) in guard.iter() {
                if !session.authenticated {
                    continue;
                }

                let dur = session.joined_at.elapsed().as_secs();
                let ports = vec![self.config.read().await.game_port];

                snapshot_map.insert(
                    *addr,
                    ActiveClientSnapshot {
                        addr: addr.to_string(),
                        peer_addr: addr.to_string(),
                        ip: addr.ip().to_string(),
                        port: addr.port(),
                        username: session.username.clone(),
                        hostname: session.hostname.clone(),
                        computer_name: session.hostname.clone(),
                        hwid: session.hwid.clone(),
                        hwid_details: session.hwid_details.clone(),
                        os: session.os.clone(),
                        version: session.version.clone(),
                        role: session.role.as_str().to_string(),
                        ping: session.ping_ms,
                        ping_ms: session.ping_ms,
                        streams: session.tcp_streams.len(),
                        active_streams: session.tcp_streams.len(),
                        rx: session.rx_bytes,
                        bytes_received: session.rx_bytes,
                        tx: session.tx_bytes,
                        bytes_sent: session.tx_bytes,
                        duration: dur,
                        duration_seconds: dur,
                        virtual_ip: session.virtual_ip.clone(),
                        local_ports: ports,
                    },
                );
            }
        }

        *self.active_clients.write().await = snapshot_map;
    }

    async fn handle_panel_kick(
        &self,
        addr: &str,
        hwid: &str,
        sessions: &Arc<RwLock<HashMap<SocketAddr, ClientSession>>>,
    ) {
        let mut target_addr = None;
        {
            let guard = sessions.read().await;
            for (a, s) in guard.iter() {
                if (!addr.is_empty() && a.to_string() == addr)
                    || (!hwid.is_empty() && s.hwid.eq_ignore_ascii_case(hwid))
                {
                    target_addr = Some(*a);
                    break;
                }
            }
        }

        if let Some(target) = target_addr {
            let kick_frame = Frame::new(
                Cmd::Disconnect,
                0,
                0,
                Bytes::from_static(b"YONETICI_TARAFINDAN_ATILDI"),
            );
            self.send_frame_to_peer(target, kick_frame, sessions).await;

            let mut guard = sessions.write().await;
            if let Some(sess) = guard.remove(&target) {
                let mut hist = self.history_manager.write().await;
                hist.record_leave(
                    &target.to_string(),
                    sess.joined_at.elapsed().as_secs(),
                    sess.rx_bytes,
                    sess.tx_bytes,
                );
                info!("Yönetici tarafından istemci atıldı: {:?}", target);
                self.logger.log_kick(&sess.username, &target.to_string(), "Yönetici tarafından atıldı");
            }
        }
    }

    async fn spawn_signaling_broker(&self) -> tokio::task::JoinHandle<()> {
        let (room_id, room_sec, _) = {
            let cfg = self.config.read().await;
            derive_credentials_from_key(&cfg.server_key)
                .unwrap_or_else(|_| ("default_room".to_string(), "".to_string(), "".to_string()))
        };

        let local_port = self.udp_sock.local_addr().map(|a| a.port()).unwrap_or(25566);
        let pub_ep_ref = self.public_endpoint.clone();
        let udp_sock_ref = self.udp_sock.clone();
        let client_candidates_ref = self.client_candidates.clone();

        tokio::spawn(async move {
            let broker_urls: Vec<String> = DEFAULT_BROKERS.iter().map(|&s| s.to_string()).collect();
            let broker = SignalingBroker::new(broker_urls);
            let cipher_broker = PacketCipher::new(&derive_key(&room_sec), true);
            let mut interval = tokio::time::interval(Duration::from_millis(1000));
            let mut last_hello_publish: Option<Instant> = None;
            let mut last_published_ep: Option<SocketAddr> = None;
            let mut seen_msgs: HashMap<String, Instant> = HashMap::new();

            loop {
                interval.tick().await;

                let allow_wan = std::env::var("MAZITUNNEL_QUARANTINE")
                    .map(|v| v != "1" && !v.eq_ignore_ascii_case("true"))
                    .unwrap_or(true);

                let current_ep = *pub_ep_ref.read().await;
                let (pub_ip, pub_port) = match current_ep {
                    Some(ep) => (ep.ip().to_string(), ep.port()),
                    None => ("127.0.0.1".to_string(), local_port),
                };

                let my_lan_ip = mazitunnel_core::nat_traversal::get_local_lan_ip();
                let now_ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs_f64();

                let make_server_hello_hex = |ip: &str, port: u16, target_cid: Option<&str>| -> String {
                    let mut server_hello = serde_json::json!({
                        "type": "server_hello",
                        "ip": ip,
                        "port": port,
                        "lan_ip": my_lan_ip,
                        "local_port": local_port,
                        "ts": now_ts,
                    });
                    if let Some(cid) = target_cid {
                        server_hello["client_id"] = serde_json::Value::String(cid.to_string());
                    }
                    let enc = cipher_broker.encrypt(server_hello.to_string().as_bytes());
                    hex::encode(enc)
                };

                // Clean seen_msgs older than 120s
                let now_inst = Instant::now();
                seen_msgs.retain(|_, t| now_inst.saturating_duration_since(*t) < Duration::from_secs(120));

                let should_publish_periodic = match last_hello_publish {
                    None => true, // First startup iteration
                    Some(t) => {
                        current_ep != last_published_ep || t.elapsed() >= Duration::from_secs(45)
                    }
                };

                if should_publish_periodic {
                    let hello_hex = make_server_hello_hex(&pub_ip, pub_port, None);
                    let _ = broker.publish(&room_id, &hello_hex).await;
                    let legacy_payload = format!("SERVER:{}:{}|{}:{}", pub_ip, pub_port, my_lan_ip, local_port);
                    let _ = broker.publish(&room_id, &hex::encode(cipher_broker.encrypt(legacy_payload.as_bytes()))).await;
                    last_hello_publish = Some(Instant::now());
                    last_published_ep = current_ep;
                }

                // Poll incoming messages
                if let Ok(msgs) = broker.poll_messages(&room_id, "60s").await {
                    for msg in msgs {
                        let trimmed = msg.trim();
                        if trimmed.is_empty() {
                            continue;
                        }

                        let decrypted_opt = if let Ok(raw_bytes) = hex::decode(trimmed) {
                            cipher_broker.decrypt(&raw_bytes)
                        } else {
                            None
                        };

                        let json_val = if let Some(ref dec) = decrypted_opt {
                            serde_json::from_slice::<Value>(dec).ok()
                        } else {
                            serde_json::from_str::<Value>(trimmed).ok()
                        };

                        if let Some(val) = json_val {
                            if val.get("type").and_then(|v| v.as_str()) == Some("client_hello") {
                                let ts = val.get("ts").and_then(|v| v.as_f64()).unwrap_or(0.0);
                                if ts > 0.0 && (now_ts - ts).abs() > 60.0 {
                                    continue;
                                }

                                let msg_id = val.get("msg_id")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string())
                                    .unwrap_or_else(|| trimmed.to_string());

                                if seen_msgs.contains_key(&msg_id) {
                                    continue;
                                }
                                seen_msgs.insert(msg_id, Instant::now());

                                let cip = val.get("ip").and_then(|v| v.as_str()).unwrap_or("");
                                let cport = val.get("port").and_then(|v| v.as_u64()).unwrap_or(0) as u16;
                                let clan = val.get("lan_ip").and_then(|v| v.as_str()).unwrap_or("");
                                let clocal_port = val.get("local_port").and_then(|v| v.as_u64()).unwrap_or(0) as u16;
                                let cid = val.get("client_id").or_else(|| val.get("cid")).and_then(|v| v.as_str()).unwrap_or("");

                                let mut targets: Vec<SocketAddr> = Vec::new();
                                // Only add clan:clocal_port to targets if cip == pub_ip (same public IP / same LAN)
                                if cip == pub_ip && !clan.is_empty() && clocal_port > 0 {
                                    if let Ok(addr) = format!("{}:{}", clan, clocal_port).parse::<SocketAddr>() {
                                        if !targets.contains(&addr) {
                                            targets.push(addr);
                                        }
                                    }
                                }

                                // Always add cip:cport if valid and not 127.0.0.1
                                if !cip.is_empty() && cport > 0 && cip != "127.0.0.1" {
                                    if let Ok(addr) = format!("{}:{}", cip, cport).parse::<SocketAddr>() {
                                        if !targets.contains(&addr) {
                                            targets.push(addr);
                                        }
                                    }
                                }

                                for target_addr in targets {
                                    if !allow_wan && !mazitunnel_core::nat_traversal::is_lan_addr(target_addr.ip()) {
                                        continue;
                                    }

                                    client_candidates_ref.write().await.insert(
                                        target_addr,
                                        Instant::now() + Duration::from_secs(25),
                                    );

                                    let enc_syn = cipher_broker.encrypt(b"P2P_TUN_PUNCH_SYN");
                                    let enc_ack = cipher_broker.encrypt(b"P2P_TUN_PUNCH_ACK");
                                    let _ = udp_sock_ref.send_to(&enc_syn, target_addr).await;
                                    let _ = udp_sock_ref.send_to(&enc_ack, target_addr).await;

                                    if !mazitunnel_core::nat_traversal::is_lan_addr(target_addr.ip()) {
                                        let base = target_addr.port() as i32;
                                        for delta in -5..=40 {
                                            if delta == 0 {
                                                continue;
                                            }
                                            let p = base + delta;
                                            if (1024..=65535).contains(&p) {
                                                let mut dest = target_addr;
                                                dest.set_port(p as u16);
                                                let _ = udp_sock_ref.send_to(&enc_syn, dest).await;
                                            }
                                        }
                                    }
                                }

                                // Immediately publish server_hello (including "client_id": cid in JSON)
                                let resp_hex = make_server_hello_hex(&pub_ip, pub_port, if cid.is_empty() { None } else { Some(cid) });
                                let _ = broker.publish(&room_id, &resp_hex).await;
                                last_hello_publish = Some(Instant::now());
                                last_published_ep = current_ep;
                            }
                        } else {
                            let text_opt = if let Some(ref dec) = decrypted_opt {
                                std::str::from_utf8(dec).ok().map(|s| s.to_string())
                            } else {
                                Some(trimmed.to_string())
                            };

                            if let Some(text) = text_opt {
                                if let Some(client_ep_str) = text.strip_prefix("CLIENT:") {
                                    let msg_id = trimmed.to_string();
                                    if seen_msgs.contains_key(&msg_id) {
                                        continue;
                                    }
                                    seen_msgs.insert(msg_id, Instant::now());

                                    let mut targets: Vec<SocketAddr> = Vec::new();
                                    for part in client_ep_str.split('|') {
                                        if let Ok(addr) = part.trim().parse::<SocketAddr>() {
                                            if !targets.contains(&addr) {
                                                targets.push(addr);
                                            }
                                        }
                                    }

                                    for target_addr in targets {
                                        if !allow_wan && !mazitunnel_core::nat_traversal::is_lan_addr(target_addr.ip()) {
                                            continue;
                                        }
                                        client_candidates_ref.write().await.insert(
                                            target_addr,
                                            Instant::now() + Duration::from_secs(25),
                                        );

                                        let enc_syn = cipher_broker.encrypt(b"P2P_TUN_PUNCH_SYN");
                                        let enc_ack = cipher_broker.encrypt(b"P2P_TUN_PUNCH_ACK");
                                        let _ = udp_sock_ref.send_to(&enc_syn, target_addr).await;
                                        let _ = udp_sock_ref.send_to(&enc_ack, target_addr).await;
                                    }

                                    let resp_hex = make_server_hello_hex(&pub_ip, pub_port, None);
                                    let _ = broker.publish(&room_id, &resp_hex).await;
                                    last_hello_publish = Some(Instant::now());
                                    last_published_ep = current_ep;
                                }
                            }
                        }
                    }
                }
            }
        })
    }

    async fn print_startup_banner(&self) {
        let cfg = self.config.read().await;
        let pub_str = {
            let ep = self.public_endpoint.read().await;
            ep.map(|e| e.to_string()).unwrap_or_else(|| "Bilinmiyor".to_string())
        };

        println!("\n================================================================");
        println!("                  maziTunnel SUNUCU SERVİSİ");
        println!("================================================================");
        println!("  Sunucu Adı       : {}", cfg.server_name);
        println!("  Yerel UDP Portu  : {}", cfg.server_udp_port);
        println!("  Dış Ağ (STUN)    : {}", pub_str);
        println!("  Bağlantı Adresi  : {}", cfg.connection_address);
        println!("  Oyun Portu       : {}", cfg.game_port);
        println!("  maziPanel Köprüsü: 127.0.0.1:{}", cfg.web_port);
        println!("----------------------------------------------------------------");
        println!("  [OYUNCU BAĞLANTI KODU] (Oyuncularla Paylaşılacak):");
        println!("  >> {}", cfg.server_key);
        println!();
        println!("  [GİZLİ YÖNETİCİ ŞİFRESİ] (SSH ve Yönetim İçin):");
        println!("  >> {}", cfg.admin_secret);
        println!("================================================================\n");
    }
}

async fn handle_tcp_bridge(
    peer_addr: SocketAddr,
    stream_id: u32,
    target_port: u16,
    virtual_ip: String,
    mut client_rx: mpsc::Receiver<Bytes>,
    sessions: Arc<RwLock<HashMap<SocketAddr, ClientSession>>>,
    udp_sock: Arc<UdpSocket>,
) {
    let connect_res = connect_to_local_service(target_port, Some(&virtual_ip)).await;
    let mut tcp_stream = match connect_res {
        Ok(s) => {
            let _ = s.set_nodelay(true);
            s
        }
        Err(e) => {
            warn!(
                "Hedef port {} bağlantısı başarısız (Sanal IP: {}): {}",
                target_port, virtual_ip, e
            );
            let close_frame = Frame::new(Cmd::Close, stream_id, target_port, Bytes::new());
            send_frame_direct(peer_addr, close_frame, &sessions, &udp_sock).await;
            return;
        }
    };

    let (mut tcp_reader, mut tcp_writer) = tcp_stream.split();
    let mut tcp_buf = [0u8; 4096];

    loop {
        tokio::select! {
            // Read from local TCP service (e.g. Minecraft 25565) -> send to client
            read_res = tcp_reader.read(&mut tcp_buf) => {
                match read_res {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        for chunk in tcp_buf[..n].chunks(1200) {
                            let payload = Bytes::copy_from_slice(chunk);
                            let data_frame = Frame::new(Cmd::Data, stream_id, target_port, payload);
                            send_frame_direct(peer_addr, data_frame, &sessions, &udp_sock).await;
                        }
                    }
                    Err(_) => break,
                }
            }

            // Inbound data from client -> write to local TCP service
            Some(chunk) = client_rx.recv() => {
                if tcp_writer.write_all(&chunk).await.is_err() {
                    break;
                }
            }
        }
    }

    let close_frame = Frame::new(Cmd::Close, stream_id, target_port, Bytes::new());
    send_frame_direct(peer_addr, close_frame, &sessions, &udp_sock).await;

    let mut guard = sessions.write().await;
    if let Some(s) = guard.get_mut(&peer_addr) {
        s.tcp_streams.remove(&stream_id);
    }
}

async fn send_frame_direct(
    peer_addr: SocketAddr,
    frame: Frame,
    sessions: &Arc<RwLock<HashMap<SocketAddr, ClientSession>>>,
    udp_sock: &Arc<UdpSocket>,
) {
    let mut guard = sessions.write().await;
    if let Some(session) = guard.get_mut(&peer_addr) {
        let dgrams = session.kcp.send_frame(&frame);
        for d in dgrams {
            let enc = session.cipher.encrypt(&d);
            let _ = udp_sock.send_to(&enc, peer_addr).await;
        }
    }
}

pub async fn connect_to_local_service(
    target_port: u16,
    virtual_ip: Option<&str>,
) -> std::io::Result<tokio::net::TcpStream> {
    let target_addr: SocketAddr = format!("127.0.0.1:{}", target_port)
        .parse()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

    if let Some(vip_str) = virtual_ip {
        if let Ok(vip) = vip_str.parse::<Ipv4Addr>() {
            let sock = socket2::Socket::new(
                socket2::Domain::IPV4,
                socket2::Type::STREAM,
                Some(socket2::Protocol::TCP),
            )?;
            let _ = sock.set_reuse_address(true);
            let local_sockaddr = SocketAddr::new(IpAddr::V4(vip), 0);

            if sock.bind(&local_sockaddr.into()).is_ok() {
                let _ = sock.set_nonblocking(true);
                match sock.connect(&target_addr.into()) {
                    Ok(()) => {
                        let std_stream: std::net::TcpStream = sock.into();
                        let tokio_s = tokio::net::TcpStream::from_std(std_stream)?;
                        let _ = tokio_s.set_nodelay(true);
                        return Ok(tokio_s);
                    }
                    Err(_e) => {
                        #[cfg(unix)]
                        if _e.raw_os_error() == Some(libc::EINPROGRESS) {
                            let std_stream: std::net::TcpStream = sock.into();
                            let tokio_stream = tokio::net::TcpStream::from_std(std_stream)?;
                            tokio_stream.writable().await?;
                            if let Some(err) = tokio_stream.take_error()? {
                                return Err(err);
                            }
                            let _ = tokio_stream.set_nodelay(true);
                            return Ok(tokio_stream);
                        }
                    }
                }
            }
        }
    }

    // Direct loopback fallback
    let stream = tokio::net::TcpStream::connect(target_addr).await?;
    let _ = stream.set_nodelay(true);
    Ok(stream)
}

pub fn ensure_virtual_ip_subnet() {
    #[cfg(target_os = "linux")]
    {
        use std::process::Command;
        let check = Command::new("ip")
            .args(["route", "show", "table", "local"])
            .output();

        if let Ok(out) = check {
            let s = String::from_utf8_lossy(&out.stdout);
            if s.contains("100.0.0.0/16") || s.contains("100.0.0.0/24") {
                return;
            }
        }

        let cmd = Command::new("ip")
            .args(["route", "replace", "local", "100.0.0.0/16", "dev", "lo"])
            .status();

        if let Err(e) = cmd {
            warn!("Sanal IP yönlendirme rotası uygulanamadı: {}", e);
        }
    }
}

fn bind_udp_socket(addr: SocketAddrV4) -> Result<std::net::UdpSocket> {
    let sock = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )?;

    sock.set_reuse_address(true)?;
    #[cfg(unix)]
    let _ = sock.set_reuse_port(true);

    let _ = sock.set_recv_buffer_size(4 * 1024 * 1024);
    let _ = sock.set_send_buffer_size(4 * 1024 * 1024);
    sock.set_nonblocking(true)?;

    sock.bind(&addr.into())?;
    Ok(sock.into())
}

pub fn get_local_lan_ip() -> Ipv4Addr {
    if let Ok(s) = std::net::UdpSocket::bind("0.0.0.0:0") {
        if s.connect("8.8.8.8:80").is_ok() {
            if let Ok(SocketAddr::V4(addr)) = s.local_addr() {
                return *addr.ip();
            }
        }
    }
    Ipv4Addr::new(127, 0, 0, 1)
}

pub fn resolve_server_config_path(custom: Option<&str>) -> PathBuf {
    if let Some(c) = custom {
        if !c.trim().is_empty() {
            return PathBuf::from(c.trim());
        }
    }

    if let Ok(env_path) = std::env::var("MAZITUNNEL_CONFIG_PATH") {
        if !env_path.trim().is_empty() {
            return PathBuf::from(env_path.trim());
        }
    }

    let candidates = [
        "/mazideveloper/p2ptunnel/config/server_config.json",
        "/mazideveloper/mazitunnel/config/server_config.json",
        "/mazideveloper/projects/maziTunnel/config/server_config.json",
        "./config/server_config.json",
        "server_config.json",
    ];

    for path_str in candidates {
        let p = Path::new(path_str);
        if p.exists() {
            return p.to_path_buf();
        }
    }

    PathBuf::from("/mazideveloper/mazitunnel/config/server_config.json")
}

pub fn load_or_create_server_config(path: &Path) -> Result<ServerConfig> {
    if path.exists() {
        if let Ok(content) = std::fs::read_to_string(path) {
            if let Ok(mut cfg) = serde_json::from_str::<ServerConfig>(&content) {
                // Normalize connection address (.localhost -> .local)
                cfg.connection_address = normalize_local_domain(&cfg.connection_address);

                // Ensure valid 12-char key
                let clean_key = normalize_server_key(&cfg.server_key);
                if clean_key.len() == 14 {
                    cfg.server_key = clean_key.clone();
                    if let Ok((_r_id, r_sec, p_tok)) = derive_credentials_from_key(&clean_key) {
                        cfg.room_secret = r_sec;
                        cfg.player_token = p_tok;
                    }
                } else {
                    let new_key = generate_server_key();
                    if let Ok((_r_id, r_sec, p_tok)) = derive_credentials_from_key(&new_key) {
                        cfg.server_key = new_key;
                        cfg.room_secret = r_sec;
                        cfg.player_token = p_tok;
                    }
                }

                if cfg.admin_secret.trim().is_empty() {
                    cfg.admin_secret = generate_admin_secret(20);
                }

                save_config_atomic(&path.to_path_buf(), &cfg);
                return Ok(cfg);
            }
        }
    }

    let server_key = generate_server_key();
    let (_room_id, room_secret, player_token) = derive_credentials_from_key(&server_key)
        .map_err(|e| anyhow::anyhow!(e))?;

    let admin_secret = generate_admin_secret(20);

    let config = ServerConfig {
        server_name: "maziTunnel Sunucusu".to_string(),
        server_key,
        game_port: 25565,
        connection_address: "baglanti.local".to_string(),
        server_udp_port: 25566,
        web_port: 9090,
        player_token,
        admin_secret,
        room_secret,
        panel_mode: "mazipanel".to_string(),
    };

    save_config_atomic(&path.to_path_buf(), &config);
    Ok(config)
}

pub fn generate_admin_secret(length: usize) -> String {
    let chars = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789*!?@#$%^&_-";
    let mut rng = rand::thread_rng();
    let mut result = Vec::with_capacity(length);

    for _ in 0..length {
        let idx = (rng.next_u32() as usize) % chars.len();
        result.push(chars[idx]);
    }

    String::from_utf8(result).unwrap_or_else(|_| "AdminSecretKey2026!".to_string())
}
