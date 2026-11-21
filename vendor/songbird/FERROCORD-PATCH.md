# songbird 0.6.1, patched for ferrocord

This is songbird 0.6.1 from crates.io (https://github.com/serenity-rs/songbird),
by Kyle Simpson and contributors, licensed under ISC (see `Cargo.toml`). It is
used through `[patch.crates-io]` in ferrocord's `Cargo.toml`.

One change, in `src/driver/tasks/udp_rx/mod.rs` (marked "ferrocord patch"):
after DAVE end-to-end decryption, or when RTP padding is present, received
packets are trimmed so the decoder sees only the Opus frame. Unpatched, every
packet from a DAVE-encrypted call failed with `opus_decode ... InvalidPacket`
("Decode error for SSRC ...") and that person could not be heard.

A second, cosmetic change in the same file: "RTCP decryption failed" is logged
at debug rather than warning level. RTCP reports fail to decrypt in the rtpsize
encryption modes, are not used, and the warning fired many times a second.

Receiving video (new `src/video.rs`, plus changes in `driver/connection`,
`driver/tasks/ws.rs`, `driver/tasks/udp_rx`, `ws.rs`, `config.rs` and
`constants.rs`, all marked "ferrocord patch"):

- the voice gateway is spoken at v7 (video needs v5+; v8 changes heartbeats);
- Identify says the connection supports video, and Select Protocol offers
  H.264 for decoding (payload type 103, retransmissions 104);
- after connecting, a Video payload (op 12) and Media Sink Wants (op 15,
  `{"any": 100}`) ask for everyone's video;
- op 12 from the server records which SSRCs carry whose video;
- H.264 RTP packets are rebuilt into frames, DAVE-decrypted, and passed to
  `Config::video_sink`.

Remove the audio fixes once a songbird release has them; the video support
is ferrocord's own and would need upstreaming.

Remove this directory and the patch entry once a songbird release fixes it.
