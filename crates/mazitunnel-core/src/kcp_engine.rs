use std::collections::BTreeMap;
use std::time::Instant;
use bytes::{BufMut, Bytes, BytesMut};

use crate::protocol::{decode_frame, encode_frame, Frame, Lane};

pub const KCP_HEADER_SIZE: usize = 24;

pub const IKCP_CMD_PUSH: u8 = 81;
pub const IKCP_CMD_ACK: u8 = 82;
pub const IKCP_CMD_WASK: u8 = 83;
pub const IKCP_CMD_WINS: u8 = 84;

pub const DEFAULT_MTU: usize = 1350;
pub const VPN_MTU: usize = 1150;
pub const DEFAULT_WND: u16 = 512;
pub const INITIAL_RTO_MS: u32 = 30;
pub const MIN_RTO_MS: u32 = 30;
pub const MAX_RTO_MS: u32 = 250;
pub const CONGESTION_THRESHOLD_GAME_LANE: usize = 384;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KcpHeader {
    pub conv: u32,
    pub cmd: u8,
    pub frg: u8,
    pub wnd: u16,
    pub ts: u32,
    pub sn: u32,
    pub una: u32,
    pub len: u32,
}

impl KcpHeader {
    pub fn encode(&self, buf: &mut BytesMut) {
        buf.put_u32_le(self.conv);
        buf.put_u8(self.cmd);
        buf.put_u8(self.frg);
        buf.put_u16_le(self.wnd);
        buf.put_u32_le(self.ts);
        buf.put_u32_le(self.sn);
        buf.put_u32_le(self.una);
        buf.put_u32_le(self.len);
    }

    pub fn decode(buf: &[u8]) -> Option<Self> {
        if buf.len() < KCP_HEADER_SIZE {
            return None;
        }
        let conv = u32::from_le_bytes(buf[0..4].try_into().ok()?);
        let cmd = buf[4];
        let frg = buf[5];
        let wnd = u16::from_le_bytes(buf[6..8].try_into().ok()?);
        let ts = u32::from_le_bytes(buf[8..12].try_into().ok()?);
        let sn = u32::from_le_bytes(buf[12..16].try_into().ok()?);
        let una = u32::from_le_bytes(buf[16..20].try_into().ok()?);
        let len = u32::from_le_bytes(buf[20..24].try_into().ok()?);
        Some(Self {
            conv,
            cmd,
            frg,
            wnd,
            ts,
            sn,
            una,
            len,
        })
    }
}

#[inline]
pub fn seq_before(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) < 0
}

#[inline]
pub fn seq_geq(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) >= 0
}

#[derive(Debug, Clone)]
pub struct UnackedSegment {
    pub sn: u32,
    pub ts: u32,
    pub rto: u32,
    pub fast_ack: u32,
    pub xmit: u32,
    pub datagram: Bytes,
}

#[derive(Debug, Clone)]
struct RecvSegment {
    frg: u8,
    payload: Bytes,
}

#[derive(Debug)]
struct LaneState {
    #[allow(dead_code)]
    lane: Lane,
    conv_id: u32,
    send_seq: u32,
    recv_seq: u32,
    wnd: u16,
    unacked: BTreeMap<u32, UnackedSegment>,
    receive_buffer: BTreeMap<u32, RecvSegment>,
    fragment_assembly: Vec<Bytes>,
}

impl LaneState {
    fn new(lane: Lane, conv_id: u32) -> Self {
        Self {
            lane,
            conv_id,
            send_seq: 0,
            recv_seq: 0,
            wnd: DEFAULT_WND,
            unacked: BTreeMap::new(),
            receive_buffer: BTreeMap::new(),
            fragment_assembly: Vec::new(),
        }
    }
}

pub struct MultiStreamKcpPeer {
    base_conv: u32,
    mtu: usize,
    start_time: Instant,
    lanes: [LaneState; 3],
}

