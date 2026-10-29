use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use anyhow::Result;
use rand::RngCore;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{lookup_host, TcpStream, UdpSocket};
use tokio::time::timeout;

pub const MAGIC_COOKIE: u32 = 0x2112A442;
pub const BINDING_REQUEST: u16 = 0x0001;
pub const BINDING_RESPONSE: u16 = 0x0101;
pub const ATTR_MAPPED_ADDRESS: u16 = 0x0001;
pub const ATTR_XOR_MAPPED_ADDRESS: u16 = 0x0020;

pub const DEFAULT_BROKERS: &[&str] = &[
    "https://ntfy.envs.net",
    "https://ntfy.sh",
];

const STUN_SERVERS: &[&str] = &[
    "stun.cloudflare.com:3478",
    "stun.l.google.com:19302",
    "stun1.l.google.com:19302",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StunEndpoint {
    pub ip: Ipv4Addr,
    pub port: u16,
    pub is_symmetric: bool,
}

impl StunEndpoint {
    pub fn new(ip: Ipv4Addr, port: u16, is_symmetric: bool) -> Self {
        Self {
            ip,
            port,
            is_symmetric,
        }
    }

    pub fn to_socket_addr(&self) -> SocketAddr {
        SocketAddr::V4(SocketAddrV4::new(self.ip, self.port))
    }
}

impl From<SocketAddr> for StunEndpoint {
    fn from(addr: SocketAddr) -> Self {
        match addr {
            SocketAddr::V4(v4) => Self {
                ip: *v4.ip(),
                port: v4.port(),
                is_symmetric: false,
            },
            SocketAddr::V6(_) => Self {
                ip: Ipv4Addr::new(127, 0, 0, 1),
                port: addr.port(),
                is_symmetric: false,
            },
        }
    }
}

pub fn topic_from_key(server_key: &str) -> String {
    if let Ok((room_id, _, _)) = crate::crypto::derive_credentials_from_key(server_key) {
        room_id
    } else {
        let clean = server_key.trim().replace('-', "").replace(' ', "").to_lowercase();
        format!("p2ptun_{}", clean)
    }
}

pub fn get_local_lan_ip() -> String {
    let socket = match std::net::UdpSocket::bind("0.0.0.0:0") {
        Ok(s) => s,
        Err(_) => return "127.0.0.1".to_string(),
    };
    if socket.connect("8.8.8.8:80").is_ok() {
        socket
            .local_addr()
            .map(|a| a.ip().to_string())
            .unwrap_or_else(|_| "127.0.0.1".to_string())
    } else {
        "127.0.0.1".to_string()
    }
}

pub fn is_private_v4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    octets[0] == 10
        || (octets[0] == 172 && (16..=31).contains(&octets[1]))
        || (octets[0] == 192 && octets[1] == 168)
        || ip.is_loopback()
}

pub fn is_lan_addr(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => is_private_v4(v4),
        std::net::IpAddr::V6(v6) => v6.is_loopback(),
    }
}

pub fn create_stun_request() -> ([u8; 20], [u8; 12]) {
    let mut trans_id = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut trans_id);

    let mut packet = [0u8; 20];
    packet[0..2].copy_from_slice(&BINDING_REQUEST.to_be_bytes());
    packet[2..4].copy_from_slice(&0u16.to_be_bytes());
    packet[4..8].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
    packet[8..20].copy_from_slice(&trans_id);

    (packet, trans_id)
}

