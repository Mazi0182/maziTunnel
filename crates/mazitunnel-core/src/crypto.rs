use std::net::Ipv4Addr;
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Key, Nonce,
};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use rand::{Rng, RngCore};
use sha2::{Digest, Sha256};

pub const DEFAULT_LOCAL_DOMAIN: &str = "baglanti.local";
pub const NONCE_LEN: usize = 12;
pub const TAG_LEN: usize = 16;
pub const MIN_PAD_LEN: usize = 1;
pub const MAX_PAD_LEN: usize = 16;

type HmacSha256 = Hmac<Sha256>;

pub fn generate_server_key() -> String {
    let mut bytes = [0u8; 6];
    rand::thread_rng().fill_bytes(&mut bytes);
    let hex_str = hex::encode_upper(bytes);
    format!("{}-{}-{}", &hex_str[0..4], &hex_str[4..8], &hex_str[8..12])
}

pub fn normalize_server_key(key: &str) -> String {
    let mut clean = key.trim().to_uppercase().replace('-', "").replace(' ', "");
    if clean.starts_with("MC") && clean.len() == 14 {
        clean = clean[2..].to_string();
    }
    if clean.len() == 12 && clean.chars().all(|c| c.is_ascii_alphanumeric()) {
        format!("{}-{}-{}", &clean[0..4], &clean[4..8], &clean[8..12])
    } else {
        clean
    }
}

pub fn derive_credentials_from_key(server_key: &str) -> Result<(String, String, String), String> {
    let clean: String = server_key
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect::<String>()
        .to_uppercase();
    if clean.len() != 12 {
        return Err("Geçersiz bağlantı kodu! Standart XXXX-XXXX-XXXX (12 karakter) formatında olmalıdır.".to_string());
    }
    let mut hasher = Sha256::new();
    hasher.update(format!("P2P_TUNNEL_KEY_{}", clean).as_bytes());
    let h = hex::encode(hasher.finalize());
    let room_id = format!("p2ptun_{}", &h[0..16]);
    let room_secret = format!("SEC_{}", &h[16..40]);
    let player_token = format!("TOK_{}", &h[40..56]);
    Ok((room_id, room_secret, player_token))
}

pub fn ip_port_to_short_code(ip: Ipv4Addr, port: u16) -> String {
    let octets = ip.octets();
    let port_bytes = port.to_be_bytes();
    let mut combined = [0u8; 6];
    combined[0..4].copy_from_slice(&octets);
    combined[4..6].copy_from_slice(&port_bytes);
    let h = hex::encode_upper(combined);
    format!("{}-{}-{}", &h[0..4], &h[4..8], &h[8..12])
}

pub fn short_code_to_ip_port(code: &str) -> Option<(Ipv4Addr, u16)> {
    let clean = code.trim().replace('-', "").replace(' ', "");
    if clean.len() != 12 {
        return None;
    }
    let bytes = hex::decode(&clean).ok()?;
    if bytes.len() != 6 {
        return None;
    }
    let ip = Ipv4Addr::new(bytes[0], bytes[1], bytes[2], bytes[3]);
    let port = u16::from_be_bytes([bytes[4], bytes[5]]);
    Some((ip, port))
}

pub fn derive_key(secret: &str) -> [u8; 32] {
    let salt = b"P2P_TUNNEL_ANTI_DPI_SALT_2026";
    let info = b"p2p-tunnel-v1";
    let hk = Hkdf::<Sha256>::new(Some(salt), secret.as_bytes());
    let mut key = [0u8; 32];
    hk.expand(info, &mut key)
        .expect("32 bytes is valid length for HKDF-SHA256");
    key
}

pub fn derive_session_key(base_secret: &str, client_nonce: &str, server_nonce: &str) -> [u8; 32] {
    derive_session_key_with_token(base_secret, client_nonce, server_nonce, base_secret)
}

