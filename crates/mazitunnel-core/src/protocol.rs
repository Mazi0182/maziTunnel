use bytes::{BufMut, Bytes, BytesMut};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Instant;

pub const PUNCH_SYN: &[u8] = b"MAZITUN_PUNCH_SYN";
pub const PUNCH_ACK: &[u8] = b"MAZITUN_PUNCH_ACK";
pub const LEGACY_PUNCH_SYN: &[u8] = b"P2P_TUN_PUNCH_SYN";
pub const LEGACY_PUNCH_ACK: &[u8] = b"P2P_TUN_PUNCH_ACK";

pub const FRAME_HEADER_SIZE: usize = 9;
pub const MAX_FRAME_PAYLOAD: usize = 65535;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum Role {
    Player = 1,
    Admin = 2,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Player => "PLAYER",
            Role::Admin => "ADMIN",
        }
    }

    pub fn from_str_role(s: &str) -> Option<Self> {
        match s.trim().to_uppercase().as_str() {
            "PLAYER" => Some(Role::Player),
            "ADMIN" => Some(Role::Admin),
            _ => None,
        }
    }

    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Role::Player),
            2 => Some(Role::Admin),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum Lane {
    UrgentControl = 0,
    GameStream = 1,
    AdminStream = 2,
}

impl Lane {
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            0 => Some(Lane::UrgentControl),
            1 => Some(Lane::GameStream),
            2 => Some(Lane::AdminStream),
            _ => None,
        }
    }

    pub fn to_u8(self) -> u8 {
        self as u8
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum Cmd {
    Connect = 1,
    Data = 2,
    Close = 3,
    Ping = 4,
    Pong = 5,
    Auth = 6,
    AuthOk = 7,
    AuthFail = 8,
    Disconnect = 9,
}

impl Cmd {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::Connect),
            2 => Some(Self::Data),
            3 => Some(Self::Close),
            4 => Some(Self::Ping),
            5 => Some(Self::Pong),
            6 => Some(Self::Auth),
            7 => Some(Self::AuthOk),
            8 => Some(Self::AuthFail),
            9 => Some(Self::Disconnect),
            _ => None,
        }
    }

    pub fn to_u8(self) -> u8 {
        self as u8
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub lane: Lane,
    pub cmd: Cmd,
    pub stream_id: u32,
    pub target_port: u16,
    pub payload: Bytes,
}

impl Frame {
    pub fn new(cmd: Cmd, stream_id: u32, target_port: u16, payload: Bytes) -> Self {
        let lane = match cmd {
            Cmd::Ping | Cmd::Pong | Cmd::Auth | Cmd::AuthOk | Cmd::AuthFail | Cmd::Disconnect => {
                Lane::UrgentControl
            }
            _ => {
                if target_port == 22 || target_port == 2222 {
                    Lane::AdminStream
                } else {
                    Lane::GameStream
                }
            }
        };
        Self {
            lane,
            cmd,
            stream_id,
            target_port,
            payload,
        }
    }

    pub fn with_lane(lane: Lane, cmd: Cmd, stream_id: u32, target_port: u16, payload: Bytes) -> Self {
        Self {
            lane,
            cmd,
            stream_id,
            target_port,
            payload,
        }
    }

    pub fn encode(&self) -> Bytes {
        let len = self.payload.len().min(MAX_FRAME_PAYLOAD) as u16;
        let mut buf = BytesMut::with_capacity(FRAME_HEADER_SIZE + len as usize);
        buf.put_u32(self.stream_id);
        buf.put_u8(self.cmd as u8);
        buf.put_u16(self.target_port);
        buf.put_u16(len);
        buf.extend_from_slice(&self.payload[..len as usize]);
        buf.freeze()
    }

