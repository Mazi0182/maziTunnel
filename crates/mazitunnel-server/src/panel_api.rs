use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mazitunnel_core::crypto::{
    derive_credentials_from_key, generate_server_key, normalize_server_key,
};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, RwLock};
use tracing::{error, info, warn};

use crate::acl::ServerAcl;
use crate::ban_manager::BanManager;
use crate::history_manager::HistoryManager;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub server_name: String,
    pub server_key: String,
    pub game_port: u16,
    pub connection_address: String,
    pub server_udp_port: u16,
    pub web_port: u16,
    pub player_token: String,
    pub admin_secret: String,
    pub room_secret: String,
    #[serde(default = "default_panel_mode")]
    pub panel_mode: String,
}

fn default_panel_mode() -> String {
    "mazipanel".to_string()
}

pub fn normalize_local_domain(raw: &str) -> String {
    mazitunnel_core::paths::normalize_local_domain(raw)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveClientSnapshot {
    pub addr: String,
    pub peer_addr: String,
    pub ip: String,
    pub port: u16,
    pub username: String,
    pub hostname: String,
    pub computer_name: String,
    pub hwid: String,
    pub hwid_details: serde_json::Value,
    pub os: String,
    pub version: String,
    pub role: String,
    pub ping: u32,
    pub ping_ms: u32,
    pub streams: usize,
    pub active_streams: usize,
    pub rx: u64,
    pub bytes_received: u64,
    pub tx: u64,
    pub bytes_sent: u64,
    pub duration: u64,
    pub duration_seconds: u64,
    pub virtual_ip: String,
    pub local_ports: Vec<u16>,
}

pub enum PanelCommand {
    Kick { addr: String, hwid: String },
    ReloadSignaling,
    DropPlayerSessions,
}

pub struct PanelApiContext {
    pub config_path: PathBuf,
    pub config: Arc<RwLock<ServerConfig>>,
    pub acl: Arc<ServerAcl>,
    pub ban_manager: Arc<std::sync::RwLock<BanManager>>,
    pub history_manager: Arc<RwLock<HistoryManager>>,
    pub active_clients: Arc<RwLock<HashMap<SocketAddr, ActiveClientSnapshot>>>,
    pub public_endpoint: Arc<RwLock<Option<SocketAddr>>>,
    pub start_time: Instant,
    pub cmd_tx: mpsc::Sender<PanelCommand>,
    pub logger: Arc<crate::logger::ServerLogger>,
}

pub async fn run_panel_api(ctx: Arc<PanelApiContext>, port: u16) {
    let bind_addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = match TcpListener::bind(bind_addr).await {
        Ok(l) => {
            info!("maziPanel HTTP API dinleyicisi baslatildi: 127.0.0.1:{}", port);
            l
        }
        Err(e) => {
            error!("maziPanel HTTP API port 127.0.0.1:{} baglanamadi: {}", port, e);
            return;
        }
    };

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let ctx_clone = ctx.clone();
                tokio::spawn(async move {
                    handle_http_connection(stream, ctx_clone).await;
                });
            }
            Err(e) => {
                warn!("HTTP kabul hatasi: {}", e);
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

async fn handle_http_connection(mut stream: TcpStream, ctx: Arc<PanelApiContext>) {
    let mut buf = Vec::with_capacity(4096);
    let mut temp = [0u8; 1024];

    // Read headers until \r\n\r\n
    let header_end_pos = loop {
        match tokio::time::timeout(Duration::from_secs(5), stream.read(&mut temp)).await {
            Ok(Ok(n)) if n > 0 => {
                buf.extend_from_slice(&temp[..n]);
                if let Some(pos) = find_subsequence(&buf, b"\r\n\r\n") {
                    break pos;
                }
                if buf.len() > 65536 {
                    return; // Request header too large
                }
            }
            _ => return,
        }
    };

    let header_bytes = &buf[..header_end_pos];
    let header_str = String::from_utf8_lossy(header_bytes);
    let mut lines = header_str.lines();

    let request_line = match lines.next() {
        Some(l) => l.trim(),
        None => return,
    };

    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_uppercase();
    let mut raw_path = parts.next().unwrap_or("/").to_string();

    if let Some((p, _query)) = raw_path.split_once('?') {
        raw_path = p.to_string();
    }

    // CORS Preflight
    if method == "OPTIONS" {
        let resp = "HTTP/1.1 204 No Content\r\n\
            Access-Control-Allow-Origin: *\r\n\
            Access-Control-Allow-Methods: GET, POST, PUT, DELETE, OPTIONS\r\n\
            Access-Control-Allow-Headers: Content-Type, Authorization, X-Admin-Secret\r\n\
            Content-Length: 0\r\n\
            Connection: close\r\n\r\n";
        let _ = stream.write_all(resp.as_bytes()).await;
        return;
    }

    // Parse Content-Length and Origin headers
    let mut content_length = 0usize;
    let mut origin_header: Option<String> = None;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            let k_clean = k.trim();
            if k_clean.eq_ignore_ascii_case("content-length") {
                content_length = v.trim().parse::<usize>().unwrap_or(0).min(65536);
            } else if k_clean.eq_ignore_ascii_case("origin") {
                origin_header = Some(v.trim().to_string());
            }
        }
    }

    // Origin CSRF & DNS Rebinding validation
    if let Some(ref origin) = origin_header {
        let origin_lower = origin.to_ascii_lowercase();
        let allowed = origin_lower.contains("127.0.0.1")
            || origin_lower.contains("localhost")
            || origin_lower.contains(".mazidev.xyz")
            || origin_lower == "null";
        if !allowed {
            let resp = "HTTP/1.1 403 Forbidden\r\n\
                Content-Type: application/json; charset=utf-8\r\n\
                Connection: close\r\n\
                Content-Length: 35\r\n\r\n\
                {\"error\":\"Origin not allowed (CSRF)\"}";
            let _ = stream.write_all(resp.as_bytes()).await;
            return;
        }
    }

    // Read body if necessary
    let body_start = header_end_pos + 4;
    let mut body_bytes = buf[body_start..].to_vec();
    while body_bytes.len() < content_length {
        let needed = content_length - body_bytes.len();
        let to_read = needed.min(temp.len());
        match tokio::time::timeout(Duration::from_secs(5), stream.read(&mut temp[..to_read])).await {
            Ok(Ok(n)) if n > 0 => {
                body_bytes.extend_from_slice(&temp[..n]);
            }
            _ => break,
        }
    }

    let (status_code, response_json) = dispatch_api(&method, &raw_path, &body_bytes, &ctx).await;

    let body_str = response_json.to_string();
    let resp = format!(
        "HTTP/1.1 {}\r\n\
        Content-Type: application/json; charset=utf-8\r\n\
        Access-Control-Allow-Origin: *\r\n\
        Access-Control-Allow-Methods: GET, POST, PUT, DELETE, OPTIONS\r\n\
        Access-Control-Allow-Headers: Content-Type, Authorization, X-Admin-Secret\r\n\
        Content-Length: {}\r\n\
        Connection: close\r\n\r\n{}",
        status_code,
        body_str.len(),
        body_str
    );

    let _ = stream.write_all(resp.as_bytes()).await;
}

