//! The message pane: header, virtualized message list and composer.

use super::{
    members::bot_tag,
    rich::{self, Reveal},
    theme,
    widgets::*,
};
use crate::{
    discord::{Attachment, Embed, Emoji, Id, Message, channel_kind, message_kind, rest::Upload},
    markdown,
    store::{MessageChange, Msg, Store, StoreEvent},
};
use chrono::{DateTime, Local};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme as _,
    input::{self, InputEvent, Textarea, TextareaState},
    menu::{ContextMenuExt as _, PopupMenuItem},
    message_scroller::{MessageScroller, MessageScrollerState},
};
use gpui_kit::assets::IconName;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant},
};

const GROUP_WINDOW_SECS: i64 = 7 * 60;
const QUICK_REACTIONS: [&str; 6] = ["👍", "❤️", "😂", "🎉", "😮", "😢"];

pub struct ChatView {
    store: Entity<Store>,
    channel: Option<Id>,
    scroller: Entity<MessageScrollerState>,
    composer: Entity<TextareaState>,
    reply_to: Option<Id>,
    editing: Option<Id>,
    uploads: Vec<PathBuf>,
    drafts: HashMap<Id, String>,
    revealed: HashSet<Id>,
    pins: Option<Vec<Rc<Msg>>>,
    last_typing: Option<Instant>,
    _subs: Vec<Subscription>,
}

