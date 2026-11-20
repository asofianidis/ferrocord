# ferrocord

A fast, native Discord client written in Rust on [GPUI](https://www.gpui.rs), the GPU-accelerated UI framework behind the Zed editor.

## Why

The official client is an Electron app: high memory use, slow startup, and a sluggish UI. ferrocord is native code end to end, with no embedded browser. The message list, channel list and member list are virtualized, so only what is on screen is laid out and painted.

## Features

- Sign in with a user or bot token, stored in the operating system's credential store
- Servers, categories, channels, threads and direct messages, filtered by your permissions
- Unread markers and mention badges, synced with read state; mark a channel or server as read
- Message history with infinite scroll, grouped by author and day
- Send, reply, edit and delete messages; upload attachments; jump to the message a reply points at
- `@mention`, `#channel` and `:emoji:` autocomplete in the composer
- Emoji picker for messages and reactions, including the server's custom emoji
- Discord markdown: bold, italic, underline, strikethrough, spoilers, inline code, code blocks, quotes, headings, lists, links, mentions and timestamps
- Image attachments with an in-app viewer, file attachments, embeds, stickers and custom emoji
- Reactions: view, toggle and add
- Message search (user accounts) and pinned messages
- Typing indicators, presence, your own status, and the member list with role colours
- User profile cards with roles and a shortcut to message them
- Who is in each voice channel (viewing only)
- Quick switcher (`Ctrl+K`) to jump to any channel or conversation
- Per-channel drafts
- Themes, including your own
- Automatic reconnect and session resume

### Not implemented

Joining voice or video, screen sharing, forum and stage channel views, slash commands and other bot interactions, stickers and GIF pickers, polls, friends list and friend requests, creating threads, server folders, notification and mute settings, and server or account management.

## Themes

Blossom, a plum-dark theme with pink accents, is the default. Blossom Light, Midnight and Paper are also built in. Pick one in Settings (`Ctrl+,` or the gear next to your name); the choice is remembered.

To add your own, put a JSON file in the themes folder (Settings has a button that opens it and writes an example):

```json
{
  "name": "My Theme",
  "dark": true,
  "bg_chat": "#221d27",
  "accent": "#d4387c"
}
```

Every colour is optional; anything left out comes from Blossom, or Blossom Light when `dark` is false. The available keys are `bg_rail`, `bg_sidebar`, `bg_chat`, `bg_input`, `bg_hover`, `bg_active`, `bg_code`, `border`, `text`, `text_bright`, `text_muted`, `accent`, `on_accent`, `accent_text`, `link`, `danger`, `warning`, `online`, `idle`, `dnd` and `offline`.

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
| `Tab` or `Enter` | Accept an autocomplete suggestion |
| `Ctrl+K` | Quick switcher |
| `Ctrl+,` | Settings |

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
- `src/ui/`: GPUI views; `src/ui/theme.rs` holds the palettes and theme loading
