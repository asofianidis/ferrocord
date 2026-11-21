//! Showing other people's video. Songbird hands over whole, decrypted H.264
//! frames; a background thread decodes them into images GPUI can draw.

use crate::discord::Id;
use gpui::RenderImage;
use openh264::{decoder::Decoder, formats::YUVSource as _};
use songbird::video::VideoFrame;
use std::{collections::HashMap, fmt, sync::Arc};

/// A decoded picture from someone's camera or screen.
#[derive(Clone)]
pub struct VideoImage {
    pub image: Arc<RenderImage>,
    pub width: u32,
    pub height: u32,
}

impl fmt::Debug for VideoImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "VideoImage({}x{})", self.width, self.height)
    }
}

/// Decodes one frame. `None` while the decoder waits for a keyframe, or if
/// the frame is damaged.
pub fn decode(decoder: &mut Decoder, frame: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let yuv = match decoder.decode(frame) {
        Ok(Some(yuv)) => yuv,
        Ok(None) => return None,
        Err(e) => {
            log::debug!("video frame could not be decoded: {e}");
            return None;
        }
    };
    let (width, height) = yuv.dimensions();
    let mut pixels = vec![0u8; width * height * 4];
    yuv.write_rgba8(&mut pixels);
    // GPUI draws BGRA.
    for px in pixels.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    Some((width as u32, height as u32, pixels))
}

/// A moving test pattern, standing in for a camera in demo mode.
pub fn test_pattern(tick: u32) -> Option<VideoImage> {
    let (w, h) = (320u32, 180u32);
    let mut bgra = vec![0u8; (w * h * 4) as usize];
    let bar = (tick * 4) % w;
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let near = (x as i32 - bar as i32).unsigned_abs() < 24;
            bgra[i] = (120 + y * 120 / h) as u8; // blue
            bgra[i + 1] = if near { 230 } else { 60 }; // green
            bgra[i + 2] = (200 * x / w) as u8; // red
            bgra[i + 3] = 255;
        }
    }
    to_image(w, h, bgra)
}

fn to_image(width: u32, height: u32, bgra: Vec<u8>) -> Option<VideoImage> {
    let buffer = image::RgbaImage::from_raw(width, height, bgra)?;
    let frame = image::Frame::new(buffer);
    Some(VideoImage { image: Arc::new(RenderImage::new(smallvec::smallvec![frame])), width, height })
}