pub fn parse_stun_response(data: &[u8], expected_tx: Option<&[u8; 12]>) -> Option<SocketAddr> {
    if data.len() < 20 {
        return None;
    }

    let msg_type = u16::from_be_bytes(data[0..2].try_into().ok()?);
    let msg_len = u16::from_be_bytes(data[2..4].try_into().ok()?) as usize;
    let magic = u32::from_be_bytes(data[4..8].try_into().ok()?);

    if msg_type != BINDING_RESPONSE || magic != MAGIC_COOKIE {
        return None;
    }

    if let Some(expected) = expected_tx {
        if &data[8..20] != expected {
            return None;
        }
    }

    let mut offset = 20;
    let end = (20 + msg_len).min(data.len());

    while offset + 4 <= end {
        let attr_type = u16::from_be_bytes(data[offset..offset + 2].try_into().ok()?);
        let attr_len = u16::from_be_bytes(data[offset + 2..offset + 4].try_into().ok()?) as usize;
        offset += 4;

        if offset + attr_len > end {
            break;
        }

        let attr_val = &data[offset..offset + attr_len];
        offset += attr_len;

        if attr_len % 4 != 0 {
            offset += 4 - (attr_len % 4);
        }

        if attr_type == ATTR_XOR_MAPPED_ADDRESS && attr_val.len() >= 8 {
            let family = attr_val[1];
            if family == 1 {
                let xport = u16::from_be_bytes(attr_val[2..4].try_into().ok()?);
                let port = xport ^ ((MAGIC_COOKIE >> 16) as u16);
                let xip = u32::from_be_bytes(attr_val[4..8].try_into().ok()?);
                let ip_u32 = xip ^ MAGIC_COOKIE;
                return Some(SocketAddr::V4(SocketAddrV4::new(
                    Ipv4Addr::from(ip_u32),
                    port,
                )));
            }
        } else if attr_type == ATTR_MAPPED_ADDRESS && attr_val.len() >= 8 {
            let family = attr_val[1];
            if family == 1 {
                let port = u16::from_be_bytes(attr_val[2..4].try_into().ok()?);
                let ip = Ipv4Addr::new(attr_val[4], attr_val[5], attr_val[6], attr_val[7]);
                return Some(SocketAddr::V4(SocketAddrV4::new(ip, port)));
            }
        }
    }

    None
}

pub async fn discover_public_endpoint(sock: &UdpSocket) -> Option<(SocketAddr, bool)> {
    let mut responses = Vec::new();

    for &server in STUN_SERVERS {
        let target_addrs: Vec<SocketAddr> = match lookup_host(server).await {
            Ok(addrs) => addrs.collect(),
            Err(_) => continue,
        };

        for target in target_addrs {
            let (req, tx_id) = create_stun_request();
            if sock.send_to(&req, target).await.is_err() {
                continue;
            }

            let mut buf = [0u8; 1024];
            let recv_res = timeout(Duration::from_millis(1200), sock.recv_from(&mut buf)).await;

            if let Ok(Ok((n, _from_addr))) = recv_res {
                if let Some(endpoint) = parse_stun_response(&buf[..n], Some(&tx_id)) {
                    responses.push(endpoint);
                    break;
                }
            }
        }

        if responses.len() >= 2 {
            break;
        }
    }

    if responses.is_empty() {
        return None;
    }

    let first = responses[0];
    let is_symmetric = if responses.len() >= 2 {
        responses[0].port() != responses[1].port()
    } else {
        false
    };

    Some((first, is_symmetric))
}

pub async fn discover_stun_endpoint(sock: &UdpSocket) -> Option<StunEndpoint> {
    discover_public_endpoint(sock).await.map(|(addr, is_sym)| {
        let ip = match addr {
            SocketAddr::V4(v4) => *v4.ip(),
            SocketAddr::V6(_) => Ipv4Addr::new(127, 0, 0, 1),
        };
        StunEndpoint {
            ip,
            port: addr.port(),
            is_symmetric: is_sym,
        }
    })
}

pub fn predict_symmetric_ports(base_port: u16) -> Vec<u16> {
    let deltas: &[i32] = &[0, 1, 2, -1, 3, 4, 5, 6, 7, 8, -2];
    let mut ports = Vec::with_capacity(deltas.len());

    for &d in deltas {
        let candidate = (base_port as i32) + d;
        if (1024..=65535).contains(&candidate) {
            let p = candidate as u16;
            if !ports.contains(&p) {
                ports.push(p);
            }
        }
    }

    ports
}

#[derive(Clone, Debug)]
pub struct SignalingBroker {
    brokers: Vec<String>,
}

impl SignalingBroker {
    pub fn new(brokers: Vec<String>) -> Self {
        let list = if brokers.is_empty() {
            DEFAULT_BROKERS.iter().map(|s| s.to_string()).collect()
        } else {
            brokers
        };
        Self { brokers: list }
    }

