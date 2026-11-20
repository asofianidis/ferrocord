//! Offline sample data, used by `--demo` to exercise the UI without a token.

use super::*;
use serde_json::json;

impl Store {
    pub fn load_demo(&mut self, cx: &mut Context<Self>) {
        let user = |id: u64, name: &str, bot: bool| json!({ "id": id.to_string(), "username": name.to_lowercase(), "global_name": name, "bot": bot });
        let me = user(1, "You", false);
        let people = [
            user(2, "Ada", false),
            user(3, "Linus", false),
            user(4, "Grace", false),
            user(5, "BuildBot", true),
            user(6, "Margaret", false),
        ];
        let member = |u: &serde_json::Value, roles: &[&str], nick: Option<&str>| json!({ "user": u, "roles": roles, "nick": nick });
        let everyone_view = (permissions::VIEW_CHANNEL | permissions::SEND_MESSAGES).to_string();

        let guild = |id: u64, name: &str, channels: serde_json::Value| {
            json!({
                "id": id.to_string(),
                "name": name,
                "owner_id": "2",
                "roles": [
                    { "id": id.to_string(), "name": "@everyone", "permissions": everyone_view, "position": 0 },
                    { "id": (id + 1).to_string(), "name": "Maintainers", "color": 0x34b3a0, "position": 2, "hoist": true, "permissions": "0" },
                    { "id": (id + 2).to_string(), "name": "Contributors", "color": 0xe0a458, "position": 1, "permissions": "0" },
                ],
                "channels": channels,
                "members": [
                    member(&me, &[], None),
                    member(&people[0], &[&(id + 1).to_string()], None),
                    member(&people[1], &[&(id + 2).to_string()], Some("linus (kernel)")),
                    member(&people[2], &[&(id + 1).to_string()], None),
                    member(&people[3], &[], None),
                    member(&people[4], &[&(id + 2).to_string()], None),
                ],
            })
        };
        let channel = |id: u64, kind: u8, name: &str, parent: Option<u64>, position: i32| {
            json!({
                "id": id.to_string(), "type": kind, "name": name, "position": position,
                "parent_id": parent.map(|p| p.to_string()),
                "topic": if name == "general" { Some("Everything about the project. Be kind.") } else { None },
                "last_message_id": "900",
            })
        };

        let ready = serde_json::from_value::<serde_json::Value>(json!({
            "user": me,
            "guilds": [
                guild(100, "Rustaceans", json!([
                    channel(110, 4, "Information", None, 0),
                    channel(111, 5, "announcements", Some(110), 0),
                    channel(112, 0, "rules", Some(110), 1),
                    channel(120, 4, "Chat", None, 1),
                    channel(121, 0, "general", Some(120), 0),
                    channel(122, 0, "help", Some(120), 1),
                    channel(123, 0, "showcase", Some(120), 2),
                    channel(124, 2, "Voice Lounge", Some(120), 3),
                    channel(130, 4, "Development", None, 2),
                    channel(131, 0, "gpui", Some(130), 0),
                    channel(132, 0, "async", Some(130), 1),
                ])),
                guild(200, "Game Night", json!([
                    channel(210, 0, "lobby", None, 0),
                    channel(211, 0, "clips", None, 1),
                ])),
                guild(300, "Design Systems Guild", json!([channel(310, 0, "critique", None, 0)])),
            ],
            "private_channels": [
                { "id": "400", "type": 1, "recipients": [people[0]], "last_message_id": "950" },
                { "id": "401", "type": 1, "recipients": [people[2]], "last_message_id": "700" },
                { "id": "402", "type": 3, "name": "Weekend plans", "recipients": [people[1], people[4]], "last_message_id": "800" },
            ],
            "read_state": [
                { "id": "121", "last_message_id": "900" },
                { "id": "122", "last_message_id": "10", "mention_count": 2 },
                { "id": "131", "last_message_id": "10" },
                { "id": "210", "last_message_id": "10", "mention_count": 1 },
                { "id": "400", "last_message_id": "10", "mention_count": 1 },
                { "id": "401", "last_message_id": "700" },
                { "id": "402", "last_message_id": "800" },
            ],
            "presences": [
                { "user": { "id": "1" }, "status": "online" },
                { "user": { "id": "2" }, "status": "online" },
                { "user": { "id": "3" }, "status": "idle" },
                { "user": { "id": "4" }, "status": "dnd" },
                { "user": { "id": "5" }, "status": "online" },
            ],
        }))
        .unwrap();
        self.session += 1;
        self.demo = true;
        self.apply_ready(gateway::parse_ready(ready), cx);

        // Messages for #general.
        let now = chrono::Utc::now();
        let base = (now.timestamp_millis() as u64 - 1_420_070_400_000) << 22;
        let at = |minutes_ago: u64| Id(base - ((minutes_ago * 60_000) << 22));
        let stamp = |minutes_ago: u64| (now - chrono::Duration::minutes(minutes_ago as i64)).to_rfc3339();
        let mut n = 0u64;
        let mut msg = |author: &serde_json::Value, minutes_ago: u64, content: &str, extra: serde_json::Value| {
            n += 1;
            let mut v = json!({
                "id": (at(minutes_ago).0 + n).to_string(),
                "channel_id": "121",
                "guild_id": "100",
                "author": author,
                "content": content,
                "timestamp": stamp(minutes_ago),
            });
            if let (Some(obj), Some(extra)) = (v.as_object_mut(), extra.as_object()) {
                for (k, val) in extra {
                    obj.insert(k.clone(), val.clone());
                }
            }
            serde_json::from_value::<Message>(v).unwrap()
        };
        let none = || json!({});
        let first = msg(&people[0], 60 * 26, "Has anyone tried building a chat UI with **GPUI** yet? The virtualized `list` element looks perfect for it.", none());
        let first_json = json!({ "id": first.id.to_string(), "channel_id": "121", "author": people[0], "content": first.content });
        let batch = vec![
            msg(&people[4], 60 * 30, "", json!({ "type": 7 })),
            first,
            msg(&people[1], 60 * 26 - 2, "Yes! It only lays out what is visible, so *thousands* of messages cost nothing.", json!({ "type": 19, "referenced_message": first_json })),
            msg(&people[1], 60 * 26 - 3, "Scrolling stays at 120 fps here.", none()),
            msg(&people[2], 180, "# Release checklist\n- bump the version\n- run `cargo test`\n- tag and push\n> Reminder: ||the secret launch date|| is not public yet.", none()),
            msg(&people[2], 179, "```rust\nfn main() {\n    println!(\"hello, {}!\", \"world\");\n}\n```", none()),
            msg(&people[3], 120, "CI finished for <#131>: all checks passed.", json!({
                "embeds": [{
                    "title": "Build #482 succeeded",
                    "url": "https://example.com/builds/482",
                    "description": "**42** tests passed in `18.3s`\nBranch: `main`",
                    "color": 0x3ba55d,
                    "fields": [{ "name": "Duration", "value": "1m 12s" }, { "name": "Artifacts", "value": "[download](https://example.com/a.zip)" }],
                    "footer": { "text": "ci.example.com" }
                }]
            })),
            msg(&people[0], 45, "<@1> could you review the gateway reconnect logic when you have a minute? Docs: https://discord.com/developers/docs/events/gateway", json!({
                "mentions": [me],
                "reactions": [
                    { "count": 3, "me": true, "emoji": { "name": "👍" } },
                    { "count": 1, "me": false, "emoji": { "name": "🎉" } },
                ]
            })),
            msg(&me, 40, "On it. ~~Tomorrow~~ __today__, I promise.", json!({ "edited_timestamp": stamp(39) })),
            msg(&people[4], 12, "Here is the design doc.", json!({
                "attachments": [{ "id": "9001", "filename": "architecture-notes.pdf", "size": 482133, "url": "https://example.com/architecture-notes.pdf" }]
            })),
            msg(&people[4], 11, "Small text follows.\n-# last updated this morning", none()),
        ];
        for m in &batch {
            self.learn_from(m);
        }
        let list: Vec<_> = batch.into_iter().map(|m| self.make_msg(m)).collect();
        self.messages.insert(Id(121), ChannelMessages { list, loaded: true, loading: false, reached_start: true });
        for id in [111, 112, 122, 123, 124, 131, 132, 210, 211, 310, 400, 401, 402] {
            self.messages.insert(Id(id), ChannelMessages { loaded: true, reached_start: true, ..Default::default() });
        }

        // Member list for the first guild.
        let row = |store: &Store, id: u64| {
            let user = store.users[&Id(id)].clone();
            let info = store.member_info.get(&(Id(100), Id(id))).cloned().unwrap_or_default();
            MemberRow::Member { status: store.status_of(Id(id)), nick: info.nick, color: info.color, user }
        };
        let rows = vec![
            MemberRow::Group { name: "Maintainers".into(), count: 2 },
            row(self, 2),
            row(self, 4),
            MemberRow::Group { name: "Online".into(), count: 3 },
            row(self, 5),
            row(self, 3),
            row(self, 1),
            MemberRow::Group { name: "Offline".into(), count: 1 },
            row(self, 6),
        ];
        self.member_lists.insert(Id(100), rows);
        self.typing.insert(
            Id(121),
            vec![TypingUser { user: Id(3), name: "linus (kernel)".into(), at: Instant::now() + Duration::from_secs(3600) }],
        );

        self.select_channel(Id(121), cx);
        cx.emit(StoreEvent::Members(Id(100)));
        cx.emit(StoreEvent::Messages { channel: Id(121), change: MessageChange::Reset });
    }
}