/// Decodes frames from `frames` until the voice connection closes, sending
/// each picture with its sender to `send`.
pub fn spawn_decoder(frames: flume::Receiver<VideoFrame>, send: impl Fn(Id, VideoImage) -> bool + Send + 'static) {
    let spawned = std::thread::Builder::new().name("video-decode".into()).spawn(move || {
        // One decoder per stream: each keeps its own reference frames.
        let mut decoders: HashMap<u32, Decoder> = HashMap::new();
        while let Ok(frame) = frames.recv() {
            let Some(user) = frame.user_id.map(Id) else { continue };
            if !decoders.contains_key(&frame.ssrc) {
                match Decoder::new() {
                    Ok(d) => {
                        decoders.insert(frame.ssrc, d);
                    }
                    Err(e) => {
                        log::warn!("could not start a video decoder: {e}");
                        continue;
                    }
                }
            }
            let Some(decoder) = decoders.get_mut(&frame.ssrc) else { continue };
            // Every frame is decoded (later ones depend on it), but when we
            // are behind only the newest picture is drawn.
            let Some((w, h, px)) = decode(decoder, &frame.data) else { continue };
            if !frames.is_empty() {
                continue;
            }
            if let Some(image) = to_image(w, h, px) {
                if !send(user, image) {
                    return;
                }
            }
        }
    });
    if let Err(e) = spawned {
        log::warn!("could not start video decoding: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openh264::{
        encoder::Encoder,
        formats::{RgbSliceU8, YUVBuffer},
    };
    use songbird::video::H264Assembler;

    /// A 64x48 test picture: left half red, right half blue.
    fn picture() -> Vec<u8> {
        let (w, h) = (64usize, 48usize);
        let mut rgb = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 3;
                if x < w / 2 { rgb[i] = 220 } else { rgb[i + 2] = 220 }
            }
        }
        rgb
    }

    fn encode_frame() -> Vec<u8> {
        let yuv = YUVBuffer::from_rgb_source(RgbSliceU8::new(&picture(), (64, 48)));
        let mut encoder = Encoder::new().unwrap();
        encoder.encode(&yuv).unwrap().to_vec()
    }

    /// Splits an Annex B frame into RTP payloads the way a sender does:
    /// small NAL units whole, large ones as FU-A fragments.
    fn packetize(frame: &[u8], max: usize) -> Vec<Vec<u8>> {
        let mut nals = Vec::new();
        let mut i = 0;
        let starts: Vec<usize> = (0..frame.len().saturating_sub(3))
            .filter(|&j| frame[j..j + 4] == [0, 0, 0, 1] || (frame[j..j + 3] == [0, 0, 1] && (j == 0 || frame[j - 1] != 0)))
            .collect();
        for (k, &s) in starts.iter().enumerate() {
            let begin = if frame[s..s + 4] == [0, 0, 0, 1] { s + 4 } else { s + 3 };
            let end = starts.get(k + 1).map_or(frame.len(), |&n| n);
            nals.push(&frame[begin..end]);
            i = end;
        }
        let _ = i;
        let mut packets = Vec::new();
        for nal in nals {
            if nal.len() <= max {
                packets.push(nal.to_vec());
                continue;
            }
            let (header, body) = (nal[0], &nal[1..]);
            for (n, chunk) in body.chunks(max - 2).enumerate() {
                let first = n == 0;
                let last = (n + 1) * (max - 2) >= body.len();
                let fu_header = (header & 0x1F) | if first { 0x80 } else { 0 } | if last { 0x40 } else { 0 };
                let mut packet = vec![(header & 0xE0) | 28, fu_header];
                packet.extend_from_slice(chunk);
                packets.push(packet);
            }
        }
        packets
    }

    #[test]
    fn frames_survive_packetizing_and_decoding() {
        let encoded = encode_frame();
        let packets = packetize(&encoded, 24);
        assert!(packets.iter().any(|p| p[0] & 0x1F == 28), "big NAL units were fragmented");

        let mut assembler = H264Assembler::new();
        let mut rebuilt = None;
        for (i, packet) in packets.iter().enumerate() {
            let last = i + 1 == packets.len();
            rebuilt = assembler.push(1000 + i as u16, 90_000, last, packet).or(rebuilt);
        }
        let rebuilt = rebuilt.expect("a whole frame");

        let mut decoder = Decoder::new().unwrap();
        let (w, h, bgra) = decode(&mut decoder, &rebuilt).expect("decodes");
        assert_eq!((w, h), (64, 48));
        // BGRA: left is red (R in byte 2), right is blue (B in byte 0).
        let left = &bgra[(24 * 64 + 8) * 4..][..4];
        let right = &bgra[(24 * 64 + 56) * 4..][..4];
        assert!(left[2] > 150 && left[0] < 80, "left {left:?}");
        assert!(right[0] > 150 && right[2] < 80, "right {right:?}");
    }

    #[test]
    fn frames_with_a_lost_packet_are_dropped() {
        let packets = packetize(&encode_frame(), 24);
        let mut assembler = H264Assembler::new();
        let mut out = None;
        for (i, packet) in packets.iter().enumerate().filter(|(i, _)| *i != 1) {
            out = assembler.push(i as u16, 7, i + 1 == packets.len(), packet).or(out);
        }
        assert!(out.is_none(), "a frame with a gap is not passed on");
        // The next frame, complete and numbered on from the last, comes through.
        let start = packets.len() as u16;
        for (i, packet) in packets.iter().enumerate() {
            out = assembler.push(start + i as u16, 8, i + 1 == packets.len(), packet).or(out);
        }
        assert!(out.is_some());
    }
}