impl MultiStreamKcpPeer {
    pub fn new(base_conv: u32) -> Self {
        let lanes = [
            LaneState::new(Lane::UrgentControl, base_conv),
            LaneState::new(Lane::GameStream, base_conv.wrapping_add(1)),
            LaneState::new(Lane::AdminStream, base_conv.wrapping_add(2)),
        ];

        Self {
            base_conv,
            mtu: DEFAULT_MTU,
            start_time: Instant::now(),
            lanes,
        }
    }

    pub fn set_mtu(&mut self, mtu: usize) {
        self.mtu = mtu.clamp(576, 1500);
    }

    pub fn now_ms(&self) -> u32 {
        self.start_time.elapsed().as_millis() as u32
    }

    pub fn is_game_lane_congested(&self) -> bool {
        self.lanes[Lane::GameStream as usize].unacked.len() > CONGESTION_THRESHOLD_GAME_LANE
    }

    pub fn unacked_count(&self, lane: Lane) -> usize {
        self.lanes[lane as usize].unacked.len()
    }

    fn lane_mut(&mut self, lane: Lane) -> &mut LaneState {
        &mut self.lanes[lane as usize]
    }

    fn find_lane_mut_by_conv(&mut self, conv: u32) -> Option<&mut LaneState> {
        let diff = conv.wrapping_sub(self.base_conv);
        if diff < 3 {
            return Some(&mut self.lanes[diff as usize]);
        }
        let fallback_idx = (conv % 3) as usize;
        Some(&mut self.lanes[fallback_idx])
    }

    pub fn send_frame(&mut self, frame: &Frame) -> Vec<Bytes> {
        let now = self.now_ms();
        let encoded = encode_frame(frame);
        let max_chunk = self.mtu.saturating_sub(KCP_HEADER_SIZE).max(64);

        let lane_state = self.lane_mut(frame.lane);
        let total_len = encoded.len();
        let chunk_count = (total_len + max_chunk - 1) / max_chunk.max(1);
        let chunk_count = chunk_count.max(1);

        let mut out_datagrams = Vec::with_capacity(chunk_count);

        for (i, chunk) in encoded.chunks(max_chunk).enumerate() {
            let frg = (chunk_count - 1 - i) as u8;
            let sn = lane_state.send_seq;
            lane_state.send_seq = lane_state.send_seq.wrapping_add(1);

            let header = KcpHeader {
                conv: lane_state.conv_id,
                cmd: IKCP_CMD_PUSH,
                frg,
                wnd: lane_state.wnd,
                ts: now,
                sn,
                una: lane_state.recv_seq,
                len: chunk.len() as u32,
            };

            let mut dgram_buf = BytesMut::with_capacity(KCP_HEADER_SIZE + chunk.len());
            header.encode(&mut dgram_buf);
            dgram_buf.put_slice(chunk);
            let dgram_bytes = dgram_buf.freeze();

            lane_state.unacked.insert(
                sn,
                UnackedSegment {
                    sn,
                    ts: now,
                    rto: INITIAL_RTO_MS,
                    fast_ack: 0,
                    xmit: 1,
                    datagram: dgram_bytes.clone(),
                },
            );

            out_datagrams.push(dgram_bytes);
        }

        out_datagrams
    }

