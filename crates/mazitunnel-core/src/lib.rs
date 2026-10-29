pub mod crypto;
pub mod hwid;
pub mod kcp_engine;
pub mod nat_traversal;
pub mod paths;
pub mod platform;
pub mod protocol;

pub use crypto::{
    compute_auth_hmac, compute_server_proof, derive_credentials_from_key, derive_key,
    derive_session_key, derive_session_key_with_token, PacketCipher,
};
pub use protocol::{
    AuthRequest, AuthResponse, Cmd, Frame, KcpEngine, Role, FRAME_HEADER_SIZE,
};

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use std::net::{Ipv4Addr, SocketAddr};

    #[test]
    fn test_protocol_frame_encode_decode() {
        let original_payload = Bytes::from_static(b"Hello Minecraft World Protocol Test!");
        let frame = protocol::Frame::new(
            protocol::Cmd::Data,
            101,
            25565,
            original_payload.clone(),
        );

        let encoded = frame.encode();
        assert_eq!(encoded.len(), protocol::FRAME_HEADER_SIZE + original_payload.len());

        let (decoded, consumed) = protocol::Frame::decode(&encoded).expect("Frame decode failed");
        assert_eq!(consumed, encoded.len());
        assert_eq!(decoded.cmd, protocol::Cmd::Data);
        assert_eq!(decoded.stream_id, 101);
        assert_eq!(decoded.target_port, 25565);
        assert_eq!(decoded.payload, original_payload);
    }

    #[test]
    fn test_protocol_empty_payload_frame() {
        let frame = protocol::Frame::new(
            protocol::Cmd::Ping,
            1,
            0,
            Bytes::new(),
        );

        let encoded = frame.encode();
        assert_eq!(encoded.len(), protocol::FRAME_HEADER_SIZE);

        let (decoded, consumed) = protocol::Frame::decode(&encoded).expect("Empty frame decode failed");
        assert_eq!(consumed, protocol::FRAME_HEADER_SIZE);
        assert_eq!(decoded.cmd, protocol::Cmd::Ping);
        assert_eq!(decoded.stream_id, 1);
        assert_eq!(decoded.target_port, 0);
        assert!(decoded.payload.is_empty());
    }

    #[test]
    fn test_role_and_cmd() {
        assert_eq!(protocol::Role::Player.as_str(), "PLAYER");
        assert_eq!(protocol::Role::Admin.as_str(), "ADMIN");
        assert_eq!(protocol::Role::from_str_role("player"), Some(protocol::Role::Player));
        assert_eq!(protocol::Role::from_str_role("ADMIN"), Some(protocol::Role::Admin));

        assert_eq!(protocol::Cmd::from_u8(1), Some(protocol::Cmd::Connect));
        assert_eq!(protocol::Cmd::from_u8(2), Some(protocol::Cmd::Data));
        assert_eq!(protocol::Cmd::from_u8(3), Some(protocol::Cmd::Close));
        assert_eq!(protocol::Cmd::from_u8(4), Some(protocol::Cmd::Ping));
        assert_eq!(protocol::Cmd::from_u8(5), Some(protocol::Cmd::Pong));
        assert_eq!(protocol::Cmd::from_u8(6), Some(protocol::Cmd::Auth));
        assert_eq!(protocol::Cmd::from_u8(7), Some(protocol::Cmd::AuthOk));
        assert_eq!(protocol::Cmd::from_u8(8), Some(protocol::Cmd::AuthFail));
        assert_eq!(protocol::Cmd::from_u8(9), Some(protocol::Cmd::Disconnect));
        assert_eq!(protocol::Cmd::from_u8(250), None);
    }

    #[test]
    fn test_crypto_key_and_credentials() {
        let key = crypto::generate_server_key();
        assert_eq!(key.len(), 14);
        assert_eq!(&key[4..5], "-");
        assert_eq!(&key[9..10], "-");

        let normalized = crypto::normalize_server_key("MC-1A2B-3C4D-5E6F");
        assert_eq!(normalized, "1A2B-3C4D-5E6F");

        let (room_id, room_secret, player_token) =
            crypto::derive_credentials_from_key("1A2B-3C4D-5E6F").expect("Valid key");
        assert!(room_id.starts_with("p2ptun_"));
        assert!(room_secret.starts_with("SEC_"));
        assert!(player_token.starts_with("TOK_"));
    }

    #[test]
    fn test_crypto_short_code_roundtrip() {
        let ip = Ipv4Addr::new(172, 20, 10, 5);
        let port = 25565;

        let code = crypto::ip_port_to_short_code(ip, port);
        assert_eq!(code.len(), 14);

        let (decoded_ip, decoded_port) =
            crypto::short_code_to_ip_port(&code).expect("Short code decoding failed");
        assert_eq!(ip, decoded_ip);
        assert_eq!(port, decoded_port);
    }

    #[test]
    fn test_crypto_packet_cipher_encryption_roundtrip() {
        let base_key = crypto::derive_key("room-secret-test-seed");
        let mut cipher = crypto::PacketCipher::new(&base_key, true);

        let plaintext = b"Super secret Minecraft payload packet data 1234567890";
        let encrypted1 = cipher.encrypt(plaintext);
        let encrypted2 = cipher.encrypt(plaintext);

        // Dynamic padding & random nonce guarantee non-identical ciphertexts
        assert_ne!(encrypted1, encrypted2);

        let decrypted1 = cipher.decrypt(&encrypted1).expect("Decryption 1 failed");
        let decrypted2 = cipher.decrypt(&encrypted2).expect("Decryption 2 failed");

        assert_eq!(decrypted1, plaintext);
        assert_eq!(decrypted2, plaintext);

        // Session key upgrade and fallback
        let session_key = crypto::derive_session_key(
            "room-secret-test-seed",
            "client_nonce_123",
            "server_nonce_456",
        );
        cipher.set_session_key(session_key);

        let session_enc = cipher.encrypt(plaintext);
        let session_dec = cipher.decrypt(&session_enc).expect("Session decryption failed");
        assert_eq!(session_dec, plaintext);

        // Decrypt old packet encrypted with base_key still succeeds via fallback
        let old_dec = cipher.decrypt(&encrypted1).expect("Base key fallback decryption failed");
        assert_eq!(old_dec, plaintext);
    }

    #[test]
    fn test_kcp_engine_single_stream_transmission() {
        let mut sender = protocol::KcpEngine::new(2026);
        let mut receiver = protocol::KcpEngine::new(2026);

        let test_frame = protocol::Frame::new(
            protocol::Cmd::Data,
            101,
            25565,
            Bytes::from_static(b"HELLO_KCP_2026"),
        );
        sender.send(&test_frame.encode());
        let dgrams = sender.flush(100);
        assert!(!dgrams.is_empty());

        for dg in dgrams {
            let acks = receiver.input(&dg, 105);
            for ack in acks {
                let _ = sender.input(&ack, 110);
            }
        }

        let mut buf = Vec::new();
        while let Some(chunk) = receiver.recv() {
            buf.extend_from_slice(&chunk);
        }
        let (decoded, consumed) = protocol::Frame::decode(&buf).expect("Decode frame");
        assert_eq!(consumed, buf.len());
        assert_eq!(decoded.cmd, protocol::Cmd::Data);
        assert_eq!(decoded.stream_id, 101);
        assert_eq!(decoded.target_port, 25565);
        assert_eq!(decoded.payload, Bytes::from_static(b"HELLO_KCP_2026"));
    }

    #[test]
    fn test_stun_packet_encode_and_parse() {
        let (req, tx_id) = nat_traversal::create_stun_request();
        assert_eq!(req.len(), 20);
        assert_eq!(&req[8..20], &tx_id);

        let target_ip = Ipv4Addr::new(198, 51, 100, 1);
        let target_port: u16 = 45678;

        let mut resp = Vec::new();
        resp.extend_from_slice(&nat_traversal::BINDING_RESPONSE.to_be_bytes());
        resp.extend_from_slice(&12u16.to_be_bytes());
        resp.extend_from_slice(&nat_traversal::MAGIC_COOKIE.to_be_bytes());
        resp.extend_from_slice(&tx_id);

        resp.extend_from_slice(&nat_traversal::ATTR_XOR_MAPPED_ADDRESS.to_be_bytes());
        resp.extend_from_slice(&8u16.to_be_bytes());
        resp.push(0);
        resp.push(1);
        let xport = target_port ^ ((nat_traversal::MAGIC_COOKIE >> 16) as u16);
        resp.extend_from_slice(&xport.to_be_bytes());
        let xip = u32::from(target_ip) ^ nat_traversal::MAGIC_COOKIE;
        resp.extend_from_slice(&xip.to_be_bytes());

        let parsed = nat_traversal::parse_stun_response(&resp, Some(&tx_id))
            .expect("Failed to parse STUN response");
        assert_eq!(parsed, SocketAddr::new(target_ip.into(), target_port));
    }

    #[test]
    fn test_paths_normalize_domain() {
        assert_eq!(paths::normalize_local_domain(""), "baglanti.local");
        assert_eq!(paths::normalize_local_domain("   "), "baglanti.local");
        assert_eq!(paths::normalize_local_domain("127.0.0.1"), "127.0.0.1");
        assert_eq!(paths::normalize_local_domain("localhost"), "localhost");
        assert_eq!(paths::normalize_local_domain("xeptra.localhost"), "xeptra.local");
        assert_eq!(paths::normalize_local_domain("isim.local.localhost"), "isim.local");
        assert_eq!(paths::normalize_local_domain("sunucu.local"), "sunucu.local");
        assert_eq!(paths::normalize_local_domain("minecraft"), "minecraft.local");
        assert_eq!(paths::normalize_local_domain("game.internal.org"), "game.internal.org");
    }

    #[test]
    fn test_hwid_identity_generation() {
        let ident = hwid::get_client_identity("1.0.0");
        assert!(ident.hwid.starts_with("HWID-"));
        assert!(ident.hwid.len() >= 21);
        assert_eq!(ident.version, "1.0.0");
        assert!(!ident.hostname.is_empty());
        assert!(!ident.username.is_empty());
        assert!(!ident.os.is_empty());
    }
}
