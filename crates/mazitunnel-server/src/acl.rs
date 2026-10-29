use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use mazitunnel_core::crypto::compute_auth_hmac;
use mazitunnel_core::protocol::{AuthRequest, Role};

use crate::ban_manager::BanManager;

pub struct ServerAcl {
    pub player_tokens: RwLock<HashSet<String>>,
    pub admin_secret: RwLock<String>,
    pub allowed_game_port: RwLock<u16>,
    pub connection_address: RwLock<String>,
    pub ban_manager: Arc<RwLock<BanManager>>,
    used_nonces: Mutex<HashMap<String, (Instant, String)>>,
}

impl ServerAcl {
    pub fn new(
        player_token: &str,
        admin_secret: &str,
        allowed_game_port: u16,
        connection_address: &str,
        ban_manager: Arc<RwLock<BanManager>>,
    ) -> Self {
        let mut tokens = HashSet::new();
        if !player_token.trim().is_empty() {
            tokens.insert(player_token.trim().to_string());
        }

        Self {
            player_tokens: RwLock::new(tokens),
            admin_secret: RwLock::new(admin_secret.trim().to_string()),
            allowed_game_port: RwLock::new(allowed_game_port),
            connection_address: RwLock::new(connection_address.trim().to_string()),
            ban_manager,
            used_nonces: Mutex::new(HashMap::new()),
        }
    }

    pub fn add_player_token(&self, token: &str) {
        let clean = token.trim();
        if !clean.is_empty() {
            if let Ok(mut tokens) = self.player_tokens.write() {
                tokens.insert(clean.to_string());
            }
        }
    }

    pub fn set_player_token(&self, token: &str) {
        let clean = token.trim();
        if let Ok(mut tokens) = self.player_tokens.write() {
            tokens.clear();
            if !clean.is_empty() {
                tokens.insert(clean.to_string());
            }
        }
    }

    pub fn set_admin_secret(&self, secret: &str) {
        if let Ok(mut admin) = self.admin_secret.write() {
            *admin = secret.trim().to_string();
        }
    }

    pub fn set_game_port(&self, port: u16) {
        if let Ok(mut p) = self.allowed_game_port.write() {
            *p = port;
        }
    }

    pub fn set_connection_address(&self, addr: &str) {
        if let Ok(mut a) = self.connection_address.write() {
            *a = addr.trim().to_string();
        }
    }

    pub fn authenticate(
        &self,
        req: &AuthRequest,
        room_secret: &str,
        fallback_token: Option<&str>,
    ) -> Result<Role, String> {
        let client_nonce = req.client_nonce.trim();
        let hwid = req.hwid.trim();

        // 1. Replay attack protection with 10s idempotent retry window
        if !client_nonce.is_empty() {
            let mut nonces = self.used_nonces.lock().map_err(|_| "Sistem kilidi hatasi")?;
            let now = Instant::now();

            if nonces.len() > 1000 {
                nonces.retain(|_, (seen_time, _)| now.duration_since(*seen_time) < Duration::from_secs(60));
            }

            if let Some((seen_time, seen_hwid)) = nonces.get(client_nonce) {
                let elapsed = now.duration_since(*seen_time);
                if elapsed <= Duration::from_secs(10) && (seen_hwid.is_empty() || seen_hwid == hwid) {
                    // Idempotent retry allowed within 10s window
                } else {
                    return Err("REPLAY_DETECTED: Tekrar eden baglanti istegi engellendi.".to_string());
                }
            } else {
                nonces.insert(client_nonce.to_string(), (now, hwid.to_string()));
            }
        }

        // 2. HWID ban enforcement
        if !hwid.is_empty() || !req.hwid_details.is_null() {
            if let Ok(ban_mgr) = self.ban_manager.read() {
                if let Some(ban) = ban_mgr.is_banned(hwid, &req.hwid_details) {
                    return Err(format!("BANNED:{}", ban.reason));
                }
            }

            if !hwid.is_empty() && hwid != "N/A" && (hwid.len() < 12 || !hwid.contains("HWID")) {
                return Err("INVALID_HWID: Gecersiz donanim kimligi (HWID) formati.".to_string());
            }
        }

        let token_hmac = req.token_hmac.trim();

        // 3. Cryptographic HMAC Token Verification
        if !token_hmac.is_empty() && !client_nonce.is_empty() {
            let current_admin = self.admin_secret.read().map(|s| s.clone()).unwrap_or_default();
            if !current_admin.is_empty() {
                let expected_admin = compute_auth_hmac(&current_admin, room_secret, client_nonce);
                if constant_time_eq(token_hmac, &expected_admin) {
                    return Ok(Role::Admin);
                }
            }

            let tokens = self.player_tokens.read().map(|t| t.clone()).unwrap_or_default();
            for pt in &tokens {
                if !pt.is_empty() {
                    let expected_player = compute_auth_hmac(pt, room_secret, client_nonce);
                    if constant_time_eq(token_hmac, &expected_player) {
                        return Ok(Role::Player);
                    }
                }
            }
        }

        // 4. Plaintext Token Verification / Fallback
        if let Some(token) = fallback_token {
            let clean_token = token.trim();
            if !clean_token.is_empty() {
                let current_admin = self.admin_secret.read().map(|s| s.clone()).unwrap_or_default();
                if !current_admin.is_empty() && constant_time_eq(clean_token, &current_admin) {
                    return Ok(Role::Admin);
                }

                let tokens = self.player_tokens.read().map(|t| t.clone()).unwrap_or_default();
                for pt in &tokens {
                    if constant_time_eq(clean_token, pt) {
                        return Ok(Role::Player);
                    }
                }
            }
        }

        Err("AUTH_FAILED: Baglanti anahtari veya yetkilendirme basarisiz.".to_string())
    }

    pub fn is_port_allowed(&self, role: Role, target_port: u16) -> bool {
        match role {
            Role::Admin => true, // Admin has unrestricted port access (SSH 22, Web 9090, etc.)
            Role::Player => {
                let allowed_port = self.allowed_game_port.read().map(|p| *p).unwrap_or(25565);
                target_port == allowed_port
            }
        }
    }
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
        diff |= x ^ y;
    }
    diff == 0
}
