//! ferrocord patch: receiving other people's video.
//!
//! The voice connection advertises H.264 decoding, collects the RTP packets
//! of each video stream, rebuilds whole frames from them, removes DAVE
//! end-to-end encryption, and hands the frames (Annex B, with 4-byte start
//! codes) to a [`Config::video_sink`](crate::Config::video_sink).

/// RTP payload type we ask Discord to use for H.264 video.
pub const H264_PAYLOAD_TYPE: u8 = 103;
/// RTP payload type for retransmissions of H.264 packets (ignored).
pub const H264_RTX_PAYLOAD_TYPE: u8 = 104;

const START_CODE: [u8; 4] = [0, 0, 0, 1];

/// One encoded video frame from someone in the call.
#[derive(Clone, Debug)]
pub struct VideoFrame {
    /// The sender, when the voice server has said whose stream this is.
    pub user_id: Option<u64>,
    /// The RTP stream the frame arrived on.
    pub ssrc: u32,
    /// H.264 in Annex B form, ready for a decoder.
    pub data: Vec<u8>,
}

/// Rebuilds H.264 frames from RTP payloads (RFC 6184: single NAL units,
/// STAP-A aggregates and FU-A fragments). A frame with a missing packet is
/// dropped rather than passed on half-built.
#[derive(Debug, Default)]
pub struct H264Assembler {
    frame: Vec<u8>,
    timestamp: Option<u32>,
    next_seq: Option<u16>,
    broken: bool,
}

impl H264Assembler {
    /// An assembler waiting for the first packet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds one packet's payload (RTP header and extensions removed).
    /// Returns a complete frame when `marker` ends it.
    pub fn push(&mut self, seq: u16, timestamp: u32, marker: bool, payload: &[u8]) -> Option<Vec<u8>> {
        let in_order = self.next_seq.is_none_or(|next| next == seq);
        if self.timestamp != Some(timestamp) {
            // A new frame. Whatever was left of the previous one is incomplete.
            self.frame.clear();
            self.timestamp = Some(timestamp);
            self.broken = !in_order;
        } else if !in_order {
            self.broken = true;
        }
        self.next_seq = Some(seq.wrapping_add(1));

        if let Some(&first) = payload.first() {
            match first & 0x1F {
                1..=23 => {
                    self.frame.extend_from_slice(&START_CODE);
                    self.frame.extend_from_slice(payload);
                },
                24 => {
                    // STAP-A: several NAL units, each preceded by a 16-bit size.
                    let mut rest = &payload[1..];
                    while rest.len() >= 2 {
                        let size = u16::from_be_bytes([rest[0], rest[1]]) as usize;
                        rest = &rest[2..];
                        if size == 0 || size > rest.len() {
                            self.broken = true;
                            break;
                        }
                        self.frame.extend_from_slice(&START_CODE);
                        self.frame.extend_from_slice(&rest[..size]);
                        rest = &rest[size..];
                    }
                },
                28 if payload.len() >= 2 => {
                    // FU-A: one NAL unit split across packets.
                    let header = payload[1];
                    if header & 0x80 != 0 {
                        self.frame.extend_from_slice(&START_CODE);
                        self.frame.push((first & 0xE0) | (header & 0x1F));
                    } else if self.frame.is_empty() {
                        // The fragment's start went missing.
                        self.broken = true;
                    }
                    self.frame.extend_from_slice(&payload[2..]);
                },
                _ => {}, // FU-B, MTAP and reserved types are not used by Discord.
            }
        }

        if !marker {
            return None;
        }
        let frame = std::mem::take(&mut self.frame);
        let complete = !self.broken && !frame.is_empty();
        self.broken = false;
        complete.then_some(frame)
    }
}
