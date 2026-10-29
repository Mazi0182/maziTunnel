use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tracing::{debug, info, warn};

pub const MDNS_PORT: u16 = 5353;
pub const LLMNR_PORT: u16 = 5355;
pub const MINECRAFT_LAN_PORT: u16 = 4445;

pub const MDNS_IPV4: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
pub const LLMNR_IPV4: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 252);
pub const MINECRAFT_LAN_IPV4: Ipv4Addr = Ipv4Addr::new(224, 0, 2, 60);

pub const DEFAULT_DOMAIN: &str = "baglanti.local";
pub const DEFAULT_SERVER_NAME: &str = "maziTunnel";
pub const DEFAULT_GAME_PORT: u16 = 25565;

#[derive(Debug, Clone)]
pub struct DnsQuery {
    pub id: u16,
    pub flags: u16,
    pub is_mdns: bool,
    pub qname: String,
    pub qtype: u16,
    pub qclass: u16,
    pub question_bytes: Vec<u8>,
}

pub struct LocalDomainInterceptor {
    active_domain: Arc<RwLock<String>>,
    active_server_name: Arc<RwLock<String>>,
    active_game_port: Arc<AtomicU16>,
    is_tunnel_active: Arc<AtomicBool>,
}

impl Default for LocalDomainInterceptor {
    fn default() -> Self {
        Self::new(DEFAULT_DOMAIN, DEFAULT_SERVER_NAME, DEFAULT_GAME_PORT)
    }
}

impl LocalDomainInterceptor {
    pub fn new(domain: &str, server_name: &str, game_port: u16) -> Self {
        let interceptor = Self {
            active_domain: Arc::new(RwLock::new(Self::normalize_domain(domain))),
            active_server_name: Arc::new(RwLock::new(server_name.to_string())),
            active_game_port: Arc::new(AtomicU16::new(game_port)),
            is_tunnel_active: Arc::new(AtomicBool::new(false)),
        };
        interceptor
    }

    pub fn normalize_domain(domain: &str) -> String {
        let trimmed = domain.trim().to_lowercase();
        if trimmed.is_empty() {
            return DEFAULT_DOMAIN.to_string();
        }

        if trimmed.ends_with(".localhost") {
            let prefix = trimmed.trim_end_matches(".localhost");
            if prefix.is_empty() {
                "localhost".to_string()
            } else {
                format!("{}.local", prefix)
            }
        } else if trimmed.ends_with(".local") {
            trimmed
        } else if trimmed == "localhost" || trimmed == "127.0.0.1" {
            trimmed
        } else {
            format!("{}.local", trimmed)
        }
    }

    pub fn set_active_domain(&self, domain: &str) {
        let normalized = Self::normalize_domain(domain);
        info!("Yerel isim cozucu aktif alan adi guncellendi: {}", normalized);
        if let Ok(mut lock) = self.active_domain.write() {
            *lock = normalized;
        }
    }

    pub fn active_domain(&self) -> String {
        self.active_domain
            .read()
            .map(|d| d.clone())
            .unwrap_or_else(|_| DEFAULT_DOMAIN.to_string())
    }

    pub fn set_server_name(&self, name: &str) {
        if let Ok(mut lock) = self.active_server_name.write() {
            *lock = name.trim().to_string();
        }
    }

    pub fn active_server_name(&self) -> String {
        self.active_server_name
            .read()
            .map(|n| n.clone())
            .unwrap_or_else(|_| DEFAULT_SERVER_NAME.to_string())
    }

    pub fn set_game_port(&self, port: u16) {
        self.active_game_port.store(port, Ordering::SeqCst);
    }

    pub fn active_game_port(&self) -> u16 {
        self.active_game_port.load(Ordering::Relaxed)
    }

    pub fn set_tunnel_active(&self, active: bool) {
        self.is_tunnel_active.store(active, Ordering::SeqCst);
    }

    pub fn is_tunnel_active(&self) -> bool {
        self.is_tunnel_active.load(Ordering::Relaxed)
    }

