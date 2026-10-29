pub mod acl;
pub mod ban_manager;
pub mod daemon;
pub mod history_manager;
pub mod logger;
pub mod panel_api;

pub use acl::ServerAcl;
pub use ban_manager::{BanEntry, BanManager};
pub use daemon::{resolve_server_config_path, ServerDaemon};
pub use history_manager::{HistoryManager, SessionHistoryEntry};
pub use logger::ServerLogger;
pub use panel_api::{ActiveClientSnapshot, PanelApiContext, PanelCommand, ServerConfig};

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, RwLock};
    use mazitunnel_core::protocol::{AuthRequest, Role};

    #[test]
    fn test_ban_manager_add_and_check() {
        let temp_dir = std::env::temp_dir().join(format!("mazitun_ban_test_{}", rand::random::<u32>()));
        let mut ban_mgr = BanManager::new(temp_dir.clone());

        let hwid = "HWID-TEST-1234-5678-ABCD";
        let details = serde_json::json!({
            "motherboard_uuid": "MB-TEST-UUID-12345",
            "machine_guid": "GUID-TEST-1234-5678",
        });

        assert!(ban_mgr.is_banned(hwid, &details).is_none());

        ban_mgr.add_ban(
            hwid,
            "TestUser",
            "PC-TEST",
            "1.2.3.4",
            "Hile kullanimi",
            details.clone(),
        );

        let ban = ban_mgr.is_banned(hwid, &details).expect("Should be banned");
        assert_eq!(ban.hwid, hwid);
        assert_eq!(ban.reason, "Hile kullanimi");

        // Match by motherboard UUID with different fake HWID
        let fake_hwid = "HWID-FAKE-9999-8888-7777";
        let ban_mb = ban_mgr.is_banned(fake_hwid, &details).expect("Should match by motherboard UUID");
        assert_eq!(ban_mb.reason, "Hile kullanimi");

        // Remove ban
        assert!(ban_mgr.remove_ban(hwid));
        assert!(ban_mgr.is_banned(hwid, &details).is_none());

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_history_manager_virtual_ip_allocation() {
        let temp_dir = std::env::temp_dir().join(format!("mazitun_hist_test_{}", rand::random::<u32>()));
        let mut hist_mgr = HistoryManager::new(temp_dir.clone());

        let ip1 = hist_mgr.get_or_assign_virtual_ip("HWID-PLAYER-1111", "Player1");
        assert_eq!(ip1, "100.0.0.2");

        // Same player gets same IP
        let ip1_repeat = hist_mgr.get_or_assign_virtual_ip("HWID-PLAYER-1111", "Player1");
        assert_eq!(ip1_repeat, "100.0.0.2");

        // Second player gets next IP
        let ip2 = hist_mgr.get_or_assign_virtual_ip("HWID-PLAYER-2222", "Player2");
        assert_eq!(ip2, "100.0.0.3");

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_acl_port_isolation_and_hmac() {
        let temp_dir = std::env::temp_dir().join(format!("mazitun_acl_test_{}", rand::random::<u32>()));
        let ban_mgr = Arc::new(RwLock::new(BanManager::new(temp_dir.clone())));

        let player_token = "TOK_TEST_PLAYER_TOKEN";
        let admin_secret = "SecretAdminPassword2026!";
        let game_port = 25565;

        let acl = ServerAcl::new(
            player_token,
            admin_secret,
            game_port,
            "baglanti.local",
            ban_mgr,
        );

        assert!(acl.is_port_allowed(Role::Admin, 22));
        assert!(acl.is_port_allowed(Role::Admin, 25565));
        assert!(acl.is_port_allowed(Role::Admin, 9090));

        assert!(acl.is_port_allowed(Role::Player, 25565));
        assert!(!acl.is_port_allowed(Role::Player, 22));
        assert!(!acl.is_port_allowed(Role::Player, 9090));

        let room_secret = "SEC_ROOM_TEST_SECRET";
        let client_nonce = "random_nonce_123456";

        // Admin HMAC
        let admin_hmac = mazitunnel_core::crypto::compute_auth_hmac(admin_secret, room_secret, client_nonce);
        let req_admin = AuthRequest {
            token_hmac: admin_hmac,
            client_nonce: client_nonce.to_string(),
            version: "1.0.0".to_string(),
            hwid: "HWID-ADMIN-TEST-1234".to_string(),
            hostname: "ADMIN-PC".to_string(),
            username: "AdminUser".to_string(),
            os: "Linux".to_string(),
            hwid_details: serde_json::Value::Null,
            ..Default::default()
        };

        let res_admin = acl.authenticate(&req_admin, room_secret, None).expect("Admin auth failed");
        assert_eq!(res_admin, Role::Admin);

        // Player HMAC
        let player_hmac = mazitunnel_core::crypto::compute_auth_hmac(player_token, room_secret, "nonce_player_999");
        let req_player = AuthRequest {
            token_hmac: player_hmac,
            client_nonce: "nonce_player_999".to_string(),
            version: "1.0.0".to_string(),
            hwid: "HWID-PLAYER-TEST-5678".to_string(),
            hostname: "PLAYER-PC".to_string(),
            username: "PlayerUser".to_string(),
            os: "Windows".to_string(),
            hwid_details: serde_json::Value::Null,
            ..Default::default()
        };

        let res_player = acl.authenticate(&req_player, room_secret, None).expect("Player auth failed");
        assert_eq!(res_player, Role::Player);

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_acl_set_player_token_replaces_old() {
        let temp_dir = std::env::temp_dir().join(format!("mazitun_acl_token_test_{}", rand::random::<u32>()));
        let ban_mgr = Arc::new(RwLock::new(BanManager::new(temp_dir.clone())));

        let acl = ServerAcl::new(
            "TOK_INITIAL_KEY",
            "AdminSecret123!",
            25565,
            "baglanti.local",
            ban_mgr,
        );

        let room_secret = "SEC_ROOM_TEST_SECRET";
        let nonce = "nonce_111222";

        // Initial token works
        let initial_hmac = mazitunnel_core::crypto::compute_auth_hmac("TOK_INITIAL_KEY", room_secret, nonce);
        let req_init = AuthRequest {
            token_hmac: initial_hmac,
            client_nonce: nonce.to_string(),
            version: "2.0.0".to_string(),
            hwid: "HWID-USER-1111".to_string(),
            hostname: "USER-PC".to_string(),
            username: "PlayerOne".to_string(),
            os: "Linux".to_string(),
            hwid_details: serde_json::Value::Null,
            ..Default::default()
        };
        assert!(acl.authenticate(&req_init, room_secret, None).is_ok());

        // Replace token
        acl.set_player_token("TOK_NEW_REGENERATED_KEY");

        // Old token must fail
        let nonce2 = "nonce_333444";
        let old_hmac = mazitunnel_core::crypto::compute_auth_hmac("TOK_INITIAL_KEY", room_secret, nonce2);
        let req_old = AuthRequest {
            token_hmac: old_hmac,
            client_nonce: nonce2.to_string(),
            version: "2.0.0".to_string(),
            hwid: "HWID-USER-1111".to_string(),
            hostname: "USER-PC".to_string(),
            username: "PlayerOne".to_string(),
            os: "Linux".to_string(),
            hwid_details: serde_json::Value::Null,
            ..Default::default()
        };
        assert!(acl.authenticate(&req_old, room_secret, None).is_err());

        // New token must succeed
        let new_hmac = mazitunnel_core::crypto::compute_auth_hmac("TOK_NEW_REGENERATED_KEY", room_secret, nonce2);
        let req_new = AuthRequest {
            token_hmac: new_hmac,
            client_nonce: nonce2.to_string(),
            version: "2.0.0".to_string(),
            hwid: "HWID-USER-1111".to_string(),
            hostname: "USER-PC".to_string(),
            username: "PlayerOne".to_string(),
            os: "Linux".to_string(),
            hwid_details: serde_json::Value::Null,
            ..Default::default()
        };
        assert!(acl.authenticate(&req_new, room_secret, None).is_ok());

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn test_server_logger_summary_and_detailed() {
        let temp_dir = std::env::temp_dir().join(format!("mazitun_logger_test_{}", rand::random::<u32>()));
        let logger = ServerLogger::new(temp_dir.clone(), false);

        logger.log_connect("Mert", false, "1.2.3.4:5555", "100.0.0.2", "HWID-MERT-1234", "Windows 11", "2.0.0");
        logger.log_game_join("Mert", "1.2.3.4:5555", 1, 25565);
        logger.log_disconnect("Mert", 135, 1024, 2048, "1.2.3.4:5555", false);
        logger.log_kick("TrollUser", "5.6.7.8:1234", "Spam");
        logger.log_ban("CheaterUser", "HWID-CHEAT-9999", "Wallhack");
        logger.log_banned_attempt("CheaterUser", "HWID-CHEAT-9999", "5.6.7.8:1234");
        logger.log_key_regenerated("1234-5678-ABCD");
        logger.log_stun("212.253.195.146:25566", false);

        let (summary, detailed) = logger.get_logs();
        assert_eq!(summary.len(), 7);
        assert_eq!(detailed.len(), 8);

        // Verify summary format
        assert!(summary[0].contains("Mert baglandi."));
        assert!(summary[1].contains("Mert oyuna giris yapti."));
        assert!(summary[2].contains("Mert ayrildi (Sure: 2dk 15sn)."));
        assert!(summary[3].contains("TrollUser sunucudan atildi."));
        assert!(summary[4].contains("CheaterUser yasaklandi (Sebep: Wallhack)."));
        assert!(summary[5].contains("Engellenen giris denemesi (CheaterUser - Yasakli cihaz)."));
        assert!(summary[6].contains("Sunucu baglanti kodu yenilendi."));

        // Verify detailed format
        assert!(detailed[0].contains("[AUTH_OK]"));
        assert!(detailed[1].contains("[STREAM_OPEN]"));
        assert!(detailed[2].contains("[SESSION_DISCONNECT]"));
        assert!(detailed[3].contains("[KICK]"));
        assert!(detailed[4].contains("[BAN]"));
        assert!(detailed[5].contains("[AUTH_FAIL]"));
        assert!(detailed[6].contains("[KEY_REGENERATE]"));
        assert!(detailed[7].contains("[STUN]"));

        // Verify files on disk
        let summary_content = std::fs::read_to_string(temp_dir.join("summary.log")).expect("summary.log missing");
        let detailed_content = std::fs::read_to_string(temp_dir.join("detailed.log")).expect("detailed.log missing");

        assert!(summary_content.contains("Mert baglandi."));
        assert!(detailed_content.contains("[AUTH_OK]"));

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[tokio::test]
    async fn test_panel_api_endpoints_and_csrf() {
        use std::collections::HashMap;
        use std::time::Instant;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpStream;
        use tokio::sync::{mpsc, RwLock};

        let temp_dir = std::env::temp_dir().join(format!("mazitun_api_test_{}", rand::random::<u32>()));
        let _ = std::fs::create_dir_all(&temp_dir);

        let config = ServerConfig {
            server_name: "Test Server".to_string(),
            server_key: "1111-2222-3333".to_string(),
            game_port: 25565,
            connection_address: "baglanti.local".to_string(),
            server_udp_port: 25566,
            web_port: 29090,
            player_token: "TOK_TEST".to_string(),
            admin_secret: "AdminSecret123!".to_string(),
            room_secret: "SEC_TEST".to_string(),
            panel_mode: "mazipanel".to_string(),
        };

        let ban_mgr = Arc::new(std::sync::RwLock::new(BanManager::new(temp_dir.clone())));
        let hist_mgr = Arc::new(RwLock::new(HistoryManager::new(temp_dir.clone())));
        let acl = Arc::new(ServerAcl::new("TOK_TEST", "AdminSecret123!", 25565, "baglanti.local", ban_mgr.clone()));
        let logger = Arc::new(ServerLogger::new(temp_dir.clone(), false));
        logger.log_summary("Test log satiri.");

        let (cmd_tx, _cmd_rx) = mpsc::channel(16);

        let ctx = Arc::new(PanelApiContext {
            config_path: temp_dir.join("server_config.json"),
            config: Arc::new(RwLock::new(config)),
            acl,
            ban_manager: ban_mgr,
            history_manager: hist_mgr,
            active_clients: Arc::new(RwLock::new(HashMap::new())),
            public_endpoint: Arc::new(RwLock::new(None)),
            start_time: Instant::now(),
            cmd_tx,
            logger,
        });

        let test_port = 29090;
        tokio::spawn(panel_api::run_panel_api(ctx, test_port));
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // 1. Test GET /api/logs
        let mut stream = TcpStream::connect(("127.0.0.1", test_port)).await.expect("Failed to connect");
        let req = "GET /api/logs HTTP/1.1\r\nHost: 127.0.0.1:29090\r\nConnection: close\r\n\r\n";
        stream.write_all(req.as_bytes()).await.unwrap();
        let mut resp = Vec::new();
        stream.read_to_end(&mut resp).await.unwrap();
        let resp_str = String::from_utf8_lossy(&resp);
        assert!(resp_str.contains("HTTP/1.1 200 OK"));
        assert!(resp_str.contains("Test log satiri."));

        // 2. Test CSRF blocked Origin
        let mut stream_csrf = TcpStream::connect(("127.0.0.1", test_port)).await.expect("Failed to connect");
        let req_csrf = "GET /api/status HTTP/1.1\r\nHost: 127.0.0.1:29090\r\nOrigin: http://malicious-website.com\r\nConnection: close\r\n\r\n";
        stream_csrf.write_all(req_csrf.as_bytes()).await.unwrap();
        let mut resp_csrf = Vec::new();
        stream_csrf.read_to_end(&mut resp_csrf).await.unwrap();
        let resp_csrf_str = String::from_utf8_lossy(&resp_csrf);
        assert!(resp_csrf_str.contains("HTTP/1.1 403 Forbidden"));
        assert!(resp_csrf_str.contains("CSRF"));

        // 3. Test allowed Origin
        let mut stream_ok = TcpStream::connect(("127.0.0.1", test_port)).await.expect("Failed to connect");
        let req_ok = "GET /api/status HTTP/1.1\r\nHost: 127.0.0.1:29090\r\nOrigin: http://localhost:3000\r\nConnection: close\r\n\r\n";
        stream_ok.write_all(req_ok.as_bytes()).await.unwrap();
        let mut resp_ok = Vec::new();
        stream_ok.read_to_end(&mut resp_ok).await.unwrap();
        let resp_ok_str = String::from_utf8_lossy(&resp_ok);
        assert!(resp_ok_str.contains("HTTP/1.1 200 OK"));
        assert!(resp_ok_str.contains("\"summary_logs\""));
        assert!(resp_ok_str.contains("\"detailed_logs\""));

        let _ = std::fs::remove_dir_all(temp_dir);
    }
}