    pub fn input_datagram(&mut self, data: &[u8], now_ms: u32) -> (Vec<Frame>, Vec<Bytes>) {
        let mut ready_frames = Vec::new();
        let mut out_acks = Vec::new();

        let mut offset = 0;
        let pkt_len = data.len();

        while offset + KCP_HEADER_SIZE <= pkt_len {
            let Some(hdr) = KcpHeader::decode(&data[offset..offset + KCP_HEADER_SIZE]) else {
                break;
            };
            offset += KCP_HEADER_SIZE;

            let seg_len = hdr.len as usize;
            if offset + seg_len > pkt_len {
                break;
            }
            let payload = Bytes::copy_from_slice(&data[offset..offset + seg_len]);
            offset += seg_len;

            let conv = hdr.conv;
            let lane = self.find_lane_mut_by_conv(conv).expect("lane always resolves");

            // 1. Process una cumulative ack
            let una = hdr.una;
            lane.unacked.retain(|&sn, _| !seq_before(sn, una));

            // 2. Process segment commands
            match hdr.cmd {
                IKCP_CMD_ACK => {
                    let ack_sn = hdr.sn;
                    lane.unacked.remove(&ack_sn);
                    for (&sn, seg) in lane.unacked.iter_mut() {
                        if seq_before(sn, ack_sn) {
                            seg.fast_ack = seg.fast_ack.saturating_add(1);
                        }
                    }
                }
                IKCP_CMD_PUSH => {
                    // Send immediate ACK segment back with received segment's ts
                    let ack_header = KcpHeader {
                        conv,
                        cmd: IKCP_CMD_ACK,
                        frg: 0,
                        wnd: lane.wnd,
                        ts: hdr.ts,
                        sn: hdr.sn,
                        una: lane.recv_seq,
                        len: 0,
                    };
                    let mut ack_buf = BytesMut::with_capacity(KCP_HEADER_SIZE);
                    ack_header.encode(&mut ack_buf);
                    out_acks.push(ack_buf.freeze());

                    let sn = hdr.sn;
                    if seq_geq(sn, lane.recv_seq) && !lane.receive_buffer.contains_key(&sn) {
                        lane.receive_buffer.insert(
                            sn,
                            RecvSegment {
                                frg: hdr.frg,
                                payload,
                            },
                        );
                    }

                    // Process in-order segments
                    while let Some(seg) = lane.receive_buffer.remove(&lane.recv_seq) {
                        lane.recv_seq = lane.recv_seq.wrapping_add(1);
                        lane.fragment_assembly.push(seg.payload);

                        if seg.frg == 0 {
                            // Message is fully assembled
                            let total_msg_len: usize =
                                lane.fragment_assembly.iter().map(|b| b.len()).sum();
                            let mut full_buf = BytesMut::with_capacity(total_msg_len);
                            for chunk in lane.fragment_assembly.drain(..) {
                                full_buf.put_slice(&chunk);
                            }

                            // Decode frames from full buffer
                            let mut read_slice: &[u8] = &full_buf;
                            while let Some((frame, consumed)) = decode_frame(read_slice) {
                                ready_frames.push(frame);
                                read_slice = &read_slice[consumed..];
                            }
                        }
                    }
                }
                IKCP_CMD_WASK => {
                    let wins_header = KcpHeader {
                        conv,
                        cmd: IKCP_CMD_WINS,
                        frg: 0,
                        wnd: lane.wnd,
                        ts: now_ms,
                        sn: 0,
                        una: lane.recv_seq,
                        len: 0,
                    };
                    let mut wins_buf = BytesMut::with_capacity(KCP_HEADER_SIZE);
                    wins_header.encode(&mut wins_buf);
                    out_acks.push(wins_buf.freeze());
                }
                _ => {}
            }
        }

        (ready_frames, out_acks)
    }

    pub fn update(&mut self, now_ms: u32) -> Vec<Bytes> {
        let mut retransmissions = Vec::new();

        for lane in &mut self.lanes {
            for seg in lane.unacked.values_mut() {
                let rto_expired = now_ms.wrapping_sub(seg.ts) >= seg.rto;
                let fast_resend = seg.fast_ack >= 2;

                if rto_expired || fast_resend {
                    seg.ts = now_ms;
                    seg.xmit = seg.xmit.saturating_add(1);
                    seg.fast_ack = 0;
                    seg.rto = ((seg.rto * 3) / 2).clamp(MIN_RTO_MS, MAX_RTO_MS);

                    retransmissions.push(seg.datagram.clone());
                }
            }
        }

        retransmissions
    }
}