    pub fn decode(buf: &[u8]) -> Option<(Self, usize)> {
        if buf.len() < FRAME_HEADER_SIZE {
            return None;
        }
        let stream_id = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
        let cmd = Cmd::from_u8(buf[4])?;
        let target_port = u16::from_be_bytes([buf[5], buf[6]]);
        let len = u16::from_be_bytes([buf[7], buf[8]]) as usize;
        if buf.len() < FRAME_HEADER_SIZE + len {
            return None;
        }
        let payload = Bytes::copy_from_slice(&buf[FRAME_HEADER_SIZE..FRAME_HEADER_SIZE + len]);
        let lane = match cmd {
            Cmd::Ping | Cmd::Pong | Cmd::Auth | Cmd::AuthOk | Cmd::AuthFail | Cmd::Disconnect => {
                Lane::UrgentControl
            }
            _ => {
                if target_port == 22 || target_port == 2222 {
                    Lane::AdminStream
                } else {
                    Lane::GameStream
                }
            }
        };
        Some((
            Self {
                lane,
                cmd,
                stream_id,
                target_port,
                payload,
            },
            FRAME_HEADER_SIZE + len,
        ))
    }
}

pub fn encode_frame(frame: &Frame) -> Bytes {
    frame.encode()
}

pub fn decode_frame(buf: &[u8]) -> Option<(Frame, usize)> {
    Frame::decode(buf)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuthRequest {
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub token_hmac: String,
    #[serde(default)]
    pub nonce: String,
    #[serde(default)]
    pub client_nonce: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub hwid: String,
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub computer_name: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub public_ip: String,
    #[serde(default)]
    pub mtu: usize,
    #[serde(default)]
    pub vpn: bool,
    #[serde(default)]
    pub hwid_details: serde_json::Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuthResponse {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub role: String,
    #[serde(default, alias = "server_nonce")]
    pub nonce: String,
    #[serde(default, alias = "server_proof")]
    pub proof: String,
    #[serde(default, alias = "connection_address")]
    pub address: String,
    #[serde(default, alias = "game_port")]
    pub port: u16,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub server_name: String,
}

#[derive(Debug, Clone)]
pub struct KcpSegment {
    pub sn: u32,
    pub ts: u32,
    pub rto: u32,
    pub xmit: u32,
    pub datagram: Bytes,
}

#[derive(Debug)]
pub struct KcpEngine {
    pub conv_id: u32,
    pub send_seq: u32,
    pub recv_seq: u32,
    pub wnd: u16,
    pub max_chunk: usize,
    pub unacked: BTreeMap<u32, KcpSegment>,
    pub receive_buffer: BTreeMap<u32, Bytes>,
    pub recv_queue: Vec<Bytes>,
    pub outbound_queue: Vec<Bytes>,
    pub start_time: Instant,
}

impl KcpEngine {
    pub fn new(conv_id: u32) -> Self {
        Self {
            conv_id,
            send_seq: 0,
            recv_seq: 0,
            wnd: 512,
            max_chunk: 1300,
            unacked: BTreeMap::new(),
            receive_buffer: BTreeMap::new(),
            recv_queue: Vec::new(),
            outbound_queue: Vec::new(),
            start_time: Instant::now(),
        }
    }

    pub fn now_ms(&self) -> u32 {
        self.start_time.elapsed().as_millis() as u32
    }

    pub fn send(&mut self, data: &[u8]) {
        let max_chunk = self.max_chunk.max(500);
        let now = self.now_ms();

        for chunk in data.chunks(max_chunk) {
            let sn = self.send_seq;
            self.send_seq = self.send_seq.wrapping_add(1);

            let mut dgram = BytesMut::with_capacity(24 + chunk.len());
            dgram.put_u32_le(self.conv_id);
            dgram.put_u8(81); // IKCP_CMD_PUSH
            dgram.put_u8(0);  // frg
            dgram.put_u16_le(self.wnd);
            dgram.put_u32_le(now);
            dgram.put_u32_le(sn);
            dgram.put_u32_le(self.recv_seq);
            dgram.put_u32_le(chunk.len() as u32);
            dgram.put_slice(chunk);
            let dgram_bytes = dgram.freeze();

            self.unacked.insert(
                sn,
                KcpSegment {
                    sn,
                    ts: now,
                    rto: 45,
                    xmit: 1,
                    datagram: dgram_bytes.clone(),
                },
            );

            self.outbound_queue.push(dgram_bytes);
        }
    }

    pub fn enqueue(&mut self, data: &[u8]) {
        self.send(data);
    }

    pub fn send_frame(&mut self, frame: &Frame) -> Vec<Bytes> {
        self.send(&frame.encode());
        let now = self.now_ms();
        self.flush(now)
    }

    pub fn flush(&mut self, _now_ms: u32) -> Vec<Bytes> {
        self.outbound_queue.drain(..).collect()
    }

    pub fn update(&mut self, now_ms: u32) -> Vec<Bytes> {
        let mut retrans = Vec::new();
        for seg in self.unacked.values_mut() {
            if now_ms.wrapping_sub(seg.ts) >= seg.rto {
                seg.ts = now_ms;
                seg.xmit = seg.xmit.saturating_add(1);
                seg.rto = ((seg.rto * 4) / 3).clamp(35, 200);
                retrans.push(seg.datagram.clone());
            }
        }
        retrans
    }

    pub fn input(&mut self, data: &[u8], now_ms: u32) -> Vec<Bytes> {
        let mut out_acks = Vec::new();
        let mut offset = 0;
        let pkt_len = data.len();

        while offset + 24 <= pkt_len {
            let conv = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
            let cmd = data[offset + 4];
            let _frg = data[offset + 5];
            let _wnd = u16::from_le_bytes(data[offset + 6..offset + 8].try_into().unwrap());
            let ts = u32::from_le_bytes(data[offset + 8..offset + 12].try_into().unwrap());
            let sn = u32::from_le_bytes(data[offset + 12..offset + 16].try_into().unwrap());
            let una = u32::from_le_bytes(data[offset + 16..offset + 20].try_into().unwrap());
            let len = u32::from_le_bytes(data[offset + 20..offset + 24].try_into().unwrap()) as usize;
            offset += 24;

            if offset + len > pkt_len {
                break;
            }

            let payload = Bytes::copy_from_slice(&data[offset..offset + len]);
            offset += len;

            if self.conv_id == 0 || conv == self.conv_id {
                self.conv_id = conv;
            }

            // Cumulative ACK una
            self.unacked.retain(|&s, _| {
                let diff = una.wrapping_sub(s);
                !(diff > 0 && diff < 0x7FFFFFFF)
            });

            match cmd {
                82 => {
                    // IKCP_CMD_ACK
                    self.unacked.remove(&sn);
                }
                81 => {
                    // IKCP_CMD_PUSH
                    let mut ack = BytesMut::with_capacity(24);
                    ack.put_u32_le(conv);
                    ack.put_u8(82); // IKCP_CMD_ACK
                    ack.put_u8(0);
                    ack.put_u16_le(self.wnd);
                    ack.put_u32_le(ts);
                    ack.put_u32_le(sn);
                    ack.put_u32_le(self.recv_seq);
                    ack.put_u32_le(0);
                    out_acks.push(ack.freeze());

                    if len > 0 {
                        if (sn.wrapping_sub(self.recv_seq) as i32) >= 0 && !self.receive_buffer.contains_key(&sn) {
                            self.receive_buffer.insert(sn, payload);
                        }

                        while let Some(chunk) = self.receive_buffer.remove(&self.recv_seq) {
                            self.recv_seq = self.recv_seq.wrapping_add(1);
                            self.recv_queue.push(chunk);
                        }
                    }
                }
                83 => {
                    // IKCP_CMD_WASK
                    let mut wins = BytesMut::with_capacity(24);
                    wins.put_u32_le(conv);
                    wins.put_u8(84); // IKCP_CMD_WINS
                    wins.put_u8(0);
                    wins.put_u16_le(self.wnd);
                    wins.put_u32_le(now_ms);
                    wins.put_u32_le(0);
                    wins.put_u32_le(self.recv_seq);
                    wins.put_u32_le(0);
                    out_acks.push(wins.freeze());
                }
                _ => {}
            }
        }

        out_acks
    }

    pub fn recv(&mut self) -> Option<Vec<u8>> {
        if self.recv_queue.is_empty() {
            None
        } else {
            Some(self.recv_queue.remove(0).to_vec())
        }
    }

    pub fn recv_all(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        for chunk in self.recv_queue.drain(..) {
            out.extend_from_slice(&chunk);
        }
        out
    }
}