async fn dispatch_api(
    method: &str,
    path: &str,
    body: &[u8],
    ctx: &PanelApiContext,
) -> (&'static str, serde_json::Value) {
    let clean_path = path.trim_end_matches('/');

    match (method, clean_path) {
        ("GET", "/api/status") | ("GET", "/status") => {
            handle_get_status(ctx).await
        }
        ("GET", "/api/logs") | ("GET", "/logs") => {
            handle_get_logs(ctx).await
        }
        ("POST", "/api/kick") => {
            handle_post_kick(body, ctx).await
        }
        ("POST", "/api/ban") => {
            handle_post_ban(body, ctx).await
        }
        ("POST", "/api/unban") => {
            handle_post_unban(body, ctx).await
        }
        ("POST", "/api/clear_history") | ("POST", "/api/clear-history") => {
            handle_clear_history(ctx).await
        }
        ("POST", "/api/regenerate_key") | ("POST", "/api/regenerate-key") => {
            handle_regenerate_key(ctx).await
        }
        ("POST", "/api/settings")
        | ("PUT", "/api/settings")
        | ("POST", "/api/update_settings")
        | ("PUT", "/api/update_settings") => {
            handle_update_settings(body, ctx).await
        }
        ("POST", "/api/version/refresh") => {
            ("200 OK", serde_json::json!({
                "success": true,
                "version": "2.0.0",
                "message": "maziTunnel aktif ve guncel."
            }))
        }
        ("GET", "/api/ping") | ("GET", "") | ("GET", "/api") => {
            ("200 OK", serde_json::json!({
                "status": "ok",
                "service": "mazitunnel-server",
                "version": "2.0.0"
            }))
        }
        _ => {
            ("404 Not Found", serde_json::json!({
                "error": "Bilinmeyen uc nokta",
                "path": path
            }))
        }
    }
}