    pub fn matches_query(&self, qname: &str) -> bool {
        let clean = qname.trim().trim_end_matches('.').to_lowercase();
        if clean.is_empty() {
            return false;
        }

        let cur_domain = self.active_domain();
        let cur_clean = cur_domain.trim_end_matches('.').to_lowercase();

        if clean == cur_clean {
            return true;
        }

        if clean == DEFAULT_DOMAIN || clean == "baglanti" || clean == "baglanti.localhost" {
            return true;
        }

        let cur_bare = if cur_clean.ends_with(".local") {
            cur_clean.trim_end_matches(".local")
        } else if cur_clean.ends_with(".localhost") {
            cur_clean.trim_end_matches(".localhost")
        } else {
            &cur_clean
        };

        if !cur_bare.is_empty() {
            if clean == cur_bare {
                return true;
            }
            if clean == format!("{}.localhost", cur_bare) {
                return true;
            }
            if clean == format!("{}.local", cur_bare) {
                return true;
            }
        }

        let srv_name = self.active_server_name().to_lowercase();
        if !srv_name.is_empty() {
            if clean == srv_name {
                return true;
            }
            if clean == format!("{}.local", srv_name) {
                return true;
            }
            if clean == format!("{}.localhost", srv_name) {
                return true;
            }
        }

        false
    }

    pub fn start_background_tasks(self: &Arc<Self>) {
        let self_mdns = Arc::clone(self);
        tokio::spawn(async move {
            self_mdns.run_mdns_listener().await;
        });

        let self_llmnr = Arc::clone(self);
        tokio::spawn(async move {
            self_llmnr.run_llmnr_listener().await;
        });

        let self_lan = Arc::clone(self);
        tokio::spawn(async move {
            self_lan.run_minecraft_lan_broadcaster().await;
        });
    }