impl ChatView {
    pub fn new(store: Entity<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let scroller = cx.new(|cx| MessageScrollerState::new(0, cx));
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx).auto_grow(1, 12).submit_on_enter(true).placeholder("Message")
        });
        let subs = vec![
            cx.subscribe_in(&store, window, Self::on_store_event),
            cx.subscribe_in(&composer, window, Self::on_input),
            cx.observe(&scroller, Self::on_scroll),
        ];
        let mut this = Self {
            store,
            channel: None,
            scroller,
            composer,
            reply_to: None,
            editing: None,
            uploads: Vec::new(),
            drafts: HashMap::new(),
            revealed: HashSet::new(),
            pins: None,
            last_typing: None,
            _subs: subs,
        };
        this.sync_channel(window, cx);
        this
    }

    pub fn focus_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.composer.update(cx, |s, cx| s.focus(window, cx));
    }

    // ------------------------------------------------------------ store sync

    fn message_count(&self, cx: &App) -> usize {
        self.channel.and_then(|c| self.store.read(cx).messages.get(&c)).map_or(0, |e| e.list.len())
    }

    fn sync_channel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected = self.store.read(cx).selected_channel;
        if selected == self.channel {
            return;
        }
        if let Some(old) = self.channel {
            let text = self.composer.read(cx).value().to_string();
            if text.trim().is_empty() || self.editing.is_some() {
                self.drafts.remove(&old);
            } else {
                self.drafts.insert(old, text);
            }
        }
        self.channel = selected;
        self.reply_to = None;
        self.editing = None;
        self.uploads.clear();
        self.pins = None;
        self.last_typing = None;

        let placeholder = {
            let store = self.store.read(cx);
            match selected.and_then(|c| store.channels.get(&c)) {
                Some(channel) if channel.is_dm() => format!("Message @{}", store.channel_title(channel)),
                Some(channel) => format!("Message #{}", store.channel_title(channel)),
                None => "Message".to_string(),
            }
        };
        let count = self.message_count(cx);
        self.scroller.update(cx, |s, cx| {
            s.reset(count + 1, cx);
            s.scroll_to_end(cx);
        });
        let draft = selected.and_then(|c| self.drafts.get(&c).cloned()).unwrap_or_default();
        self.composer.update(cx, |s, cx| {
            s.set_placeholder(placeholder, window, cx);
            s.set_value(draft, window, cx);
            s.focus(window, cx);
        });
        cx.notify();
    }

    fn on_store_event(
        &mut self,
        _: &Entity<Store>,
        event: &StoreEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            StoreEvent::Selection => {
                self.sync_channel(window, cx);
                cx.notify();
            }
            StoreEvent::Connection => {
                self.sync_channel(window, cx);
                cx.notify();
            }
            StoreEvent::Channels => cx.notify(),
            StoreEvent::Typing(channel) if Some(*channel) == self.channel => cx.notify(),
            StoreEvent::Messages { channel, change } if Some(*channel) == self.channel => {
                let count = self.message_count(cx);
                // Row 0 is the channel header, so message `i` lives at row `i + 1`.
                self.scroller.update(cx, |s, cx| match change {
                    MessageChange::Reset => {
                        s.reset(count + 1, cx);
                        s.scroll_to_end(cx);
                    }
                    MessageChange::Appended(n) => {
                        s.append(*n, cx);
                    }
                    MessageChange::Prepended(n) => {
                        let at_top = s.logical_scroll_top().item_ix == 0;
                        s.splice(1..1, *n, cx);
                        // A view resting on the header would otherwise stay there and
                        // immediately request the next page, cascading through history.
                        if at_top && *n > 0 {
                            s.scroll_to_item(n + 1, cx);
                        }
                        // The header and the first previously-loaded message change shape.
                        s.remeasure_items(0..1, cx);
                        s.remeasure_items(n + 1..n + 2, cx);
                    }
                    MessageChange::Updated(ix) => {
                        s.remeasure_items(ix + 1..ix + 2, cx);
                    }
                    MessageChange::Removed(ix) => {
                        s.splice(ix + 1..ix + 2, 0, cx);
                        // The message that followed may now start a new group.
                        s.remeasure_items(ix + 1..ix + 2, cx);
                    }
                });
                cx.notify();
            }
            _ => {}
        }
    }

    /// Loads older history when the view nears the top of what is loaded.
    fn on_scroll(&mut self, scroller: Entity<MessageScrollerState>, cx: &mut Context<Self>) {
        let Some(channel) = self.channel else { return };
        if scroller.read(cx).logical_scroll_top().item_ix > 6 {
            return;
        }
        let wants_more = self
            .store
            .read(cx)
            .messages
            .get(&channel)
            .is_some_and(|e| e.loaded && !e.loading && !e.reached_start && !e.list.is_empty());
        if wants_more {
            self.store.update(cx, |store, cx| store.load_history(channel, cx));
        }
    }

    // ------------------------------------------------------------ composing

    fn on_input(
        &mut self,
        _: &Entity<TextareaState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::PressEnter { shift: false, .. } => self.submit(window, cx),
            InputEvent::Change => {
                let Some(channel) = self.channel else { return };
                if self.editing.is_some() || self.composer.read(cx).value().trim().is_empty() {
                    return;
                }
                let due = self.last_typing.is_none_or(|at| at.elapsed() > Duration::from_secs(8));
                if due {
                    self.last_typing = Some(Instant::now());
                    self.store.update(cx, |store, cx| store.send_typing(channel, cx));
                }
            }
            _ => {}
        }
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(channel) = self.channel else { return };
        let content = self.composer.read(cx).value().trim().to_string();

        if let Some(message) = self.editing.take() {
            if !content.is_empty() {
                self.store.update(cx, |store, cx| store.edit_message(channel, message, content, cx));
            }
            self.composer.update(cx, |s, cx| s.set_value("", window, cx));
            cx.notify();
            return;
        }

        let paths = std::mem::take(&mut self.uploads);
        if content.is_empty() && paths.is_empty() {
            return;
        }
        let reply_to = self.reply_to.take();
        self.last_typing = None;
        self.composer.update(cx, |s, cx| s.set_value("", window, cx));
        self.scroller.update(cx, |s, cx| s.scroll_to_end(cx));
        cx.notify();

        if paths.is_empty() {
            self.store.update(cx, |store, cx| store.send_message(channel, content, reply_to, Vec::new(), cx));
            return;
        }
        // Read attachments off the UI thread.
        let store = self.store.clone();
        cx.spawn(async move |_, cx| {
            let read = cx
                .background_executor()
                .spawn(async move {
                    paths
                        .into_iter()
                        .map(|path| {
                            let filename = path
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| "file".into());
                            std::fs::read(&path).map(|bytes| Upload { filename, bytes })
                        })
                        .collect::<std::io::Result<Vec<_>>>()
                })
                .await;
            let _ = store.update(cx, |store, cx| match read {
                Ok(uploads) => store.send_message(channel, content, reply_to, uploads, cx),
                Err(e) => cx.emit(StoreEvent::Error(format!("Could not read attachment: {e}"))),
            });
        })
        .detach();
    }

    fn pick_files(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picked.await {
                let _ = this.update(cx, |this, cx| {
                    for path in paths {
                        if !this.uploads.contains(&path) && this.uploads.len() < 10 {
                            this.uploads.push(path);
                        }
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn start_reply(&mut self, message: Id, window: &mut Window, cx: &mut Context<Self>) {
        self.editing = None;
        self.reply_to = Some(message);
        self.composer.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    fn start_edit(&mut self, message: Id, window: &mut Window, cx: &mut Context<Self>) {
        let Some(channel) = self.channel else { return };
        let content = self
            .store
            .read(cx)
            .messages
            .get(&channel)
            .and_then(|e| e.list.iter().find(|m| m.m.id == message))
            .map(|m| m.m.content.clone());
        let Some(content) = content else { return };
        self.reply_to = None;
        self.editing = Some(message);
        self.composer.update(cx, |s, cx| {
            let end = content.len();
            s.set_value(content, window, cx);
            s.set_selected_range(end..end, cx);
            s.focus(window, cx);
        });
        cx.notify();
    }

    fn edit_last_own(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(channel) = self.channel else { return };
        let store = self.store.read(cx);
        let me = store.my_id();
        let last = store.messages.get(&channel).and_then(|e| {
            e.list.iter().rev().find(|m| m.m.author.id == me && !m.pending && !m.m.is_system()).map(|m| m.m.id)
        });
        if let Some(id) = last {
            self.start_edit(id, window, cx);
        }
    }

    /// Returns true if there was something to cancel.
    fn cancel_modes(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.pins.take().is_some() {
            cx.notify();
            return true;
        }
        if self.editing.take().is_some() {
            self.composer.update(cx, |s, cx| s.set_value("", window, cx));
            cx.notify();
            return true;
        }
        if self.reply_to.take().is_some() {
            cx.notify();
            return true;
        }
        false
    }

    fn confirm_delete(&mut self, message: Id, window: &mut Window, cx: &mut Context<Self>) {
        let Some(channel) = self.channel else { return };
        let answer = window.prompt(
            PromptLevel::Warning,
            "Delete message?",
            Some("This cannot be undone."),
            &["Delete", "Cancel"],
            cx,
        );
        let store = self.store.clone();
        cx.spawn(async move |_, cx| {
            if answer.await == Ok(0) {
                let _ = store.update(cx, |store, cx| store.delete_message(channel, message, cx));
            }
        })
        .detach();
    }

    fn toggle_pins(&mut self, cx: &mut Context<Self>) {
        if self.pins.take().is_some() {
            cx.notify();
            return;
        }
        let Some(channel) = self.channel else { return };
        self.pins = Some(Vec::new());
        cx.notify();
        let this = cx.weak_entity();
        self.store.update(cx, |store, cx| {
            store.fetch_pins(channel, cx, move |pins, cx| {
                let _ = this.update(cx, |this, cx| {
                    if this.channel == Some(channel) && this.pins.is_some() {
                        this.pins = Some(pins);
                        cx.notify();
                    }
                });
            });
        });
    }

    // ------------------------------------------------------------ rows

    fn render_row(&self, ix: usize, this: &Entity<Self>, cx: &App) -> AnyElement {
        let store = self.store.read(cx);
        let empty = || div().h(px(1.)).into_any_element();
        let Some(channel_id) = self.channel else { return empty() };
        let Some(entry) = store.messages.get(&channel_id) else { return empty() };

        if ix == 0 {
            let title = store.channels.get(&channel_id).map(|c| store.channel_title(c)).unwrap_or_default();
            let text = if entry.reached_start {
                format!("This is the beginning of {title}.")
            } else if entry.loading || !entry.loaded {
                "Loading messages…".to_string()
            } else {
                "Scroll up to load earlier messages".to_string()
            };
            return div()
                .w_full()
                .px_4()
                .pt_6()
                .pb_2()
                .text_color(theme::text_muted())
                .when(entry.reached_start, |d| {
                    d.child(
                        div()
                            .text_size(px(24.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme::text_bright())
                            .child(title),
                    )
                })
                .child(text)
                .into_any_element();
        }

        let Some(msg) = entry.list.get(ix - 1) else { return empty() };
        let prev = ix.checked_sub(2).and_then(|p| entry.list.get(p));
        let guild = store.channels.get(&channel_id).and_then(|c| c.guild_id);
        let m = &msg.m;
        let id = m.id;
        let me = store.my_id();
        let mine = m.author.id == me;

        let new_day = prev.is_none_or(|p| p.time.date_naive() != msg.time.date_naive());
        let is_reply = m.referenced_message.is_some() || m.kind == message_kind::REPLY;
        let compact = !new_day
            && !is_reply
            && !m.is_system()
            && prev.is_some_and(|p| {
                !p.m.is_system()
                    && p.m.author.id == m.author.id
                    && (msg.time - p.time).num_seconds().abs() < GROUP_WINDOW_SECS
            });

        let name = store.display_name(guild, &m.author);
        let name_color = store.name_color(guild, m.author.id).map(theme::c).unwrap_or_else(theme::text_bright);

        let divider = new_day.then(|| {
            h_flex()
                .w_full()
                .px_4()
                .pt_4()
                .pb_1()
                .gap_3()
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::text_muted())
                .child(div().flex_1().h(px(1.)).bg(theme::bg_active()))
                .child(msg.time.format("%B %-d, %Y").to_string())
                .child(div().flex_1().h(px(1.)).bg(theme::bg_active()))
        });

        if m.is_system() {
            return v_flex()
                .w_full()
                .children(divider)
                .child(
                    h_flex()
                        .w_full()
                        .px_4()
                        .py_1()
                        .gap_3()
                        .items_start()
                        .text_color(theme::text_muted())
                        .child(div().w(px(40.)).flex().justify_center().pt(px(2.)).child(icon(
                            IconName::ArrowRight,
                            16.,
                            theme::accent(),
                        )))
                        .child(div().flex_1().min_w_0().child(system_text(m, &name)))
                        .child(div().text_size(px(11.)).child(format_stamp(msg.time))),
                )
                .into_any_element();
        }

        // ---- body
        let revealed = self.revealed.contains(&id);
        let on_reveal: Option<Reveal> = {
            let this = this.clone();
            Some(Rc::new(move |_: &mut Window, cx: &mut App| {
                this.update(cx, |this, cx| {
                    this.revealed.insert(id);
                    cx.notify();
                });
            }))
        };
        let mono = cx.theme().mono_font_family.clone();

        let mut body = v_flex().flex_1().min_w_0().gap(px(2.));
        if !compact {
            body = body.child(
                h_flex()
                    .gap_2()
                    .items_baseline()
                    .child(div().font_weight(FontWeight::SEMIBOLD).text_color(name_color).child(name.clone()))
                    .when(m.author.bot, |d| d.child(bot_tag()))
                    .child(
                        div().text_size(px(11.)).text_color(theme::text_muted()).child(format_stamp(msg.time)),
                    ),
            );
        }
        let mut content = v_flex()
            .w_full()
            .gap(px(2.))
            .when(msg.pending, |d| d.opacity(0.55))
            .when(msg.failed, |d| d.text_color(theme::danger()))
            .children(rich::render_doc(&msg.doc, revealed, on_reveal, mono.clone()));
        if m.edited_timestamp.is_some() {
            content = content.child(div().text_size(px(10.)).text_color(theme::text_muted()).child("(edited)"));
        }
        body = body.child(content);

        if msg.failed {
            let this = this.clone();
            body = body.child(
                div()
                    .id("discard")
                    .text_size(px(12.))
                    .text_color(theme::danger())
                    .cursor_pointer()
                    .child("Failed to send. Click to dismiss.")
                    .on_click(move |_, _, cx| {
                        this.update(cx, |this, cx| {
                            this.store.update(cx, |s, cx| s.discard_failed(channel_id, id, cx));
                        });
                    }),
            );
        }
        for (i, attachment) in m.attachments.iter().enumerate() {
            body = body.child(render_attachment(i, attachment));
        }
        for sticker in &m.sticker_items {
            body = body.child(match sticker.image_url() {
                Some(url) => img(url).size(px(160.)).object_fit(ObjectFit::Contain).into_any_element(),
                None => div()
                    .text_color(theme::text_muted())
                    .child(format!("[sticker: {}]", sticker.name))
                    .into_any_element(),
            });
        }
        for (i, embed) in m.embeds.iter().enumerate() {
            body = body.child(render_embed(i, embed, mono.clone()));
        }
        if !m.reactions.is_empty() {
            body = body.child(h_flex().flex_wrap().gap_1().pt_1().children(m.reactions.iter().enumerate().map(
                |(i, reaction)| {
                    let this = this.clone();
                    let emoji = reaction.emoji.clone();
                    let visual = match reaction.emoji.image_url(32) {
                        Some(url) => img(url).size(px(16.)).into_any_element(),
                        None => div().child(reaction.emoji.name.clone().unwrap_or_default()).into_any_element(),
                    };
                    h_flex()
                        .id(("reaction", i))
                        .h(px(24.))
                        .px_1p5()
                        .gap_1()
                        .rounded_md()
                        .border_1()
                        .cursor_pointer()
                        .text_size(px(13.))
                        .map(|d| {
                            if reaction.me {
                                d.border_color(theme::accent()).bg(theme::accent().opacity(0.15))
                            } else {
                                d.border_color(theme::bg_input()).bg(theme::bg_input())
                            }
                        })
                        .hover(|s| s.border_color(theme::text_muted()))
                        .child(visual)
                        .child(div().text_color(theme::text()).child(reaction.count.to_string()))
                        .on_click(move |_, _, cx| {
                            let emoji = emoji.clone();
                            this.update(cx, |this, cx| {
                                this.store.update(cx, |s, cx| s.toggle_reaction(channel_id, id, emoji, cx));
                            });
                        })
                },
            )));
        }

        // ---- gutter
        let gutter = if compact {
            div()
                .w(px(40.))
                .flex_shrink_0()
                .pt(px(3.))
                .text_size(px(10.))
                .text_color(theme::text_muted())
                .invisible()
                .group_hover("message", |s| s.visible())
                .child(msg.time.format("%H:%M").to_string())
                .into_any_element()
        } else {
            div().pt(px(2.)).child(avatar(m.author.avatar_url(128), 40.)).into_any_element()
        };

        // ---- reply preview
        let reply = is_reply.then(|| {
            let (who, what) = match &m.referenced_message {
                Some(r) => {
                    let text = if r.content.is_empty() {
                        "Click to see attachment".to_string()
                    } else {
                        markdown::parse_inline_only(r.content.lines().next().unwrap_or(""), &markdown::NoResolver)
                            .text
                    };
                    (store.display_name(guild, &r.author), text)
                }
                None => (String::new(), "Original message was deleted".to_string()),
            };
            h_flex()
                .pl(px(52.))
                .gap_1()
                .text_size(px(12.))
                .text_color(theme::text_muted())
                .child(icon(IconName::CornerUpLeft, 12., theme::text_muted()))
                .when(!who.is_empty(), |d| {
                    d.child(div().flex_shrink_0().font_weight(FontWeight::SEMIBOLD).child(format!("@{who}")))
                })
                .child(div().flex_1().min_w_0().truncate().child(what))
        });

        // ---- hover toolbar
        let can_delete = mine || store.can_manage_messages(channel_id);
        let toolbar = (!msg.pending).then(|| {
            let action = |name: &'static str, icon_name: IconName, tooltip: &'static str| {
                icon_button(name, icon_name, tooltip)
            };
            let mut bar = h_flex()
                .absolute()
                .top(px(-14.))
                .right(px(16.))
                .p(px(2.))
                .gap(px(2.))
                .rounded_md()
                .border_1()
                .border_color(theme::border())
                .bg(theme::bg_sidebar())
                .invisible()
                .group_hover("message", |s| s.visible());
            bar = bar.child({
                let this = this.clone();
                div()
                    .id("react")
                    .size(px(28.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(theme::bg_active()))
                    .child("👍")
                    .tooltip(tip("React"))
                    .on_click(move |_, _, cx| {
                        let emoji = Emoji { name: Some("👍".into()), ..Default::default() };
                        this.update(cx, |this, cx| {
                            this.store.update(cx, |s, cx| s.toggle_reaction(channel_id, id, emoji, cx));
                        });
                    })
            });
            bar = bar.child({
                let this = this.clone();
                action("reply", IconName::Reply, "Reply").on_click(move |_, window, cx| {
                    this.update(cx, |this, cx| this.start_reply(id, window, cx));
                })
            });
            if mine {
                let this = this.clone();
                bar = bar.child(action("edit", IconName::Pencil, "Edit").on_click(move |_, window, cx| {
                    this.update(cx, |this, cx| this.start_edit(id, window, cx));
                }));
            }
            if can_delete {
                let this = this.clone();
                bar = bar.child(action("delete", IconName::Trash, "Delete").on_click(move |_, window, cx| {
                    this.update(cx, |this, cx| this.confirm_delete(id, window, cx));
                }));
            }
            bar
        });

        // ---- context menu
        let menu = {
            let this = this.clone();
            let content = m.content.clone();
            let pinned = m.pinned;
            let pending = msg.pending;
            move |mut menu: gpui_component::menu::PopupMenu, _: &mut Window, _: &mut Context<_>| {
                if pending {
                    return menu.label("Sending…");
                }
                let entity = this.clone();
                menu = menu.item(PopupMenuItem::new("Reply").on_click(move |_, window, cx| {
                    entity.update(cx, |this, cx| this.start_reply(id, window, cx));
                }));
                if mine {
                    let entity = this.clone();
                    menu = menu.item(PopupMenuItem::new("Edit Message").on_click(move |_, window, cx| {
                        entity.update(cx, |this, cx| this.start_edit(id, window, cx));
                    }));
                }
                menu = menu.separator();
                for emoji in QUICK_REACTIONS {
                    let entity = this.clone();
                    menu = menu.item(PopupMenuItem::new(format!("React {emoji}")).on_click(move |_, _, cx| {
                        let emoji = Emoji { name: Some(emoji.to_string()), ..Default::default() };
                        entity.update(cx, |this, cx| {
                            this.store.update(cx, |s, cx| s.toggle_reaction(channel_id, id, emoji, cx));
                        });
                    }));
                }
                menu = menu.separator();
                let text = content.clone();
                menu = menu.item(PopupMenuItem::new("Copy Text").on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                }));
                menu = menu.item(PopupMenuItem::new("Copy Message ID").on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(id.to_string()));
                }));
                let entity = this.clone();
                menu = menu.item(
                    PopupMenuItem::new(if pinned { "Unpin Message" } else { "Pin Message" }).on_click(
                        move |_, _, cx| {
                            entity.update(cx, |this, cx| {
                                this.store.update(cx, |s, cx| s.set_pinned(channel_id, id, !pinned, cx));
                            });
                        },
                    ),
                );
                if can_delete {
                    let entity = this.clone();
                    menu = menu.separator().item(PopupMenuItem::new("Delete Message").on_click(
                        move |_, window, cx| {
                            entity.update(cx, |this, cx| this.confirm_delete(id, window, cx));
                        },
                    ));
                }
                menu
            }
        };

        let highlighted = self.reply_to == Some(id) || self.editing == Some(id);
        let row = div()
            .id(("message", id.0))
            .group("message")
            .relative()
            .w_full()
            .px_4()
            .when(!compact, |d| d.mt_3())
            .py(px(2.))
            .when(msg.mentions_me, |d| {
                d.bg(theme::idle().opacity(0.08)).border_l_2().border_color(theme::idle())
            })
            .when(highlighted, |d| d.bg(theme::accent().opacity(0.10)))
            .hover(|s| s.bg(theme::bg_hover()))
            .children(reply)
            .child(h_flex().items_start().gap_3().child(gutter).child(body))
            .children(toolbar)
            .context_menu(menu);

        v_flex().w_full().children(divider).child(row).into_any_element()
    }

    // ------------------------------------------------------------ chrome

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let channel = self.channel.and_then(|c| store.channels.get(&c));
        let (kind, title, topic, in_guild) = match channel {
            Some(c) => (
                c.kind,
                store.channel_title(c),
                c.topic.clone().filter(|t| !t.is_empty()),
                c.guild_id.is_some(),
            ),
            None => (channel_kind::TEXT, String::new(), None, false),
        };
        let members_shown = store.show_members;
        let pins_shown = self.pins.is_some();

        h_flex()
            .h(px(48.))
            .px_4()
            .gap_2()
            .flex_shrink_0()
            .border_b_1()
            .border_color(theme::border())
            .child(icon(channel_icon(kind), 18., theme::text_muted()))
            .child(
                div()
                    .flex_shrink_0()
                    .max_w(px(320.))
                    .truncate()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::text_bright())
                    .child(title),
            )
            .child(match topic {
                Some(topic) => div()
                    .flex_1()
                    .min_w_0()
                    .pl_2()
                    .truncate()
                    .text_size(px(13.))
                    .text_color(theme::text_muted())
                    .child(markdown::parse_inline_only(topic.lines().next().unwrap_or(""), &markdown::NoResolver).text),
                None => div().flex_1(),
            })
            .when(self.channel.is_some(), |d| {
                d.child(
                    icon_button("pins", IconName::Pin, "Pinned messages")
                        .when(pins_shown, |b| b.bg(theme::bg_active()))
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_pins(cx))),
                )
            })
            .when(in_guild, |d| {
                d.child(
                    icon_button("members", IconName::Users, "Member list")
                        .when(members_shown, |b| b.bg(theme::bg_active()))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.store.update(cx, |store, cx| store.toggle_members(cx));
                        })),
                )
            })
    }

    fn render_pins(&self, pins: &[Rc<Msg>], cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let guild = self.channel.and_then(|c| store.channels.get(&c)).and_then(|c| c.guild_id);
        let mono = cx.theme().mono_font_family.clone();
        v_flex()
            .id("pins")
            .absolute()
            .top(px(52.))
            .right(px(12.))
            .w(px(420.))
            .max_h(px(480.))
            .occlude()
            .rounded_lg()
            .border_1()
            .border_color(theme::border())
            .bg(theme::bg_sidebar())
            .shadow_lg()
            .overflow_y_scroll()
            .child(
                div()
                    .px_3()
                    .py_2()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::text_bright())
                    .child("Pinned Messages"),
            )
            .when(pins.is_empty(), |d| {
                d.child(div().px_3().pb_3().text_color(theme::text_muted()).child("Nothing pinned, or still loading…"))
            })
            .children(pins.iter().enumerate().map(|(i, msg)| {
                v_flex()
                    .id(("pin", i))
                    .mx_2()
                    .mb_2()
                    .p_2()
                    .gap_1()
                    .rounded_md()
                    .bg(theme::bg_chat())
                    .child(
                        h_flex()
                            .gap_2()
                            .child(avatar(msg.m.author.avatar_url(64), 20.))
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme::text_bright())
                                    .child(store.display_name(guild, &msg.m.author)),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(theme::text_muted())
                                    .child(format_stamp(msg.time)),
                            ),
                    )
                    .children(rich::render_doc(&msg.doc, true, None, mono.clone()))
                    .when(!msg.m.attachments.is_empty(), |d| {
                        d.child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme::text_muted())
                                .child(format!("{} attachment(s)", msg.m.attachments.len())),
                        )
                    })
            }))
    }

    fn render_composer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let Some(channel) = self.channel else { return div().into_any_element() };
        let guild = store.channels.get(&channel).and_then(|c| c.guild_id);

        let typing: Vec<&str> = store
            .typing
            .get(&channel)
            .map(|list| list.iter().map(|t| t.name.as_str()).collect())
            .unwrap_or_default();
        let typing_text = match typing.as_slice() {
            [] => String::new(),
            [a] => format!("{a} is typing…"),
            [a, b] => format!("{a} and {b} are typing…"),
            [a, b, c] => format!("{a}, {b} and {c} are typing…"),
            _ => "Several people are typing…".to_string(),
        };

        if !store.can_send(channel) {
            return v_flex()
                .px_4()
                .pb_4()
                .flex_shrink_0()
                .child(
                    div()
                        .px_4()
                        .py_3()
                        .rounded_lg()
                        .bg(theme::bg_input())
                        .text_color(theme::text_muted())
                        .child("You do not have permission to send messages in this channel."),
                )
                .into_any_element();
        }

        let banner = if let Some(id) = self.editing {
            let _ = id;
            Some(("Editing message".to_string(), "Enter to save, Esc to cancel"))
        } else if let Some(id) = self.reply_to {
            let who = store
                .messages
                .get(&channel)
                .and_then(|e| e.list.iter().find(|m| m.m.id == id))
                .map(|m| store.display_name(guild, &m.m.author))
                .unwrap_or_else(|| "message".into());
            Some((format!("Replying to {who}"), "Esc to cancel"))
        } else {
            None
        };

        v_flex()
            .px_4()
            .flex_shrink_0()
            .when_some(banner, |d, (text, hint)| {
                d.child(
                    h_flex()
                        .px_3()
                        .py_1p5()
                        .gap_2()
                        .rounded_t_lg()
                        .bg(theme::bg_sidebar())
                        .text_size(px(12.))
                        .text_color(theme::text_muted())
                        .child(div().flex_1().min_w_0().truncate().font_weight(FontWeight::SEMIBOLD).child(text))
                        .child(hint)
                        .child(
                            icon_button("cancel-mode", IconName::X, "Cancel").size(px(20.)).on_click(cx.listener(
                                |this, _, window, cx| {
                                    this.cancel_modes(window, cx);
                                },
                            )),
                        ),
                )
            })
            .when(!self.uploads.is_empty(), |d| {
                d.child(h_flex().flex_wrap().gap_2().p_2().bg(theme::bg_input()).children(
                    self.uploads.iter().enumerate().map(|(i, path)| {
                        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        h_flex()
                            .id(("upload", i))
                            .px_2()
                            .py_1()
                            .gap_2()
                            .rounded_md()
                            .bg(theme::bg_chat())
                            .text_size(px(12.))
                            .child(icon(IconName::File, 14., theme::text_muted()))
                            .child(div().max_w(px(180.)).truncate().child(name))
                            .child(
                                icon_button(("remove-upload", i), IconName::X, "Remove").size(px(18.)).on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        if i < this.uploads.len() {
                                            this.uploads.remove(i);
                                            cx.notify();
                                        }
                                    }),
                                ),
                            )
                    }),
                ))
            })
            .child(
                h_flex()
                    .items_end()
                    .gap_1()
                    .px_2()
                    .py_1p5()
                    .rounded_lg()
                    .bg(theme::bg_input())
                    .child(
                        icon_button("attach", IconName::Paperclip, "Attach files")
                            .on_click(cx.listener(|this, _, window, cx| this.pick_files(window, cx))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Textarea::new(&self.composer).appearance(false).bordered(false)),
                    )
                    .child(
                        icon_button("send", IconName::Send, "Send")
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    ),
            )
            .child(
                div()
                    .h(px(22.))
                    .px_1()
                    .text_size(px(12.))
                    .text_color(theme::text_muted())
                    .truncate()
                    .child(typing_text),
            )
            .into_any_element()
    }
}