async fn handle_get_status(ctx: &PanelApiContext) -> (&'static str, serde_json::Value) {
    let cfg = ctx.config.read().await.clone();
    let uptime = ctx.start_time.elapsed().as_secs();

    let pub_endpoint_str = {
        let ep = ctx.public_endpoint.read().await;
        ep.map(|a| a.to_string()).unwrap_or_default()
    };

    let clients_list: Vec<ActiveClientSnapshot> = {
        let clients = ctx.active_clients.read().await;
        clients.values().cloned().collect()
    };

    let bans_list = {
        let ban_mgr = ctx.ban_manager.read().unwrap();
        ban_mgr.list_bans()
    };

    let history_list = {
        let hist_mgr = ctx.history_manager.read().await;
        let ban_mgr = ctx.ban_manager.read().unwrap();
        hist_mgr.list_history(&ban_mgr, &clients_list)
    };

    let (summary_logs, detailed_logs) = ctx.logger.get_logs();

    ("200 OK", serde_json::json!({
        "running": true,
        "server_name": cfg.server_name,
        "server_key": cfg.server_key,
        "game_port": cfg.game_port,
        "connection_address": cfg.connection_address,
        "server_udp_port": cfg.server_udp_port,
        "web_port": cfg.web_port,
        "public_endpoint": pub_endpoint_str,
        "clients": clients_list,
        "bans": bans_list,
        "history": history_list,
        "summary_logs": summary_logs,
        "detailed_logs": detailed_logs,
        "uptime_seconds": uptime
    }))
}

async fn handle_get_logs(ctx: &PanelApiContext) -> (&'static str, serde_json::Value) {
    let (summary, detailed) = ctx.logger.get_logs();
    ("200 OK", serde_json::json!({
        "summary": summary,
        "detailed": detailed,
    }))
}

async fn handle_post_kick(body: &[u8], ctx: &PanelApiContext) -> (&'static str, serde_json::Value) {
    let payload: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => {
            return ("400 Bad Request", serde_json::json!({ "success": false, "error": "Gecersiz JSON verisi" }));
        }
    };

    let addr = payload.get("addr").or_else(|| payload.get("peer_addr")).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let hwid = payload.get("hwid").and_then(|v| v.as_str()).unwrap_or("").to_string();

    ctx.logger.log_kick(&addr, &addr, "Yonetici tarafindan atildi");
    let _ = ctx.cmd_tx.send(PanelCommand::Kick { addr: addr.clone(), hwid: hwid.clone() }).await;

    ("200 OK", serde_json::json!({
        "success": true,
        "message": format!("Istemci baglantisi kesildi: {} ({})", addr, hwid)
    }))
}

