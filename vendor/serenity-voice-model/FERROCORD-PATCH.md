# serenity-voice-model 0.3.0, patched for ferrocord

This is serenity-voice-model 0.3.0 from crates.io
(https://github.com/serenity-rs/serenity), licensed under ISC (see
`Cargo.toml`). It is used through `[patch.crates-io]` in ferrocord's
`Cargo.toml`.

One change, in `src/payload.rs` (marked "ferrocord patch"): the voice
gateway's op 12 (`ClientConnect`, called "Video" by Discord) keeps the
`streams` list of simulcast video layers, and is no longer `Copy`. Ferrocord
needs those SSRCs to know whose video each packet carries.