impl Render for ChatView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity();
        let has_channel = self.channel.is_some();

        let list = if has_channel {
            let key = self.channel.map_or(0, |c| c.0);
            // Images are retained per channel, so switching channels frees them.
            image_cache(retain_all(("chat-images", key)))
                .flex_1()
                .min_h_0()
                .child(
                    MessageScroller::new("messages", self.scroller.clone(), move |ix, _window, cx| {
                        let view = this.read(cx);
                        view.render_row(ix, &this, cx)
                    })
                    .with_row_style(StyleRefinement::default().px_0().pb_0())
                    .size_full(),
                )
                .into_any_element()
        } else {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme::text_muted())
                .child("Select a channel to start chatting.")
                .into_any_element()
        };

        let pins = self.pins.clone();
        v_flex()
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(theme::bg_chat())
            // The composer binds Escape and Up itself, so intercept them on the way down.
            .capture_action(cx.listener(|this, _: &input::Escape, window, cx| {
                if this.cancel_modes(window, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &input::MoveUp, window, cx| {
                if this.editing.is_none() && this.composer.read(cx).value().is_empty() {
                    this.edit_last_own(window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(self.render_header(cx))
            .child(list)
            .child(self.render_composer(cx))
            .when_some(pins, |d, pins| d.child(self.render_pins(&pins, cx)))
    }
}

// ---------------------------------------------------------------- helpers

fn format_stamp(time: DateTime<Local>) -> String {
    let today = Local::now().date_naive();
    let day = time.date_naive();
    if day == today {
        format!("Today at {}", time.format("%H:%M"))
    } else if Some(day) == today.pred_opt() {
        format!("Yesterday at {}", time.format("%H:%M"))
    } else {
        time.format("%Y-%m-%d %H:%M").to_string()
    }
}

fn system_text(m: &Message, name: &str) -> String {
    use message_kind::*;
    let target = m.mentions.first().map(|u| u.display_name().to_string()).unwrap_or_else(|| "someone".into());
    match m.kind {
        MEMBER_JOIN => format!("{name} joined the server."),
        PINNED => format!("{name} pinned a message to this channel."),
        BOOST | BOOST_TIER_1 | BOOST_TIER_2 | BOOST_TIER_3 => format!("{name} boosted the server!"),
        RECIPIENT_ADD => format!("{name} added {target} to the group."),
        RECIPIENT_REMOVE => format!("{name} removed {target} from the group."),
        CALL => format!("{name} started a call."),
        CHANNEL_NAME_CHANGE => format!("{name} changed the channel name: {}", m.content),
        CHANNEL_ICON_CHANGE => format!("{name} changed the channel icon."),
        THREAD_CREATED => format!("{name} started a thread: {}", m.content),
        _ if !m.content.is_empty() => format!("{name}: {}", m.content),
        _ => format!("{name} triggered an event this client cannot display yet."),
    }
}

/// Scales `(w, h)` down to fit within the given box, preserving aspect ratio.
fn fit(w: u32, h: u32, max_w: f32, max_h: f32) -> (f32, f32) {
    let (w, h) = (w.max(1) as f32, h.max(1) as f32);
    let scale = (max_w / w).min(max_h / h).min(1.0);
    ((w * scale).round().max(1.), (h * scale).round().max(1.))
}

/// Asks Discord's media proxy for a downscaled rendition.
fn sized_url(url: &str, w: f32, h: f32) -> String {
    if !url.contains("discordapp.net") && !url.contains("discordapp.com") {
        return url.to_string();
    }
    let sep = if url.contains('?') { '&' } else { '?' };
    format!("{url}{sep}width={}&height={}", (w * 2.) as u32, (h * 2.) as u32)
}

fn media(id: impl Into<ElementId>, src: String, open: String, w: f32, h: f32) -> impl IntoElement {
    div()
        .id(id)
        .w(px(w))
        .h(px(h))
        .mt_1()
        .rounded_md()
        .overflow_hidden()
        .cursor_pointer()
        .bg(theme::bg_code())
        .child(
            img(src)
                .size_full()
                .object_fit(ObjectFit::Cover)
                .with_fallback(|| {
                    div()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(theme::text_muted())
                        .child("Image unavailable")
                        .into_any_element()
                }),
        )
        .on_click(move |_, _, cx| cx.open_url(&open))
}

fn render_attachment(ix: usize, attachment: &Attachment) -> AnyElement {
    if attachment.is_image() {
        let (w, h) = fit(attachment.width.unwrap_or(400), attachment.height.unwrap_or(300), 420., 320.);
        let base = if attachment.proxy_url.is_empty() { &attachment.url } else { &attachment.proxy_url };
        // Animated images lose their animation when resized by the proxy.
        let src = if attachment.filename.to_ascii_lowercase().ends_with(".gif") {
            attachment.url.clone()
        } else {
            sized_url(base, w, h)
        };
        return media(("attachment", ix), src, attachment.url.clone(), w, h).into_any_element();
    }
    let url = attachment.url.clone();
    h_flex()
        .id(("attachment", ix))
        .mt_1()
        .max_w(px(420.))
        .px_3()
        .py_2()
        .gap_3()
        .rounded_md()
        .border_1()
        .border_color(theme::border())
        .bg(theme::bg_sidebar())
        .cursor_pointer()
        .hover(|s| s.bg(theme::bg_hover()))
        .child(icon(IconName::File, 24., theme::text_muted()))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(div().truncate().text_color(theme::link()).child(attachment.filename.clone()))
                .child(
                    div().text_size(px(12.)).text_color(theme::text_muted()).child(format_size(attachment.size)),
                ),
        )
        .child(icon(IconName::Download, 16., theme::text_muted()))
        .on_click(move |_, _, cx| cx.open_url(&url))
        .into_any_element()
}

fn render_embed(ix: usize, embed: &Embed, mono: SharedString) -> AnyElement {
    let has_text = embed.title.is_some()
        || embed.description.is_some()
        || embed.author.is_some()
        || !embed.fields.is_empty()
        || embed.footer.is_some();

    // Bare image/GIF links: show just the picture.
    if !has_text {
        if let Some(image) = embed.image.as_ref().or(embed.thumbnail.as_ref()) {
            let (w, h) = fit(image.width.unwrap_or(400), image.height.unwrap_or(300), 420., 320.);
            let open = embed.url.clone().unwrap_or_else(|| image.url.clone());
            return media(("embed", ix), sized_url(image.best_url(), w, h), open, w, h).into_any_element();
        }
        return div().into_any_element();
    }

    let bar = embed.color.map(theme::c).unwrap_or_else(theme::bg_active);
    let resolver = markdown::NoResolver;
    let mut column = v_flex().flex_1().min_w_0().gap_1();
    if let Some(provider) = &embed.provider {
        column = column.child(
            div().text_size(px(12.)).text_color(theme::text_muted()).child(provider.name.clone()),
        );
    }
    if let Some(author) = &embed.author {
        column = column.child(
            div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(theme::text_bright()).child(author.name.clone()),
        );
    }
    if let Some(title) = &embed.title {
        let url = embed.url.clone();
        column = column.child(
            div()
                .id(("embed-title", ix))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(if url.is_some() { theme::link() } else { theme::text_bright() })
                .when_some(url, |d, url| d.cursor_pointer().on_click(move |_, _, cx| cx.open_url(&url)))
                .child(markdown::parse_inline_only(title, &resolver).text),
        );
    }
    if let Some(description) = &embed.description {
        let doc = markdown::parse(description, &resolver);
        column = column.child(
            v_flex().gap(px(2.)).text_size(px(13.)).children(rich::render_doc(&doc, true, None, mono.clone())),
        );
    }
    for field in &embed.fields {
        let doc = markdown::parse(&field.value, &resolver);
        column = column.child(
            v_flex()
                .pt_1()
                .text_size(px(13.))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::text_bright())
                        .child(markdown::parse_inline_only(&field.name, &resolver).text),
                )
                .children(rich::render_doc(&doc, true, None, mono.clone())),
        );
    }
    if let Some(image) = &embed.image {
        let (w, h) = fit(image.width.unwrap_or(400), image.height.unwrap_or(300), 380., 300.);
        column = column.child(media(("embed-image", ix), sized_url(image.best_url(), w, h), image.url.clone(), w, h));
    }
    if let Some(footer) = &embed.footer {
        column = column.child(
            div().pt_1().text_size(px(11.)).text_color(theme::text_muted()).child(footer.text.clone()),
        );
    }

    let thumbnail = embed.image.is_none().then_some(embed.thumbnail.as_ref()).flatten().map(|thumb| {
        let (w, h) = fit(thumb.width.unwrap_or(80), thumb.height.unwrap_or(80), 80., 80.);
        media(("embed-thumb", ix), sized_url(thumb.best_url(), w, h), thumb.url.clone(), w, h)
    });

    h_flex()
        .mt_1()
        .max_w(px(480.))
        .items_stretch()
        .rounded_md()
        .overflow_hidden()
        .bg(theme::bg_sidebar())
        .child(div().w(px(4.)).flex_shrink_0().bg(bar))
        .child(h_flex().flex_1().min_w_0().items_start().gap_3().p_3().child(column).children(thumbnail))
        .into_any_element()
}