    pub async fn publish(&self, topic: &str, payload: &str) -> Result<bool> {
        for broker in &self.brokers {
            let broker_clean = broker.trim_end_matches('/');

            if broker_clean.starts_with("http://") {
                if let Ok(true) = self.publish_http_pure(broker_clean, topic, payload).await {
                    return Ok(true);
                }
            } else {
                let url = format!("{}/{}", broker_clean, topic);
                let mut cmd = tokio::process::Command::new("curl");
                #[cfg(windows)]
                {
                    cmd.creation_flags(0x08000000);
                }

                let cmd_res = cmd
                    .arg("-s")
                    .arg("-m")
                    .arg("4")
                    .arg("-X")
                    .arg("POST")
                    .arg("-H")
                    .arg("Title: P2P_Signal")
                    .arg("-H")
                    .arg("Priority: urgent")
                    .arg("-A")
                    .arg("P2PTunnel/2.0")
                    .arg("-d")
                    .arg(payload)
                    .arg(&url)
                    .output()
                    .await;

                if let Ok(out) = cmd_res {
                    if out.status.success() {
                        return Ok(true);
                    }
                }
            }
        }

        Ok(false)
    }

    pub async fn poll_messages(&self, topic: &str, _since: &str) -> Result<Vec<String>> {
        for broker in &self.brokers {
            let broker_clean = broker.trim_end_matches('/');

            if broker_clean.starts_with("http://") {
                if let Ok(msgs) = self.poll_http_pure(broker_clean, topic, "60s").await {
                    if !msgs.is_empty() {
                        return Ok(msgs);
                    }
                }
            } else {
                let url = format!("{}/{}/json?poll=1&since=60s", broker_clean, topic);
                let mut cmd = tokio::process::Command::new("curl");
                #[cfg(windows)]
                {
                    cmd.creation_flags(0x08000000);
                }

                let cmd_res = cmd
                    .arg("-s")
                    .arg("-m")
                    .arg("5")
                    .arg("-A")
                    .arg("P2PTunnel/2.0")
                    .arg(&url)
                    .output()
                    .await;

                if let Ok(out) = cmd_res {
                    if out.status.success() {
                        let text = String::from_utf8_lossy(&out.stdout);
                        let mut msgs = Vec::new();
                        for line in text.lines() {
                            let line = line.trim();
                            if line.is_empty() {
                                continue;
                            }
                            if let Ok(val) = serde_json::from_str::<Value>(line) {
                                if val.get("event").and_then(|v| v.as_str()) == Some("message") {
                                    if let Some(msg_body) = val.get("message").and_then(|v| v.as_str()) {
                                        msgs.push(msg_body.to_string());
                                    }
                                }
                            } else {
                                msgs.push(line.to_string());
                            }
                        }
                        if !msgs.is_empty() {
                            return Ok(msgs);
                        }
                    }
                }
            }
        }

        Ok(Vec::new())
    }

    async fn publish_http_pure(&self, broker_clean: &str, topic: &str, payload: &str) -> Result<bool> {
        let uri = broker_clean.trim_start_matches("http://");
        let (host, port) = if let Some((h, p)) = uri.split_once(':') {
            (h, p.parse::<u16>().unwrap_or(80))
        } else {
            (uri, 80)
        };

        let addr = format!("{}:{}", host, port);
        let mut stream = timeout(Duration::from_millis(2500), TcpStream::connect(&addr)).await??;

        let path = format!("/{}", topic);
        let req = format!(
            "POST {} HTTP/1.1\r\nHost: {}\r\nTitle: P2P_Signal\r\nPriority: urgent\r\nUser-Agent: P2PTunnel/2.0\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            path, host, payload.len(), payload
        );

        stream.write_all(req.as_bytes()).await?;

        let mut resp = Vec::new();
        timeout(Duration::from_millis(2500), stream.read_to_end(&mut resp)).await??;

        let resp_str = String::from_utf8_lossy(&resp);
        Ok(resp_str.starts_with("HTTP/1.1 200") || resp_str.starts_with("HTTP/1.0 200"))
    }

    async fn poll_http_pure(&self, broker_clean: &str, topic: &str, _since: &str) -> Result<Vec<String>> {
        let uri = broker_clean.trim_start_matches("http://");
        let (host, port) = if let Some((h, p)) = uri.split_once(':') {
            (h, p.parse::<u16>().unwrap_or(80))
        } else {
            (uri, 80)
        };

        let addr = format!("{}:{}", host, port);
        let mut stream = timeout(Duration::from_millis(3000), TcpStream::connect(&addr)).await??;

        let path = format!("/{}/json?poll=1&since=60s", topic);
        let req = format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: P2PTunnel/2.0\r\nConnection: close\r\n\r\n",
            path, host
        );

