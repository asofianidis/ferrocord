# ferrocord

A fast, native Discord client written in Rust on [GPUI](https://www.gpui.rs), the GPU-accelerated UI framework behind the Zed editor.

## Why

The official client is an Electron app: high memory use, slow startup, and a sluggish UI. ferrocord is native code end to end, with no embedded browser. The message list, channel list and member list are virtualized, so only what is on screen is laid out and painted.

## Features

**Navigation**
- Servers in your own order and folders, categories, channels, threads, forums and direct messages, filtered by your permissions
- Unread markers and mention badges, synced with read state; mark a channel or server as read
- Quick switcher (`Ctrl+K`) to jump to any channel or conversation
- Mute channels, conversations and servers

**Messages**
- History with infinite scroll, grouped by author and day
- Send, reply, edit, delete, pin and forward messages; upload attachments
- `@mention`, `#channel`, `:emoji:` and `/command` autocomplete in the composer
- Emoji, GIF and sticker pickers
- Discord markdown: bold, italic, underline, strikethrough, spoilers, inline code, code blocks, quotes, headings, lists, links, mentions and timestamps
- Image attachments with an in-app viewer, file attachments, embeds, stickers and custom emoji
- Reactions, polls with voting, and forwarded messages
- Message search and pinned messages; click a reply to jump to the original
- Per-channel drafts

**Threads and forums**
- Start a thread from any message
- Forum channels list their posts, with a form to create one

**Apps**
- Slash commands with named or positional arguments, including subcommands; attached files fill a command's file options
- Buttons and select menus on app messages

**People**
- Friends page with pending requests and blocked users; add, accept, remove and block
- Group conversations, profile cards with roles, presence, and your own status
- Typing indicators and the member list with role colours
- Voice channels and direct-message calls: join, leave, mute, deafen, speaking indicators, and microphone and speaker selection with an adjustable input sensitivity
- Stage channels: speakers and audience, raising your hand, and for moderators inviting speakers, moving people to the audience, and starting or ending the stage
- Voice moderation: server mute, server deafen and disconnect

**Server administration**
- Server settings: name, icon, the ban list with unbanning, and the audit log
- Create, rename, retopic and delete channels and categories
- Roles: create, rename, recolour, reorder, show separately in the member list, edit permissions and delete
- Channel and category permission overrides for any role or member
- Custom emoji and stickers: upload, rename and delete
- Webhooks: create, rename, move to another channel, copy the address and delete
- AutoMod: keyword rules with blocking and timeouts; switch any rule on or off, or delete it
- Onboarding: default channels, and questions whose answers give roles and channels
- Scheduled events: browse, mark yourself interested, and create, start, end, cancel or delete them
- Members: change nicknames, add and remove roles, time out, kick and ban, limited to what your permissions allow

**App**
- Account: display name, username, avatar, pronouns and about-me text
- Themes, including your own
- Leave a server, close a conversation, copy an invite link
- Automatic reconnect and session resume

### Bot accounts versus user accounts

Discord only offers some features to user accounts: search, friends, GIF search, poll voting, slash commands and pressing other apps' buttons. With a bot token those are unavailable and say so when used. The user-account versions of these rely on endpoints Discord does not document, so they can break without notice.

### Not implemented

Video, camera and screen sharing. Voice is built on songbird, which handles audio only, so video would need its own transport, codecs and DAVE frame encryption. 

### About voice

Voice uses Discord's voice protocol with DAVE end-to-end encryption, via songbird. The audio side (devices, capture, playback, voice activity) is tested on this machine. Connecting to a real Discord voice server has not been tested yet. If it fails, the call panel shows the reason.

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