async fn handle_post_ban(body: &[u8], ctx: &PanelApiContext) -> (&'static str, serde_json::Value) {
    let payload: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => {
            return ("400 Bad Request", serde_json::json!({ "success": false, "error": "Gecersiz JSON verisi" }));
        }
    };

    let hwid = payload.get("hwid").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let addr = payload.get("addr").or_else(|| payload.get("peer_addr")).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let username = payload.get("username").and_then(|v| v.as_str()).unwrap_or("Bilinmeyen").trim().to_string();
    let hostname = payload.get("hostname").or_else(|| payload.get("computer_name")).and_then(|v| v.as_str()).unwrap_or("-").trim().to_string();
    let reason = payload.get("reason").and_then(|v| v.as_str()).unwrap_or("Yonetici tarafindan yasaklandi").trim().to_string();
    let hwid_details = payload.get("hwid_details").cloned().unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

    if hwid.is_empty() && addr.is_empty() {
        return ("400 Bad Request", serde_json::json!({ "success": false, "error": "Yasaklanacak HWID veya adres belirtilmedi" }));
    }

    let ban_entry = {
        let mut ban_mgr = ctx.ban_manager.write().unwrap();
        ban_mgr.add_ban(&hwid, &username, &hostname, &addr, &reason, hwid_details)
    };

    ctx.logger.log_ban(&username, &hwid, &reason);

    // If client is currently active, kick them
    let _ = ctx.cmd_tx.send(PanelCommand::Kick { addr, hwid }).await;

    ("200 OK", serde_json::json!({
        "success": true,
        "message": format!("{} basariyla yasaklandi.", username),
        "ban": ban_entry
    }))
}