        stream.write_all(req.as_bytes()).await?;

        let mut resp = Vec::new();
        timeout(Duration::from_millis(4000), stream.read_to_end(&mut resp)).await??;

        let resp_str = String::from_utf8_lossy(&resp);
        let body = if let Some((_, b)) = resp_str.split_once("\r\n\r\n") {
            b
        } else {
            &resp_str
        };

        let mut msgs = Vec::new();
        for line in body.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(val) = serde_json::from_str::<Value>(line) {
                if val.get("event").and_then(|v| v.as_str()) == Some("message") {
                    if let Some(msg_body) = val.get("message").and_then(|v| v.as_str()) {
                        msgs.push(msg_body.to_string());
                    }
                }
            } else {
                msgs.push(line.to_string());
            }
        }

        Ok(msgs)
    }
}

pub async fn publish_signal(topic: &str, payload: &str) -> bool {
    let broker = SignalingBroker::new(vec![]);
    broker.publish(topic, payload).await.unwrap_or(false)
}

pub async fn poll_signals(topic: &str) -> Vec<String> {
    let broker = SignalingBroker::new(vec![]);
    broker.poll_messages(topic, "60s").await.unwrap_or_default()
}

pub async fn rendezvous_client(server_key: &str, my_ep: &StunEndpoint) -> Vec<SocketAddr> {
    rendezvous_client_full(server_key, my_ep, my_ep.port).await
}

pub async fn rendezvous_client_full(
    server_key: &str,
    my_ep: &StunEndpoint,
    local_port: u16,
) -> Vec<SocketAddr> {
    let (room_id, room_secret, _) = match crate::crypto::derive_credentials_from_key(server_key) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let cipher = crate::crypto::PacketCipher::new(&crate::crypto::derive_key(&room_secret), true);
    let my_client_id = {
        let mut cid_bytes = [0u8; 4];
        rand::thread_rng().fill_bytes(&mut cid_bytes);
        hex::encode(cid_bytes)
    };
    let my_lan_ip = get_local_lan_ip();
    let start_ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();

    let parse_signals = |msgs: &[String]| -> Vec<SocketAddr> {
        let mut addrs = Vec::new();
        for line in msgs.iter().rev() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let msg_text = if let Ok(val) = serde_json::from_str::<Value>(line) {
                if val.get("event").and_then(|v| v.as_str()) == Some("message") {
                    val.get("message").and_then(|v| v.as_str()).unwrap_or("").to_string()
                } else {
                    line.to_string()
                }
            } else {
                line.to_string()
            };

            let decrypted_opt = if let Ok(bytes) = hex::decode(msg_text.trim()) {
                cipher.decrypt(&bytes)
            } else {
                None
            };

            let json_val = if let Some(ref dec) = decrypted_opt {
                serde_json::from_slice::<Value>(dec).ok()
            } else {
                serde_json::from_str::<Value>(&msg_text).ok()
            };

            if let Some(msg) = json_val {
                if let Some(cid) = msg.get("cid").or_else(|| msg.get("client_id")).and_then(|v| v.as_str()) {
                    if !cid.is_empty() && cid != my_client_id {
                        continue;
                    }
                }
                let mtype = msg.get("type").and_then(|v| v.as_str()).unwrap_or("");
                if mtype == "server_hello" || mtype == "server_locked" {
                    let sip = msg.get("ip").or_else(|| msg.get("sip")).and_then(|v| v.as_str()).unwrap_or("");
                    let sport = msg.get("port").or_else(|| msg.get("sport")).and_then(|v| v.as_u64()).unwrap_or(0) as u16;
                    let slan = msg.get("lan_ip").or_else(|| msg.get("slan")).and_then(|v| v.as_str()).unwrap_or("");
                    let lport = msg.get("local_port").or_else(|| msg.get("lport")).and_then(|v| v.as_u64()).unwrap_or(0) as u16;

                    if !slan.is_empty() && lport > 0 {
                        let mut local_addrs = Vec::new();
                        if let Ok(addr) = format!("{}:{}", slan, lport).parse() {
                            local_addrs.push(addr);
                        }
                        if let Ok(addr) = format!("127.0.0.1:{}", lport).parse() {
                            if !local_addrs.contains(&addr) {
                                local_addrs.push(addr);
                            }
                        }
                        for addr in local_addrs.into_iter().rev() {
                            addrs.retain(|a| a != &addr);
                            addrs.insert(0, addr);
                        }
                    }
                    if !sip.is_empty() && sport > 0 {
                        if let Ok(addr) = format!("{}:{}", sip, sport).parse() {
                            if !addrs.contains(&addr) {
                                addrs.push(addr);
                            }
                        }
                    }
                    if !sip.is_empty() && lport > 0 && lport != sport {
                        if let Ok(addr) = format!("{}:{}", sip, lport).parse() {
                            if !addrs.contains(&addr) {
                                addrs.push(addr);
                            }
                        }
                    }
                }
            } else {
                let server_str = if let Some(ref dec) = decrypted_opt {
                    std::str::from_utf8(dec).ok().and_then(|s| s.strip_prefix("SERVER:").map(|r| r.to_string()))
                } else {
                    msg_text.strip_prefix("SERVER:").map(|r| r.to_string())
                };

                if let Some(rest) = server_str {
                    for part in rest.split('|') {
                        if let Ok(addr) = part.trim().parse::<SocketAddr>() {
                            if is_lan_addr(addr.ip()) {
                                addrs.retain(|a| a != &addr);
                                addrs.insert(0, addr);
                            } else if !addrs.contains(&addr) {
                                addrs.push(addr);
                            }
                        }
                    }
                }
            }
        }
        addrs
    };

    let msg_id_hex = {
        let mut msg_id_bytes = [0u8; 4];
        rand::thread_rng().fill_bytes(&mut msg_id_bytes);
        hex::encode(msg_id_bytes)
    };

    let client_hello = serde_json::json!({
        "type": "client_hello",
        "client_id": my_client_id,
        "ip": my_ep.ip.to_string(),
        "port": my_ep.port,
        "lan_ip": my_lan_ip,
        "local_port": local_port,
        "ts": start_ts,
        "msg_id": msg_id_hex,
    });
    let enc_body = cipher.encrypt(client_hello.to_string().as_bytes());
    let hex_body = hex::encode(enc_body);

    let _ = publish_signal(&room_id, &hex_body).await;

    for attempt in 0..12 {
        if attempt > 0 && attempt % 3 == 0 {
            let _ = publish_signal(&room_id, &hex_body).await;
        }

        let msgs = poll_signals(&room_id).await;
        let candidates = parse_signals(&msgs);
        if !candidates.is_empty() {
            return candidates;
        }

        tokio::time::sleep(Duration::from_millis(900)).await;
    }

    Vec::new()
}