pub fn derive_session_key_with_token(
    base_secret: &str,
    client_nonce: &str,
    server_nonce: &str,
    token_secret: &str,
) -> [u8; 32] {
    let rev_c: String = client_nonce.chars().rev().collect();
    let salt = format!("{}:{}", token_secret, rev_c);
    let info = format!("p2p-tunnel-session-v1:{}:{}", client_nonce, server_nonce);
    let hk = Hkdf::<Sha256>::new(Some(salt.as_bytes()), base_secret.as_bytes());
    let mut key = [0u8; 32];
    hk.expand(info.as_bytes(), &mut key)
        .expect("32 bytes is valid length for HKDF-SHA256");
    key
}

pub fn compute_auth_hmac(secret: &str, room_secret: &str, client_nonce: &str) -> String {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(secret.as_bytes())
        .expect("HMAC-SHA256 accepts variable length keys");
    let msg = format!("P2P_AUTH:{}:{}", room_secret, client_nonce);
    mac.update(msg.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

pub fn compute_server_proof(room_secret: &str, client_nonce: &str, server_nonce: &str) -> String {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(room_secret.as_bytes())
        .expect("HMAC-SHA256 accepts variable length keys");
    let msg = format!("P2P_SERVER_OK:{}:{}", client_nonce, server_nonce);
    mac.update(msg.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

#[derive(Clone)]
pub struct PacketCipher {
    base_key: [u8; 32],
    session_key: Option<[u8; 32]>,
}

impl PacketCipher {
    pub fn new(base_key: &[u8; 32], _obfuscate: bool) -> Self {
        Self {
            base_key: *base_key,
            session_key: None,
        }
    }

    pub fn from_key(base_key: [u8; 32]) -> Self {
        Self {
            base_key,
            session_key: None,
        }
    }

    pub fn set_session_key(&mut self, session_key: [u8; 32]) {
        self.session_key = Some(session_key);
    }

    pub fn encrypt(&self, plaintext: &[u8]) -> Vec<u8> {
        let mut rng = rand::thread_rng();
        let pad_len = rng.gen_range(MIN_PAD_LEN..=MAX_PAD_LEN);
        let mut padding = vec![0u8; pad_len];
        rng.fill_bytes(&mut padding);

        let mut padded = Vec::with_capacity(plaintext.len() + pad_len + 1);
        padded.extend_from_slice(plaintext);
        padded.extend_from_slice(&padding);
        padded.push(pad_len as u8);

        let mut nonce_bytes = [0u8; NONCE_LEN];
        rng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let active_key = self.session_key.as_ref().unwrap_or(&self.base_key);
        let cipher = ChaCha20Poly1305::new(Key::from_slice(active_key));

        let ciphertext = cipher
            .encrypt(nonce, padded.as_ref())
            .expect("ChaCha20Poly1305 encryption cannot fail with valid inputs");

        let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&ciphertext);
        out
    }

    pub fn decrypt(&self, packet: &[u8]) -> Option<Vec<u8>> {
        // Minimum packet length: 12 (nonce) + 16 (tag) + 1 (min pad) + 1 (pad_len byte) = 30
        if packet.len() < NONCE_LEN + TAG_LEN + 2 {
            return None;
        }

        let nonce = Nonce::from_slice(&packet[0..NONCE_LEN]);
        let ciphertext = &packet[NONCE_LEN..];

        let decrypted = if let Some(session_key) = &self.session_key {
            let session_cipher = ChaCha20Poly1305::new(Key::from_slice(session_key));
            match session_cipher.decrypt(nonce, ciphertext) {
                Ok(plain) => Ok(plain),
                Err(_) => {
                    let base_cipher = ChaCha20Poly1305::new(Key::from_slice(&self.base_key));
                    base_cipher.decrypt(nonce, ciphertext)
                }
            }
        } else {
            let base_cipher = ChaCha20Poly1305::new(Key::from_slice(&self.base_key));
            base_cipher.decrypt(nonce, ciphertext)
        };

        let decrypted_bytes = decrypted.ok()?;
        if decrypted_bytes.len() < 2 {
            return None;
        }

        let pad_len = decrypted_bytes[decrypted_bytes.len() - 1] as usize;
        if pad_len == 0 || pad_len + 1 > decrypted_bytes.len() {
            return None;
        }

        let plaintext_len = decrypted_bytes.len() - (pad_len + 1);
        Some(decrypted_bytes[0..plaintext_len].to_vec())
    }
}