async fn handle_post_unban(body: &[u8], ctx: &PanelApiContext) -> (&'static str, serde_json::Value) {
    let payload: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => {
            return ("400 Bad Request", serde_json::json!({ "success": false, "error": "Gecersiz JSON verisi" }));
        }
    };

    let identifier = payload.get("id")
        .or_else(|| payload.get("hwid"))
        .or_else(|| payload.get("target"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();

    if identifier.is_empty() {
        return ("400 Bad Request", serde_json::json!({ "success": false, "error": "Kaldirilacak yasak kimligi (ID veya HWID) belirtilmedi" }));
    }

    let removed = {
        let mut ban_mgr = ctx.ban_manager.write().unwrap();
        ban_mgr.remove_ban(identifier)
    };

    if removed {
        ("200 OK", serde_json::json!({
            "success": true,
            "message": "Yasak basariyla kaldirildi."
        }))
    } else {
        ("200 OK", serde_json::json!({
            "success": true,
            "message": "Belirtilen kimlikle eslesen aktif yasak bulunamadi."
        }))
    }
}

async fn handle_clear_history(ctx: &PanelApiContext) -> (&'static str, serde_json::Value) {
    let cleared = {
        let mut hist = ctx.history_manager.write().await;
        hist.clear_history()
    };

    if cleared {
        ("200 OK", serde_json::json!({
            "success": true,
            "message": "Baglanti gecmisi basariyla temizlendi."
        }))
    } else {
        ("500 Internal Error", serde_json::json!({
            "success": false,
            "error": "Gecmis dosyasina yazilamadi."
        }))
    }
}

async fn handle_regenerate_key(ctx: &PanelApiContext) -> (&'static str, serde_json::Value) {
    let new_key = generate_server_key();
    let (derived_room_id, derived_room_sec, derived_player_tok) = match derive_credentials_from_key(&new_key) {
        Ok(res) => res,
        Err(e) => {
            return ("500 Internal Error", serde_json::json!({ "success": false, "error": e }));
        }
    };

    {
        let mut cfg = ctx.config.write().await;
        cfg.server_key = new_key.clone();
        cfg.room_secret = derived_room_sec.clone();
        cfg.player_token = derived_player_tok.clone();

        save_config_atomic(&ctx.config_path, &cfg);
    }

    ctx.acl.set_player_token(&derived_player_tok);

    let _ = ctx.cmd_tx.send(PanelCommand::DropPlayerSessions).await;
    let _ = ctx.cmd_tx.send(PanelCommand::ReloadSignaling).await;
    ctx.logger.log_key_regenerated(&new_key);

    ("200 OK", serde_json::json!({
        "success": true,
        "server_key": new_key,
        "serverKey": new_key,
        "room_id": derived_room_id,
        "message": format!("Yeni sunucu baglanti kodu olusturuldu: {}", new_key)
    }))
}

async fn handle_update_settings(body: &[u8], ctx: &PanelApiContext) -> (&'static str, serde_json::Value) {
    let payload: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => {
            return ("400 Bad Request", serde_json::json!({ "success": false, "error": "Gecersiz JSON verisi" }));
        }
    };

    let mut key_changed = false;
    let mut updated_key = String::new();

    {
        let mut cfg = ctx.config.write().await;

        if let Some(name) = payload.get("server_name").or_else(|| payload.get("serverName")).and_then(|v| v.as_str()) {
            if !name.trim().is_empty() {
                cfg.server_name = name.trim().to_string();
            }
        }

        if let Some(port) = payload.get("game_port").or_else(|| payload.get("gamePort")).and_then(|v| v.as_u64()) {
            let p = port as u16;
            if p > 0 {
                cfg.game_port = p;
                ctx.acl.set_game_port(p);
            }
        }

        if let Some(addr) = payload.get("connection_address").or_else(|| payload.get("connectionAddress")).and_then(|v| v.as_str()) {
            let norm = normalize_local_domain(addr);
            cfg.connection_address = norm.clone();
            ctx.acl.set_connection_address(&norm);
        }

        if let Some(admin) = payload.get("admin_secret").or_else(|| payload.get("adminSecret")).and_then(|v| v.as_str()) {
            if !admin.trim().is_empty() {
                cfg.admin_secret = admin.trim().to_string();
                ctx.acl.set_admin_secret(admin.trim());
            }
        }

        if let Some(key) = payload.get("server_key").or_else(|| payload.get("serverKey")).and_then(|v| v.as_str()) {
            let norm_key = normalize_server_key(key);
            if norm_key.len() == 14 && norm_key != cfg.server_key {
                if let Ok((_r_id, r_sec, p_tok)) = derive_credentials_from_key(&norm_key) {
                    cfg.server_key = norm_key.clone();
                    cfg.room_secret = r_sec;
                    cfg.player_token = p_tok.clone();
                    ctx.acl.set_player_token(&p_tok);
                    key_changed = true;
                    updated_key = norm_key;
                }
            }
        }

        save_config_atomic(&ctx.config_path, &cfg);
    }

    if key_changed {
        let _ = ctx.cmd_tx.send(PanelCommand::DropPlayerSessions).await;
        let _ = ctx.cmd_tx.send(PanelCommand::ReloadSignaling).await;
        ctx.logger.log_key_regenerated(&updated_key);
    }

    let updated_cfg = ctx.config.read().await.clone();
    ("200 OK", serde_json::json!({
        "success": true,
        "message": "Yapilandirma basariyla kaydedildi.",
        "settings": updated_cfg
    }))
}

pub fn save_config_atomic(path: &PathBuf, config: &ServerConfig) -> bool {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = path.with_extension("tmp");
    let serialized = match serde_json::to_string_pretty(config) {
        Ok(s) => s,
        Err(_) => return false,
    };

    if std::fs::write(&tmp, serialized.as_bytes()).is_err() {
        return false;
    }

    if std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::copy(&tmp, path);
        let _ = std::fs::remove_file(&tmp);
    }
    true
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}
