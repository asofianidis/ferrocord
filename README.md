# ferrocord

A fast, native Discord client written in Rust on [GPUI](https://www.gpui.rs), the GPU-accelerated UI framework behind the Zed editor.

## Why

The official client is an Electron app: high memory use, slow startup, and a sluggish UI. ferrocord is native code end to end, with no embedded browser. The message list, channel list and member list are virtualized, so only what is on screen is laid out and painted.

## Features

- Sign in with a user or bot token, stored in the operating system's credential store
- Servers, categories, channels, threads and direct messages, filtered by your permissions
- Unread markers and mention badges, synced with read state
- Message history with infinite scroll, grouped by author and day
- Send, reply, edit and delete messages; upload attachments
- Discord markdown: bold, italic, underline, strikethrough, spoilers, inline code, code blocks, quotes, headings, lists, links, mentions and timestamps
- Image attachments, file attachments, embeds, stickers and custom emoji
- Reactions: view, toggle and add
- Typing indicators, presence, and the member list with role colours
- Pinned messages
- Quick switcher (`Ctrl+K`) to jump to any channel or conversation
- Per-channel drafts
- Automatic reconnect and session resume

Not implemented: voice and video, forum channels, message search, slash commands, emoji and mention pickers, and server or account settings.

## Running

```sh
cargo run --release
```

To look around without signing in, load offline sample data:

```sh
cargo run -- --demo
```

### Shortcuts

| Key | Action |
| --- | --- |
| `Enter` | Send message |
| `Shift+Enter` | New line |
| `Up` (empty composer) | Edit your last message |
| `Esc` | Cancel reply or edit |
| `Ctrl+K` | Quick switcher |

Right-click a message for reply, edit, react, copy, pin and delete.

## A note on tokens

A bot token is the option Discord sanctions for third-party software. Signing in with a personal account token is against Discord's terms of service and can get an account restricted. ferrocord identifies itself honestly to Discord rather than imitating the official client.

## Development

```sh
cargo test          # markdown, gateway payload and permission tests
RUST_LOG=debug cargo run
```

Layout:

- `src/discord/`: API models, REST client, gateway connection, permission maths
- `src/store.rs`: client state and the bridge between the network runtime and the UI
- `src/markdown.rs`: Discord markdown parser
- `src/ui/`: GPUI views