pub async fn punch_udp_client_encrypted(
    udp: &tokio::net::UdpSocket,
    candidates: &[SocketAddr],
    server_key: &str,
    duration_ms: u64,
) -> Option<SocketAddr> {
    let (_, room_secret, _) = match crate::crypto::derive_credentials_from_key(server_key) {
        Ok(c) => c,
        Err(_) => return None,
    };
    let cipher = crate::crypto::PacketCipher::new(&crate::crypto::derive_key(&room_secret), true);

    let syn_pkt = cipher.encrypt(b"P2P_TUN_PUNCH_SYN");
    let ack_pkt = cipher.encrypt(b"P2P_TUN_PUNCH_ACK");

    let mut lan_targets = Vec::new();
    let mut wan_candidates = Vec::new();

    for &cand in candidates {
        if is_lan_addr(cand.ip()) {
            if !lan_targets.contains(&cand) {
                lan_targets.push(cand);
            }
        } else if !wan_candidates.contains(&cand) {
            wan_candidates.push(cand);
        }
    }

    let mut wan_targets = wan_candidates.clone();
    for &cand in &wan_candidates {
        let base_port = cand.port() as i32;
        for delta in -8..=24 {
            let p = base_port + delta;
            if (1024..=65535).contains(&p) {
                let mut a = cand;
                a.set_port(p as u16);
                if !wan_targets.contains(&a) {
                    wan_targets.push(a);
                }
            }
        }
    }

    let mut buf = [0u8; 1024];

    // Fast LAN First Phase: If !lan_targets.is_empty(), run 4 fast probe rounds (25ms timeout per round, total ~100ms) sending syn_pkt ONLY to lan_targets!
    if !lan_targets.is_empty() {
        for _ in 0..4 {
            for &target in &lan_targets {
                let _ = udp.send_to(&syn_pkt, target).await;
            }

            let round_deadline = Instant::now() + Duration::from_millis(25);
            while Instant::now() < round_deadline {
                let rem = round_deadline.saturating_duration_since(Instant::now());
                if rem.is_zero() {
                    break;
                }
                if let Ok(Ok((len, addr))) = timeout(rem, udp.recv_from(&mut buf)).await {
                    let raw = &buf[..len];
                    let is_match = if let Some(plain) = cipher.decrypt(raw) {
                        plain == b"P2P_TUN_PUNCH_SYN" || plain == b"P2P_TUN_PUNCH_ACK"
                    } else {
                        raw.starts_with(b"MAZITUN_PUNCH") || raw.starts_with(b"P2P_TUN_PUNCH")
                    };

                    if is_match {
                        for _ in 0..2 {
                            let _ = udp.send_to(&ack_pkt, addr).await;
                        }
                        return Some(addr);
                    }
                } else {
                    break;
                }
            }
        }
    }

    let start = Instant::now();
    let max_dur = Duration::from_millis(duration_ms);
    let mut all_targets = lan_targets;
    for wt in wan_targets {
        if !all_targets.contains(&wt) {
            all_targets.push(wt);
        }
    }

    while start.elapsed() < max_dur {
        for &target in &all_targets {
            let _ = udp.send_to(&syn_pkt, target).await;
        }

        for _ in 0..5 {
            let res = timeout(Duration::from_millis(20), udp.recv_from(&mut buf)).await;
            if let Ok(Ok((len, addr))) = res {
                let raw = &buf[..len];
                let is_match = if let Some(plain) = cipher.decrypt(raw) {
                    plain == b"P2P_TUN_PUNCH_SYN" || plain == b"P2P_TUN_PUNCH_ACK"
                } else {
                    raw.starts_with(b"MAZITUN_PUNCH") || raw.starts_with(b"P2P_TUN_PUNCH")
                };

                if is_match {
                    for _ in 0..4 {
                        let _ = udp.send_to(&ack_pkt, addr).await;
                    }
                    return Some(addr);
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(40)).await;
    }

    candidates.first().copied()
}

pub async fn rendezvous_server(
    server_key: &str,
    my_ep: &StunEndpoint,
    local_port: u16,
) -> Vec<SocketAddr> {
    let (room_id, room_secret, _) = match crate::crypto::derive_credentials_from_key(server_key) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let cipher = crate::crypto::PacketCipher::new(&crate::crypto::derive_key(&room_secret), true);
    let my_lan_ip = get_local_lan_ip();
    let now_ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();

    let server_hello = serde_json::json!({
        "type": "server_hello",
        "ip": my_ep.ip.to_string(),
        "port": my_ep.port,
        "lan_ip": my_lan_ip,
        "local_port": local_port,
        "ts": now_ts,
    });
    let enc_body = cipher.encrypt(server_hello.to_string().as_bytes());
    let hex_body = hex::encode(enc_body);

    let _ = publish_signal(&room_id, &hex_body).await;

    let msgs = poll_signals(&room_id).await;
    let mut client_addrs = Vec::new();

    for line in msgs {
        let msg_text = if let Ok(val) = serde_json::from_str::<Value>(&line) {
            if val.get("event").and_then(|v| v.as_str()) == Some("message") {
                val.get("message").and_then(|v| v.as_str()).unwrap_or("").to_string()
            } else {
                line.clone()
            }
        } else {
            line.clone()
        };

        let decrypted_opt = if let Ok(bytes) = hex::decode(msg_text.trim()) {
            cipher.decrypt(&bytes)
        } else {
            None
        };

        let json_val = if let Some(ref dec) = decrypted_opt {
            serde_json::from_slice::<Value>(dec).ok()
        } else {
            serde_json::from_str::<Value>(&msg_text).ok()
        };

        if let Some(msg) = json_val {
            let mtype = msg.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if mtype == "client_hello" {
                let cip = msg.get("ip").and_then(|v| v.as_str()).unwrap_or("");
                let cport = msg.get("port").and_then(|v| v.as_u64()).unwrap_or(0) as u16;
                if !cip.is_empty() && cport > 0 {
                    if let Ok(addr) = format!("{}:{}", cip, cport).parse() {
                        if !client_addrs.contains(&addr) {
                            client_addrs.push(addr);
                        }
                    }
                }
            }
        }
    }

    client_addrs
}

pub async fn punch_udp_server(
    udp: &tokio::net::UdpSocket,
    client_candidates: &[SocketAddr],
    server_key: &str,
    duration_ms: u64,
) -> Option<SocketAddr> {
    let (_, room_secret, _) = match crate::crypto::derive_credentials_from_key(server_key) {
        Ok(c) => c,
        Err(_) => return None,
    };
    let cipher = crate::crypto::PacketCipher::new(&crate::crypto::derive_key(&room_secret), true);

    let syn_pkt = cipher.encrypt(b"P2P_TUN_PUNCH_SYN");
    let ack_pkt = cipher.encrypt(b"P2P_TUN_PUNCH_ACK");

    let start = Instant::now();
    let max_dur = Duration::from_millis(duration_ms);
    let mut buf = [0u8; 1024];

    while start.elapsed() < max_dur {
        for &target in client_candidates {
            let _ = udp.send_to(&syn_pkt, target).await;
        }

        for _ in 0..5 {
            let res = timeout(Duration::from_millis(20), udp.recv_from(&mut buf)).await;
            if let Ok(Ok((len, addr))) = res {
                let raw = &buf[..len];
                let is_match = if let Some(plain) = cipher.decrypt(raw) {
                    plain == b"P2P_TUN_PUNCH_SYN" || plain == b"P2P_TUN_PUNCH_ACK"
                } else {
                    raw.starts_with(b"MAZITUN_PUNCH") || raw.starts_with(b"P2P_TUN_PUNCH")
                };

                if is_match {
                    for _ in 0..4 {
                        let _ = udp.send_to(&ack_pkt, addr).await;
                    }
                    return Some(addr);
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(40)).await;
    }

    client_candidates.first().copied()
}

pub async fn discover_lan_server(
    udp: &tokio::net::UdpSocket,
    server_key: &str,
    default_port: u16,
) -> Option<SocketAddr> {
    let (_, room_secret, _) = match crate::crypto::derive_credentials_from_key(server_key) {
        Ok(c) => c,
        Err(_) => return None,
    };
    let cipher = crate::crypto::PacketCipher::new(&crate::crypto::derive_key(&room_secret), true);
    let _ = udp.set_broadcast(true);

    let mut targets = Vec::new();
    if let Ok(addr) = format!("127.0.0.1:{}", default_port).parse::<SocketAddr>() {
        targets.push(addr);
    }
    if let Ok(addr) = format!("255.255.255.255:{}", default_port).parse::<SocketAddr>() {
        targets.push(addr);
    }

    let lan_ip_str = get_local_lan_ip();
    if let Ok(ip) = lan_ip_str.parse::<Ipv4Addr>() {
        if !ip.is_loopback() && !ip.is_unspecified() {
            let oct = ip.octets();
            let bcast = Ipv4Addr::new(oct[0], oct[1], oct[2], 255);
            let bcast_addr = SocketAddr::V4(SocketAddrV4::new(bcast, default_port));
            if !targets.contains(&bcast_addr) {
                targets.push(bcast_addr);
            }
        }
    }

    let syn_pkt = cipher.encrypt(b"P2P_TUN_PUNCH_SYN");
    let ack_pkt = cipher.encrypt(b"P2P_TUN_PUNCH_ACK");
    let mut buf = [0u8; 1024];

    for _ in 0..3 {
        for &target in &targets {
            let _ = udp.send_to(&syn_pkt, target).await;
        }

        let round_start = Instant::now();
        let round_dur = Duration::from_millis(60);

        while round_start.elapsed() < round_dur {
            let remaining = round_dur.saturating_sub(round_start.elapsed());
            if remaining.is_zero() {
                break;
            }
            if let Ok(Ok((len, addr))) = timeout(remaining, udp.recv_from(&mut buf)).await {
                let raw = &buf[..len];
                let is_match = if let Some(plain) = cipher.decrypt(raw) {
                    plain == b"P2P_TUN_PUNCH_SYN" || plain == b"P2P_TUN_PUNCH_ACK"
                } else {
                    false
                };

                if is_match {
                    for _ in 0..2 {
                        let _ = udp.send_to(&ack_pkt, addr).await;
                    }
                    return Some(addr);
                }
            }
        }
    }

    None
}