    async fn run_mdns_listener(&self) {
        let socket = match create_multicast_socket(MDNS_PORT, MDNS_IPV4) {
            Ok(s) => s,
            Err(e) => {
                warn!("mDNS soketi acilamadi (Port 5353 mesgul olabilir): {}", e);
                return;
            }
        };

        let tokio_sock = match tokio::net::UdpSocket::from_std(socket.into()) {
            Ok(s) => s,
            Err(e) => {
                warn!("mDNS tokio soket donusumu basarisiz: {}", e);
                return;
            }
        };

        info!("mDNS dinleyici baslatildi (Port 5353, Multicast: 224.0.0.251)");
        let mut buf = [0u8; 4096];

        loop {
            match tokio_sock.recv_from(&mut buf).await {
                Ok((len, src_addr)) => {
                    if let Some(query) = parse_dns_query(&buf[..len], true) {
                        if self.matches_query(&query.qname) {
                            debug!(
                                "mDNS eslesmesi: {} -> 127.0.0.1 (Kaynak: {})",
                                query.qname, src_addr
                            );
                            let is_ipv6 = query.qtype == 28;
                            let resp = build_dns_response(&query, is_ipv6, true);

                            let _ = tokio_sock.send_to(&resp, src_addr).await;

                            if src_addr.port() == MDNS_PORT {
                                let mcast_dest = SocketAddrV4::new(MDNS_IPV4, MDNS_PORT);
                                let _ = tokio_sock.send_to(&resp, SocketAddr::V4(mcast_dest)).await;
                            }
                        }
                    }
                }
                Err(e) => {
                    debug!("mDNS okuma hatasi: {}", e);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
    }

    async fn run_llmnr_listener(&self) {
        let socket = match create_multicast_socket(LLMNR_PORT, LLMNR_IPV4) {
            Ok(s) => s,
            Err(e) => {
                warn!("LLMNR soketi acilamadi (Port 5355 mesgul olabilir): {}", e);
                return;
            }
        };

        let tokio_sock = match tokio::net::UdpSocket::from_std(socket.into()) {
            Ok(s) => s,
            Err(e) => {
                warn!("LLMNR tokio soket donusumu basarisiz: {}", e);
                return;
            }
        };

        info!("LLMNR dinleyici baslatildi (Port 5355, Multicast: 224.0.0.252)");
        let mut buf = [0u8; 4096];

        loop {
            match tokio_sock.recv_from(&mut buf).await {
                Ok((len, src_addr)) => {
                    if let Some(query) = parse_dns_query(&buf[..len], false) {
                        if self.matches_query(&query.qname) {
                            debug!(
                                "LLMNR eslesmesi: {} -> 127.0.0.1 (Kaynak: {})",
                                query.qname, src_addr
                            );
                            let is_ipv6 = query.qtype == 28;
                            let resp = build_dns_response(&query, is_ipv6, false);
                            let _ = tokio_sock.send_to(&resp, src_addr).await;
                        }
                    }
                }
                Err(e) => {
                    debug!("LLMNR okuma hatasi: {}", e);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
    }

    async fn run_minecraft_lan_broadcaster(&self) {
        let socket = match tokio::net::UdpSocket::bind("0.0.0.0:0").await {
            Ok(s) => s,
            Err(e) => {
                warn!("Minecraft LAN yayin soketi olusturulamadi: {}", e);
                return;
            }
        };

        let _ = socket.set_broadcast(true);
        let mc_dest = SocketAddrV4::new(MINECRAFT_LAN_IPV4, MINECRAFT_LAN_PORT);
        let loopback_dest = SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), MINECRAFT_LAN_PORT);

        let mut interval = tokio::time::interval(Duration::from_millis(1500));

        loop {
            interval.tick().await;

            if !self.is_tunnel_active() {
                continue;
            }

            let srv_name = self.active_server_name();
            let domain = self.active_domain();
            let port = self.active_game_port();

            let motd = format!(
                "[MOTD]maziTunnel • {} ({})[/MOTD][AD]{}[/AD]",
                srv_name, domain, port
            );

            let payload = motd.as_bytes();
            let _ = socket.send_to(payload, SocketAddr::V4(mc_dest)).await;
            let _ = socket.send_to(payload, SocketAddr::V4(loopback_dest)).await;
        }
    }
}

fn create_multicast_socket(port: u16, multicast_addr: Ipv4Addr) -> std::io::Result<socket2::Socket> {
    let socket = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )?;

    socket.set_reuse_address(true)?;

    #[cfg(unix)]
    {
        let _ = socket.set_reuse_port(true);
    }

    socket.set_nonblocking(true)?;

    let bind_addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port);
    socket.bind(&bind_addr.into())?;

    let _ = socket.join_multicast_v4(&multicast_addr, &Ipv4Addr::UNSPECIFIED);
    let _ = socket.join_multicast_v4(&multicast_addr, &Ipv4Addr::LOCALHOST);

    Ok(socket)
}

pub fn parse_dns_query(buf: &[u8], is_mdns: bool) -> Option<DnsQuery> {
    if buf.len() < 12 {
        return None;
    }

    let id = u16::from_be_bytes([buf[0], buf[1]]);
    let flags = u16::from_be_bytes([buf[2], buf[3]]);

    let is_response = (flags & 0x8000) != 0;
    if is_response {
        return None;
    }

    let qdcount = u16::from_be_bytes([buf[4], buf[5]]);
    if qdcount < 1 {
        return None;
    }

    let mut pos = 12;
    let mut labels = Vec::new();
    let question_start = pos;

    while pos < buf.len() {
        let len = buf[pos] as usize;
        if len == 0 {
            pos += 1;
            break;
        }

        if (len & 0xC0) == 0xC0 {
            pos += 2;
            break;
        }

        pos += 1;
        if pos + len > buf.len() {
            return None;
        }

        let label_slice = &buf[pos..pos + len];
        if let Ok(label_str) = std::str::from_utf8(label_slice) {
            labels.push(label_str);
        } else {
            return None;
        }
        pos += len;
    }

    if pos + 4 > buf.len() {
        return None;
    }

    let qtype = u16::from_be_bytes([buf[pos], buf[pos + 1]]);
    let qclass = u16::from_be_bytes([buf[pos + 2], buf[pos + 3]]);
    let question_end = pos + 4;

    let question_bytes = buf[question_start..question_end].to_vec();
    let qname = labels.join(".");

    Some(DnsQuery {
        id,
        flags,
        is_mdns,
        qname,
        qtype,
        qclass,
        question_bytes,
    })
}

pub fn build_dns_response(query: &DnsQuery, is_ipv6: bool, is_mdns: bool) -> Vec<u8> {
    let mut resp = Vec::with_capacity(128);

    resp.extend_from_slice(&query.id.to_be_bytes());

    let flags: u16 = if is_mdns {
        0x8400
    } else {
        0x8400
    };
    resp.extend_from_slice(&flags.to_be_bytes());

    let qdcount: u16 = 1;
    resp.extend_from_slice(&qdcount.to_be_bytes());

    let ancount: u16 = 1;
    resp.extend_from_slice(&ancount.to_be_bytes());

    let nscount: u16 = 0;
    resp.extend_from_slice(&nscount.to_be_bytes());

    let arcount: u16 = 0;
    resp.extend_from_slice(&arcount.to_be_bytes());

    resp.extend_from_slice(&query.question_bytes);

    resp.extend_from_slice(&[0xC0, 0x0C]);

    let rtype: u16 = if is_ipv6 { 28 } else { 1 };
    resp.extend_from_slice(&rtype.to_be_bytes());

    let rclass: u16 = if is_mdns { 0x8001 } else { 0x0001 };
    resp.extend_from_slice(&rclass.to_be_bytes());

    let ttl: u32 = 60;
    resp.extend_from_slice(&ttl.to_be_bytes());

    if is_ipv6 {
        let rdlength: u16 = 16;
        resp.extend_from_slice(&rdlength.to_be_bytes());
        resp.extend_from_slice(&[0u8; 15]);
        resp.push(1);
    } else {
        let rdlength: u16 = 4;
        resp.extend_from_slice(&rdlength.to_be_bytes());
        resp.extend_from_slice(&[127, 0, 0, 1]);
    }

    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_domain() {
        assert_eq!(LocalDomainInterceptor::normalize_domain("baglanti.local"), "baglanti.local");
        assert_eq!(LocalDomainInterceptor::normalize_domain("xeptra.localhost"), "xeptra.local");
        assert_eq!(LocalDomainInterceptor::normalize_domain("xeptra"), "xeptra.local");
        assert_eq!(LocalDomainInterceptor::normalize_domain("XEPTRA.LOCAL"), "xeptra.local");
        assert_eq!(LocalDomainInterceptor::normalize_domain("localhost"), "localhost");
        assert_eq!(LocalDomainInterceptor::normalize_domain(""), "baglanti.local");
    }

    #[test]
    fn test_matches_query() {
        let interceptor = LocalDomainInterceptor::new("xeptra.local", "xeptra", 25565);
        assert!(interceptor.matches_query("xeptra.local"));
        assert!(interceptor.matches_query("xeptra.local."));
        assert!(interceptor.matches_query("xeptra.localhost"));
        assert!(interceptor.matches_query("xeptra"));
        assert!(interceptor.matches_query("baglanti.local"));
        assert!(interceptor.matches_query("baglanti"));
        assert!(!interceptor.matches_query("google.com"));
    }

    #[test]
    fn test_dns_query_parse_and_response() {
        let query_bytes = vec![
            0x12, 0x34,
            0x00, 0x00,
            0x00, 0x01,
            0x00, 0x00,
            0x00, 0x00,
            0x00, 0x00,
            0x06, b'x', b'e', b'p', b't', b'r', b'a',
            0x05, b'l', b'o', b'c', b'a', b'l',
            0x00,
            0x00, 0x01,
            0x00, 0x01,
        ];

        let query = parse_dns_query(&query_bytes, true).expect("Query parse basarisiz");
        assert_eq!(query.id, 0x1234);
        assert_eq!(query.qname, "xeptra.local");
        assert_eq!(query.qtype, 1);

        let response = build_dns_response(&query, false, true);
        assert!(response.len() > query_bytes.len());
        assert_eq!(&response[0..2], &[0x12, 0x34]);
        assert_eq!(&response[response.len() - 4..], &[127, 0, 0, 1]);
    }

    #[test]
    fn test_dns_ipv6_response() {
        let query_bytes = vec![
            0xAB, 0xCD,
            0x00, 0x00,
            0x00, 0x01,
            0x00, 0x00,
            0x00, 0x00,
            0x00, 0x00,
            0x08, b'b', b'a', b'g', b'l', b'a', b'n', b't', b'i',
            0x05, b'l', b'o', b'c', b'a', b'l',
            0x00,
            0x00, 0x1C,
            0x00, 0x01,
        ];

        let query = parse_dns_query(&query_bytes, false).expect("Query parse basarisiz");
        assert_eq!(query.qtype, 28);

        let response = build_dns_response(&query, true, false);
        assert_eq!(&response[0..2], &[0xAB, 0xCD]);
        assert_eq!(&response[response.len() - 16..], &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
    }
}
