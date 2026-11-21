# songbird 0.6.1, patched for ferrocord

This is songbird 0.6.1 from crates.io (https://github.com/serenity-rs/songbird),
by Kyle Simpson and contributors, licensed under ISC (see `Cargo.toml`). It is
used through `[patch.crates-io]` in ferrocord's `Cargo.toml`.

One change, in `src/driver/tasks/udp_rx/mod.rs` (marked "ferrocord patch"):
after DAVE end-to-end decryption, or when RTP padding is present, received
packets are trimmed so the decoder sees only the Opus frame. Unpatched, every
packet from a DAVE-encrypted call failed with `opus_decode ... InvalidPacket`
("Decode error for SSRC ...") and that person could not be heard.

Remove this directory and the patch entry once a songbird release fixes it.
