//! The message pane: header, virtualized message list and composer.

use super::{
    call,
    members::bot_tag,
    rich::{self, Reveal},
    theme,
    widgets::*,
};
use crate::{
    discord::{
        Attachment, Component, Embed, Emoji, Gif, Id, Message, Poll, channel_kind, component_kind, message_kind,
        rest::Upload,
    },
    emoji, markdown,
    store::{MessageChange, Msg, Store, StoreEvent},
};
use chrono::{DateTime, Local};
use gpui::{prelude::*, *};
use gpui_component::{
    ActiveTheme as _,
    input::{self, Input, InputEvent, InputState, Textarea, TextareaState},
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

pub enum ChatEvent {
    /// Show an image in the full-window viewer.
    OpenImage(String),
    /// Ask where to forward a message.
    Forward { channel: Id, message: Id },
}

#[derive(Clone, Copy, PartialEq)]
enum PanelKind {
    Pins,
    Search,
}

/// The floating list of messages used for pins and search results.
#[derive(Clone)]
struct Panel {
    kind: PanelKind,
    title: String,
    items: Vec<Rc<Msg>>,
    loading: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum PickerTarget {
    Composer,
    React(Id),
}

#[derive(Clone, Copy, PartialEq)]
enum PickerTab {
    Emoji,
    Gifs,
    Stickers,
}

enum GifState {
    Idle,
    Loading,
    Loaded(Vec<Gif>),
    Failed(String),
}

#[derive(Clone)]
enum Visual {
    Avatar(String),
    Image(String),
    Glyph(String),
    Icon(IconName),
}

struct CompletionItem {
    visual: Visual,
    label: String,
    detail: String,
    /// What the composer shows.
    shown: String,
    /// What is sent to Discord in its place.
    wire: String,
}

struct Completion {
    /// Byte offset of the trigger character in the composer.
    start: usize,
    items: Vec<CompletionItem>,
    selected: usize,
}

type Opener = Rc<dyn Fn(String, &mut App)>;

impl EventEmitter<ChatEvent> for ChatView {}

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
    panel: Option<Panel>,
    search: Entity<InputState>,
    picker: Option<PickerTarget>,
    picker_query: Entity<InputState>,
    picker_tab: PickerTab,
    gifs: GifState,
    completion: Option<Completion>,
    /// Friendly text inserted by autocomplete, with what to send instead.
    replacements: Vec<(String, String)>,
    /// The open profile card: whose, and where it was asked for (window coordinates).
    profile: Option<ProfileCard>,
    flash: Option<Id>,
    post_title: Entity<InputState>,
    post_body: Entity<InputState>,
    stage_topic: Entity<InputState>,
    last_typing: Option<Instant>,
    _subs: Vec<Subscription>,
}

impl ChatView {
    pub fn new(store: Entity<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let scroller = cx.new(|cx| MessageScrollerState::new(0, cx));
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx).auto_grow(1, 12).submit_on_enter(true).placeholder("Message")
        });
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search"));
        let picker_query = cx.new(|cx| InputState::new(window, cx).placeholder("Find an emoji"));
        let post_title = cx.new(|cx| InputState::new(window, cx).placeholder("Post title"));
        let post_body = cx.new(|cx| InputState::new(window, cx).placeholder("Say something to start the post"));
        let stage_topic = cx.new(|cx| InputState::new(window, cx).placeholder("Stage topic"));
        let subs = vec![
            cx.subscribe_in(&store, window, Self::on_store_event),
            cx.subscribe_in(&composer, window, Self::on_input),
            cx.observe(&scroller, Self::on_scroll),
            cx.subscribe_in(&search, window, |this, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.run_search(cx);
                }
            }),
            cx.subscribe(&picker_query, |this, _, event: &InputEvent, cx| match event {
                InputEvent::Change => cx.notify(),
                InputEvent::PressEnter { .. } if this.picker_tab == PickerTab::Gifs => this.load_gifs(cx),
                _ => {}
            }),
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
            panel: None,
            search,
            picker: None,
            picker_query,
            picker_tab: PickerTab::Emoji,
            gifs: GifState::Idle,
            completion: None,
            replacements: Vec::new(),
            profile: None,
            flash: None,
            post_title,
            post_body,
            stage_topic,
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
        self.panel = None;
        self.picker = None;
        self.completion = None;
        self.replacements.clear();
        self.profile = None;
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
            StoreEvent::Channels | StoreEvent::Voice => cx.notify(),
            StoreEvent::Commands => {
                self.completion = self.compute_completion(cx);
                cx.notify();
            }
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
                if self.composer.read(cx).value().starts_with('/') {
                    self.store.update(cx, |store, cx| store.load_commands(channel, cx));
                }
                self.completion = self.compute_completion(cx);
                cx.notify();
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
        let mut content = self.composer.read(cx).value().trim().to_string();
        // Swap friendly names for the ids Discord expects, longest first so
        // that "@Ann" never clobbers part of "@Annabel".
        let mut replacements = std::mem::take(&mut self.replacements);
        replacements.sort_by_key(|(shown, _)| std::cmp::Reverse(shown.len()));
        for (shown, wire) in &replacements {
            content = content.replace(shown.as_str(), wire);
        }
        self.completion = None;

        if let Some(message) = self.editing.take() {
            if !content.is_empty() {
                self.store.update(cx, |store, cx| store.edit_message(channel, message, content, cx));
            }
            self.composer.update(cx, |s, cx| s.set_value("", window, cx));
            cx.notify();
            return;
        }

        if let Some((accepted, used_files)) = self.try_command(channel, &content, cx) {
            if accepted {
                if used_files {
                    self.uploads.clear();
                }
                self.composer.update(cx, |s, cx| s.set_value("", window, cx));
            } else {
                // Keep the friendly names working for the retry.
                self.replacements = replacements;
            }
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
        let had_overlay = self.completion.take().is_some()
            || self.picker.take().is_some()
            || self.profile.take().is_some()
            || self.panel.take().is_some();
        if had_overlay {
            self.composer.update(cx, |s, cx| s.focus(window, cx));
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
        if self.panel.as_ref().is_some_and(|p| p.kind == PanelKind::Pins) {
            self.panel = None;
            cx.notify();
            return;
        }
        let Some(channel) = self.channel else { return };
        self.panel =
            Some(Panel { kind: PanelKind::Pins, title: "Pinned Messages".into(), items: Vec::new(), loading: true });
        cx.notify();
        let this = cx.weak_entity();
        self.store.update(cx, |store, cx| {
            store.fetch_pins(channel, cx, move |items, cx| {
                let _ = this.update(cx, |this, cx| this.fill_panel(channel, PanelKind::Pins, items, cx));
            });
        });
    }

    fn run_search(&mut self, cx: &mut Context<Self>) {
        let Some(channel) = self.channel else { return };
        let query = self.search.read(cx).value().trim().to_string();
        if query.is_empty() {
            return;
        }
        self.panel = Some(Panel {
            kind: PanelKind::Search,
            title: format!("Results for \u{201c}{query}\u{201d}"),
            items: Vec::new(),
            loading: true,
        });
        cx.notify();
        let this = cx.weak_entity();
        self.store.update(cx, |store, cx| {
            store.search(channel, query, cx, move |items, cx| {
                let _ = this.update(cx, |this, cx| this.fill_panel(channel, PanelKind::Search, items, cx));
            });
        });
    }

    fn fill_panel(&mut self, channel: Id, kind: PanelKind, items: Vec<Rc<Msg>>, cx: &mut Context<Self>) {
        if self.channel != Some(channel) {
            return;
        }
        if let Some(panel) = self.panel.as_mut().filter(|p| p.kind == kind) {
            panel.items = items;
            panel.loading = false;
            cx.notify();
        }
    }

    /// Scrolls to a message and highlights it briefly.
    fn jump_to(&mut self, channel: Id, message: Id, cx: &mut Context<Self>) {
        if self.channel != Some(channel) {
            self.store.update(cx, |store, cx| store.select_channel(channel, cx));
            return;
        }
        let ix = self
            .store
            .read(cx)
            .messages
            .get(&channel)
            .and_then(|e| e.list.iter().position(|m| m.m.id == message));
        let Some(ix) = ix else {
            self.store.update(cx, |_, cx| {
                cx.emit(StoreEvent::Error("That message is not loaded yet. Scroll up to load more history.".into()));
            });
            return;
        };
        self.scroller.update(cx, |s, cx| {
            s.scroll_to_item(ix + 1, cx);
        });
        self.flash = Some(message);
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(1600)).await;
            let _ = this.update(cx, |this, cx| {
                if this.flash == Some(message) {
                    this.flash = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    // ------------------------------------------------------------ forums

    fn create_post(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(forum) = self.channel else { return };
        let title = self.post_title.read(cx).value().trim().to_string();
        let body = self.post_body.read(cx).value().trim().to_string();
        if title.is_empty() || body.is_empty() {
            self.store.update(cx, |_, cx| {
                cx.emit(StoreEvent::Error("A post needs both a title and a message.".into()));
            });
            return;
        }
        self.post_title.update(cx, |s, cx| s.set_value("", window, cx));
        self.post_body.update(cx, |s, cx| s.set_value("", window, cx));
        self.store.update(cx, |store, cx| store.create_post(forum, title, body, cx));
    }

    fn render_forum(&self, cx: &mut Context<Self>) -> AnyElement {
        let store = self.store.read(cx);
        let Some(forum) = self.channel else { return div().into_any_element() };
        let guild = store.channels.get(&forum).and_then(|c| c.guild_id);
        let can_post = store.can_send(forum);
        let posts: Vec<_> = store
            .forum_posts(forum)
            .into_iter()
            .map(|post| {
                let owner = post
                    .owner_id
                    .and_then(|id| store.users.get(&id))
                    .map(|user| store.display_name(guild, user))
                    .unwrap_or_else(|| "Unknown".into());
                let active = post.last_message_id.unwrap_or(post.id).time();
                (post.id, post.name.clone().unwrap_or_default(), owner, post.message_count, active, store.is_unread(post.id))
            })
            .collect();

        v_flex()
            .id("forum")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_4()
            .gap_2()
            .when(can_post, |d| {
                d.child(
                    h_flex()
                        .gap_2()
                        .p_2()
                        .rounded_lg()
                        .bg(theme::bg_input())
                        .child(div().w(px(240.)).flex_shrink_0().child(Input::new(&self.post_title)))
                        .child(div().flex_1().min_w_0().child(Input::new(&self.post_body)))
                        .child(
                            div()
                                .id("new-post")
                                .px_3()
                                .py_1p5()
                                .rounded_md()
                                .cursor_pointer()
                                .bg(theme::accent())
                                .text_color(theme::on_accent())
                                .child("Post")
                                .on_click(cx.listener(|this, _, window, cx| this.create_post(window, cx))),
                        ),
                )
            })
            .when(posts.is_empty(), |d| {
                d.child(div().p_4().text_color(theme::text_muted()).child("No posts yet."))
            })
            .children(posts.into_iter().map(|(id, name, owner, count, active, unread)| {
                v_flex()
                    .id(("post", id.0))
                    .p_3()
                    .gap_1()
                    .rounded_lg()
                    .cursor_pointer()
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::bg_sidebar())
                    .hover(|s| s.bg(theme::bg_hover()))
                    .child(
                        h_flex()
                            .gap_2()
                            .when(unread, |d| d.child(div().size(px(8.)).rounded_full().bg(theme::accent())))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme::text_bright())
                                    .child(name),
                            ),
                    )
                    .child(div().text_size(px(12.)).text_color(theme::text_muted()).child(format!(
                        "{owner} \u{00b7} {count} message{} \u{00b7} active {}",
                        if count == 1 { "" } else { "s" },
                        format_stamp(active).to_lowercase()
                    )))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.store.update(cx, |store, cx| store.select_channel(id, cx));
                    }))
            }))
            .into_any_element()
    }

    // ------------------------------------------------------------ autocomplete

    fn composer_focused(&self, window: &Window, cx: &App) -> bool {
        self.composer.focus_handle(cx).is_focused(window)
    }

    fn compute_completion(&self, cx: &App) -> Option<Completion> {
        let channel = self.channel?;
        let state = self.composer.read(cx);
        let text = state.value();
        let cursor = state.cursor().min(text.len());
        if !text.is_char_boundary(cursor) {
            return None;
        }
        let before = &text[..cursor];
        if let Some(typed) = before.strip_prefix('/').filter(|t| !t.contains('\n')) {
            return self.command_completion(channel, typed, cx);
        }
        let start = before.char_indices().rev().find(|(_, c)| c.is_whitespace()).map_or(0, |(i, c)| i + c.len_utf8());
        let mut chars = before[start..].chars();
        let trigger = chars.next()?;
        let query = chars.as_str();
        let store = self.store.read(cx);

        let items: Vec<CompletionItem> = match trigger {
            '@' => store
                .mention_candidates(channel, query, 8)
                .into_iter()
                .map(|(user, name)| CompletionItem {
                    visual: Visual::Avatar(user.avatar_url(64)),
                    detail: user.tag(),
                    shown: format!("@{name}"),
                    wire: format!("<@{}>", user.id),
                    label: name,
                })
                .collect(),
            '#' => store
                .channel_candidates(channel, query, 8)
                .into_iter()
                .map(|(id, name)| CompletionItem {
                    visual: Visual::Icon(IconName::Hash),
                    detail: String::new(),
                    shown: format!("#{name}"),
                    wire: format!("<#{id}>"),
                    label: name,
                })
                .collect(),
            ':' if query.len() >= 2 && query.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') => {
                let needle = query.to_lowercase();
                let guild = store.channels.get(&channel).and_then(|c| c.guild_id).and_then(|g| store.guild(g));
                let custom = guild.into_iter().flat_map(|g| g.guild.emojis.iter()).filter_map(|e| {
                    let (id, name) = (e.id?, e.name.clone()?);
                    name.to_lowercase().contains(&needle).then(|| CompletionItem {
                        visual: Visual::Image(e.image_url(64).unwrap_or_default()),
                        label: format!(":{name}:"),
                        detail: "Server emoji".into(),
                        shown: format!(":{name}:"),
                        wire: format!("<{}:{name}:{id}>", if e.animated { "a" } else { "" }),
                    })
                });
                let unicode = emoji::search(&needle, 8).into_iter().map(|(name, glyph)| CompletionItem {
                    visual: Visual::Glyph(glyph.to_string()),
                    label: format!(":{name}:"),
                    detail: String::new(),
                    shown: glyph.to_string(),
                    wire: glyph.to_string(),
                });
                custom.take(4).chain(unicode).take(8).collect()
            }
            _ => return None,
        };
        (!items.is_empty()).then_some(Completion { start, items, selected: 0 })
    }

    /// Suggestions while a slash command name is being typed.
    fn command_completion(&self, channel: Id, typed: &str, cx: &App) -> Option<Completion> {
        let commands = self.store.read(cx).commands_for(channel)?;
        let needle = typed.trim_end().to_lowercase();
        // Once a full command name is followed by a space the user is typing arguments.
        let in_arguments = typed.ends_with(' ')
            || commands.iter().any(|c| typed.to_lowercase().starts_with(&format!("{} ", c.full_name())));
        let mut matches: Vec<_> = commands
            .iter()
            .filter(|c| {
                let name = c.full_name();
                if in_arguments { name.starts_with(&needle) && name != needle } else { name.contains(&needle) }
            })
            .collect();
        matches.sort_by_key(|c| (!c.full_name().starts_with(&needle), c.full_name()));
        let items: Vec<_> = matches
            .into_iter()
            .take(8)
            .map(|command| {
                let name = format!("/{}", command.full_name());
                let usage = command.usage();
                CompletionItem {
                    visual: Visual::Icon(IconName::SquareTerminal),
                    label: if usage.is_empty() { name.clone() } else { format!("{name} {usage}") },
                    detail: format!("{} \u{00b7} {}", command.description, command.app_name),
                    shown: name.clone(),
                    wire: name,
                }
            })
            .collect();
        (!items.is_empty()).then_some(Completion { start: 0, items, selected: 0 })
    }

    /// Runs the composer text as a slash command if it names one, passing the
    /// attached files to its file options. Returns `None` when the text is not
    /// a known command, otherwise whether it was accepted and used the files.
    fn try_command(&mut self, channel: Id, content: &str, cx: &mut Context<Self>) -> Option<(bool, bool)> {
        let typed = content.strip_prefix('/')?;
        let commands = self.store.read(cx).commands_for(channel)?;
        let command = commands
            .iter()
            .filter(|c| {
                let name = c.full_name();
                typed == name || typed.starts_with(&format!("{name} "))
            })
            .max_by_key(|c| c.full_name().len())?
            .clone();
        let args = typed[command.full_name().len()..].to_string();
        let files = self.uploads.clone();
        let used_files = command.file_options() > 0 && !files.is_empty();
        let accepted = self.store.update(cx, |store, cx| store.run_command(channel, &command, &args, files, cx));
        Some((accepted, used_files))
    }

    fn accept_completion(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(completion) = self.completion.take() else { return };
        let Some(item) = completion.items.get(ix) else { return };
        let (text, cursor) = {
            let state = self.composer.read(cx);
            (state.value().to_string(), state.cursor())
        };
        let cursor = cursor.min(text.len());
        let start = completion.start;
        if start > cursor || !text.is_char_boundary(start) || !text.is_char_boundary(cursor) {
            return;
        }
        let updated = format!("{}{} {}", &text[..start], item.shown, &text[cursor..]);
        let caret = start + item.shown.len() + 1;
        if item.shown != item.wire && !self.replacements.iter().any(|(shown, _)| *shown == item.shown) {
            self.replacements.push((item.shown.clone(), item.wire.clone()));
        }
        self.composer.update(cx, |s, cx| {
            s.set_value(updated, window, cx);
            s.set_selected_range(caret..caret, cx);
            s.focus(window, cx);
        });
        cx.notify();
    }

    fn step_completion(&mut self, delta: isize, cx: &mut Context<Self>) -> bool {
        let Some(completion) = self.completion.as_mut() else { return false };
        let len = completion.items.len() as isize;
        completion.selected = (completion.selected as isize + delta).rem_euclid(len) as usize;
        cx.notify();
        true
    }

    // ------------------------------------------------------------ emoji picker

    fn open_picker(&mut self, target: PickerTarget, window: &mut Window, cx: &mut Context<Self>) {
        if self.picker == Some(target) {
            self.picker = None;
            self.composer.update(cx, |s, cx| s.focus(window, cx));
        } else {
            self.picker = Some(target);
            self.picker_tab = PickerTab::Emoji;
            self.picker_query.update(cx, |s, cx| {
                s.set_placeholder("Find an emoji", window, cx);
                s.set_value("", window, cx);
                s.focus(window, cx);
            });
        }
        cx.notify();
    }

    fn pick_emoji(&mut self, emoji: Emoji, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(target), Some(channel)) = (self.picker.take(), self.channel) else { return };
        match target {
            PickerTarget::React(message) => {
                self.store.update(cx, |s, cx| s.toggle_reaction(channel, message, emoji, cx));
            }
            PickerTarget::Composer => {
                let name = emoji.name.clone().unwrap_or_default();
                let text = match emoji.id {
                    Some(id) => {
                        let shown = format!(":{name}:");
                        let wire = format!("<{}:{name}:{id}>", if emoji.animated { "a" } else { "" });
                        if !self.replacements.iter().any(|(s, _)| *s == shown) {
                            self.replacements.push((shown.clone(), wire));
                        }
                        format!("{shown} ")
                    }
                    None => name,
                };
                self.composer.update(cx, |s, cx| s.insert(text, window, cx));
            }
        }
        self.composer.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    fn load_gifs(&mut self, cx: &mut Context<Self>) {
        let query = self.picker_query.read(cx).value().trim().to_string();
        self.gifs = GifState::Loading;
        cx.notify();
        let this = cx.weak_entity();
        self.store.update(cx, |store, cx| {
            store.search_gifs(query, cx, move |result, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.gifs = match result {
                        Ok(list) => GifState::Loaded(list),
                        Err(reason) => GifState::Failed(reason),
                    };
                    cx.notify();
                });
            });
        });
    }

    fn set_picker_tab(&mut self, tab: PickerTab, window: &mut Window, cx: &mut Context<Self>) {
        self.picker_tab = tab;
        self.picker_query.update(cx, |s, cx| {
            let placeholder = match tab {
                PickerTab::Emoji => "Find an emoji",
                PickerTab::Gifs => "Search GIFs, then press Enter",
                PickerTab::Stickers => "Find a sticker",
            };
            s.set_placeholder(placeholder, window, cx);
            s.set_value("", window, cx);
            s.focus(window, cx);
        });
        if tab == PickerTab::Gifs {
            self.load_gifs(cx);
        }
        cx.notify();
    }

    /// The Emoji / GIFs / Stickers switch, shown when picking for the composer.
    fn picker_tabs(&self, cx: &mut Context<Self>) -> Option<Div> {
        if self.picker != Some(PickerTarget::Composer) {
            return None;
        }
        let current = self.picker_tab;
        let tabs = [("Emoji", PickerTab::Emoji), ("GIFs", PickerTab::Gifs), ("Stickers", PickerTab::Stickers)];
        Some(h_flex().gap_1().children(tabs.into_iter().enumerate().map(|(ix, (label, tab))| {
            div()
                .id(("picker-tab", ix))
                .px_2()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .text_size(px(13.))
                .when(tab == current, |d| d.bg(theme::bg_active()).text_color(theme::text_bright()))
                .hover(|s| s.bg(theme::bg_hover()))
                .child(label)
                .on_click(cx.listener(move |this, _, window, cx| this.set_picker_tab(tab, window, cx)))
        })))
    }

    fn picker_shell() -> Stateful<Div> {
        v_flex()
            .id("picker")
            .absolute()
            .bottom(px(88.))
            .right(px(16.))
            .w(px(348.))
            .h(px(400.))
            .p_2()
            .gap_1()
            .occlude()
            .rounded_lg()
            .border_1()
            .border_color(theme::border())
            .bg(theme::bg_sidebar())
            .shadow_lg()
    }

    fn render_media_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let tabs = self.picker_tabs(cx);
        let note = |text: String| div().p_2().text_color(theme::text_muted()).child(text);
        let mut body = v_flex().id("media-scroll").flex_1().min_h_0().overflow_y_scroll();

        if self.picker_tab == PickerTab::Gifs {
            match &self.gifs {
                GifState::Idle | GifState::Loading => body = body.child(note("Loading\u{2026}".into())),
                GifState::Failed(reason) => body = body.child(note(reason.clone())),
                GifState::Loaded(list) if list.is_empty() => body = body.child(note("No GIFs found.".into())),
                GifState::Loaded(list) => {
                    body = body.child(h_flex().flex_wrap().items_start().gap_1().children(
                        list.iter().take(40).enumerate().map(|(ix, gif)| {
                            let url = gif.url.clone();
                            let ratio = if gif.width == 0 { 0.75 } else { gif.height as f32 / gif.width as f32 };
                            div()
                                .id(("gif", ix))
                                .w(px(162.))
                                .h(px((162. * ratio).clamp(60., 200.)))
                                .rounded_md()
                                .overflow_hidden()
                                .cursor_pointer()
                                .bg(theme::bg_code())
                                .hover(|s| s.opacity(0.85))
                                .child(img(gif.thumbnail().to_string()).size_full().object_fit(ObjectFit::Cover))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    let Some(channel) = this.channel else { return };
                                    let url = url.clone();
                                    this.picker = None;
                                    this.store.update(cx, |s, cx| s.send_message(channel, url, None, Vec::new(), cx));
                                    this.composer.update(cx, |s, cx| s.focus(window, cx));
                                    cx.notify();
                                }))
                        }),
                    ));
                }
            }
        } else {
            let query = self.picker_query.read(cx).value().trim().to_lowercase();
            let stickers: Vec<_> = self
                .channel
                .map(|c| self.store.read(cx).stickers(c))
                .unwrap_or_default()
                .into_iter()
                .filter(|s| s.name.to_lowercase().contains(&query))
                .collect();
            if stickers.is_empty() {
                body = body.child(note("This server has no stickers to show.".into()));
            }
            body = body.child(h_flex().flex_wrap().items_start().gap_1().children(
                stickers.into_iter().enumerate().map(|(ix, sticker)| {
                    let name = sticker.name.clone();
                    let label = name.clone();
                    let visual = match sticker.image_url() {
                        Some(url) => img(url)
                            .size(px(88.))
                            .object_fit(ObjectFit::Contain)
                            .with_fallback(move || {
                                div()
                                    .size(px(88.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_size(px(12.))
                                    .text_color(theme::text_muted())
                                    .child(label.clone())
                                    .into_any_element()
                            })
                            .into_any_element(),
                        None => div().size(px(88.)).child(label).into_any_element(),
                    };
                    div()
                        .id(("sticker", ix))
                        .p_2()
                        .rounded_md()
                        .cursor_pointer()
                        .hover(|s| s.bg(theme::bg_active()))
                        .child(visual)
                        .tooltip(tip(name))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let Some(channel) = this.channel else { return };
                            let sticker = sticker.clone();
                            this.picker = None;
                            this.store.update(cx, |s, cx| s.send_sticker(channel, sticker, cx));
                            this.composer.update(cx, |s, cx| s.focus(window, cx));
                            cx.notify();
                        }))
                }),
            ));
        }
        Self::picker_shell().children(tabs).child(Input::new(&self.picker_query)).child(body).into_any_element()
    }

    fn render_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.picker == Some(PickerTarget::Composer) && self.picker_tab != PickerTab::Emoji {
            return self.render_media_picker(cx);
        }
        let tabs = self.picker_tabs(cx);
        let store = self.store.read(cx);
        let query = self.picker_query.read(cx).value().trim().trim_matches(':').to_lowercase();
        let guild = self.channel.and_then(|c| store.channels.get(&c)).and_then(|c| c.guild_id).and_then(|g| store.guild(g));
        let custom: Vec<Emoji> = guild
            .into_iter()
            .flat_map(|g| g.guild.emojis.iter())
            .filter(|e| e.id.is_some() && e.name.as_ref().is_some_and(|n| n.to_lowercase().contains(&query)))
            .take(60)
            .cloned()
            .collect();

        let cell = |key: (&'static str, usize), tooltip: String| {
            div()
                .id(key)
                .size(px(36.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_md()
                .cursor_pointer()
                .text_size(px(22.))
                .hover(|s| s.bg(theme::bg_active()))
                .tooltip(tip(tooltip))
        };
        let heading = |title: &str| {
            div()
                .w_full()
                .pt_2()
                .pb_1()
                .text_size(px(11.))
                .font_weight(FontWeight::BOLD)
                .text_color(theme::text_muted())
                .child(title.to_uppercase())
        };

        let mut body = v_flex().id("picker-scroll").flex_1().min_h_0().overflow_y_scroll();
        if !custom.is_empty() {
            body = body.child(heading("Server emoji")).child(h_flex().flex_wrap().children(
                custom.into_iter().enumerate().map(|(i, e)| {
                    let name = e.name.clone().unwrap_or_default();
                    let url = e.image_url(64).unwrap_or_default();
                    cell(("custom-emoji", i), format!(":{name}:"))
                        .child(img(url).size(px(26.)).object_fit(ObjectFit::Contain))
                        .on_click(cx.listener(move |this, _, window, cx| this.pick_emoji(e.clone(), window, cx)))
                }),
            ));
        }
        let mut index = 0usize;
        let mut any = false;
        for (category, list) in emoji::CATEGORIES {
            let hits: Vec<_> = list.iter().filter(|(name, _)| name.contains(&query)).collect();
            if hits.is_empty() {
                continue;
            }
            any = true;
            body = body.child(heading(category)).child(h_flex().flex_wrap().children(hits.into_iter().map(
                |(name, glyph)| {
                    index += 1;
                    cell(("emoji", index), format!(":{name}:")).child(*glyph).on_click(cx.listener(
                        move |this, _, window, cx| {
                            let emoji = Emoji { name: Some(glyph.to_string()), ..Default::default() };
                            this.pick_emoji(emoji, window, cx);
                        },
                    ))
                },
            )));
        }
        if !any {
            body = body.child(div().p_2().text_color(theme::text_muted()).child("No emoji match."));
        }

        Self::picker_shell().children(tabs).child(Input::new(&self.picker_query)).child(body).into_any_element()
    }

    fn render_completion(&self, completion: &Completion, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("completion")
            .absolute()
            .bottom(px(88.))
            .left(px(16.))
            .right(px(16.))
            .p_1()
            .occlude()
            .rounded_lg()
            .border_1()
            .border_color(theme::border())
            .bg(theme::bg_sidebar())
            .shadow_lg()
            .children(completion.items.iter().enumerate().map(|(ix, item)| {
                let visual = match &item.visual {
                    Visual::Avatar(url) => avatar(url.clone(), 22.).into_any_element(),
                    Visual::Image(url) => img(url.clone()).size(px(22.)).into_any_element(),
                    Visual::Glyph(glyph) => div().w(px(22.)).text_size(px(18.)).child(glyph.clone()).into_any_element(),
                    Visual::Icon(name) => icon(*name, 16., theme::text_muted()).into_any_element(),
                };
                h_flex()
                    .id(("completion-item", ix))
                    .h(px(32.))
                    .px_2()
                    .gap_2()
                    .rounded_md()
                    .cursor_pointer()
                    .when(ix == completion.selected, |d| d.bg(theme::bg_active()))
                    .hover(|s| s.bg(theme::bg_hover()))
                    .child(visual)
                    .child(div().flex_shrink_0().text_color(theme::text_bright()).child(item.label.clone()))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.))
                            .text_color(theme::text_muted())
                            .child(item.detail.clone()),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| this.accept_completion(ix, window, cx)))
            }))
    }

    /// Adds `@name` for `user` to the message box (sent as a proper mention).
    pub fn insert_mention(&mut self, user: Id, window: &mut Window, cx: &mut Context<Self>) {
        let store = self.store.read(cx);
        let guild = self.channel.and_then(|c| store.channels.get(&c)).and_then(|c| c.guild_id);
        let Some(person) = store.users.get(&user) else { return };
        let shown = format!("@{}", store.display_name(guild, person));
        if !self.replacements.iter().any(|(s, _)| *s == shown) {
            self.replacements.push((shown.clone(), format!("<@{user}>")));
        }
        let current = self.composer.read(cx).value().to_string();
        let joiner = if current.is_empty() || current.ends_with(' ') { "" } else { " " };
        let text = format!("{current}{joiner}{shown} ");
        self.composer.update(cx, |s, cx| {
            s.set_value(text, window, cx);
            s.focus(window, cx);
        });
        cx.notify();
    }

    /// Opens a user's profile card beside the point that was clicked.
    pub fn show_profile(&mut self, user: Id, at: Point<Pixels>, side: Anchor, cx: &mut Context<Self>) {
        self.profile = Some(ProfileCard { user, at, side });
        cx.notify();
    }

    fn render_profile(&self, card_at: ProfileCard, cx: &mut Context<Self>) -> impl IntoElement {
        let user_id = card_at.user;
        let store = self.store.read(cx);
        let guild_id = self.channel.and_then(|c| store.channels.get(&c)).and_then(|c| c.guild_id);
        let user = store.users.get(&user_id).cloned().unwrap_or_default();
        let name = store.display_name(guild_id, &user);
        let status = if user_id == store.my_id() { store.my_status } else { store.status_of(user_id) };
        let roles: Vec<(String, u32)> = guild_id
            .and_then(|g| Some((store.guild(g)?, store.member_info.get(&(g, user_id))?)))
            .map(|(guild, member)| {
                let mut roles: Vec<_> = guild.guild.roles.iter().filter(|r| member.roles.contains(&r.id)).collect();
                roles.sort_by_key(|r| std::cmp::Reverse(r.position));
                roles.into_iter().map(|r| (r.name.clone(), r.color)).collect()
            })
            .unwrap_or_default();
        let is_me = user_id == store.my_id();

        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .px_3()
                .py_1p5()
                .rounded_md()
                .cursor_pointer()
                .map(|d| {
                    if primary {
                        d.bg(theme::accent()).text_color(theme::on_accent())
                    } else {
                        d.bg(theme::bg_input()).text_color(theme::text())
                    }
                })
                .hover(|s| s.opacity(0.85))
                .child(label)
        };

        // A small gap between the click and the card, on the side it opens towards.
        let gap = px(12.);
        let offset = match card_at.side {
            Anchor::TopRight | Anchor::BottomRight => point(-gap, px(0.)),
            _ => point(gap, px(0.)),
        };
        deferred(
            anchored()
                .anchor(card_at.side)
                .position(card_at.at + offset)
                .snap_to_window_with_margin(px(8.))
                .child(
                v_flex()
                    .id("profile")
                    .occlude()
                    .w(px(320.))
                    .rounded_xl()
                    .overflow_hidden()
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::bg_sidebar())
                    .shadow_lg()
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.profile = None;
                        cx.notify();
                    }))
                    .child(div().h(px(56.)).w_full().bg(theme::accent()))
                    .child(
                        v_flex()
                            .px_4()
                            .pb_4()
                            .gap_2()
                            .child(div().mt(px(-36.)).child(avatar_with_status(
                                user.avatar_url(128),
                                72.,
                                status,
                                theme::bg_sidebar(),
                            )))
                            .child(
                                v_flex()
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .text_size(px(18.))
                                                    .font_weight(FontWeight::BOLD)
                                                    .text_color(theme::text_bright())
                                                    .child(name),
                                            )
                                            .when(user.bot, |d| d.child(bot_tag())),
                                    )
                                    .child(div().text_color(theme::text_muted()).child(user.tag()))
                                    .child(
                                        div().text_size(px(12.)).text_color(theme::text_muted()).child(status.label()),
                                    ),
                            )
                            .when(!roles.is_empty(), |d| {
                                d.child(h_flex().flex_wrap().gap_1().children(roles.into_iter().map(|(role, color)| {
                                    h_flex()
                                        .px_2()
                                        .py(px(2.))
                                        .gap_1p5()
                                        .rounded_md()
                                        .bg(theme::bg_input())
                                        .text_size(px(12.))
                                        .child(div().size(px(8.)).rounded_full().bg(if color == 0 {
                                            theme::text_muted()
                                        } else {
                                            theme::c(color)
                                        }))
                                        .child(role)
                                })))
                            })
                            .child(
                                h_flex()
                                    .pt_1()
                                    .gap_2()
                                    .when(!is_me, |d| {
                                        d.child(button("profile-message", "Message", true).on_click(cx.listener(
                                            move |this, _, _, cx| {
                                                this.profile = None;
                                                this.store.update(cx, |store, cx| store.open_dm(user_id, cx));
                                            },
                                        )))
                                    })
                                    .child(button("profile-copy", "Copy User ID", false).on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(user_id.to_string()));
                                    })),
                            ),
                    ),
                ),
        )
        .with_priority(1)
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
        let opener: Opener = {
            let this = this.clone();
            Rc::new(move |url: String, cx: &mut App| {
                this.update(cx, |_, cx| cx.emit(ChatEvent::OpenImage(url)));
            })
        };
        let author_id = m.author.id;
        let show_profile = {
            let this = this.clone();
            move |event: &ClickEvent, _: &mut Window, cx: &mut App| {
                let at = event.position();
                this.update(cx, |this, cx| this.show_profile(author_id, at, Anchor::TopLeft, cx));
            }
        };
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
                    .child(
                        // The menu lives on a plain wrapper so it does not inherit
                        // the name's colour, weight and hover underline.
                        div()
                            .id("author")
                            .child(
                                div()
                                    .id("author-name")
                                    .cursor_pointer()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(name_color)
                                    .hover(|s| s.underline())
                                    .child(name.clone())
                                    .on_click(show_profile.clone()),
                            )
                            .context_menu({
                                let store = self.store.clone();
                                let author = m.author.id;
                                move |menu, _, cx| super::user_menu::user_menu(menu, store.clone(), author, guild, cx)
                            }),
                    )
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
            body = body.child(render_attachment(i, attachment, opener.clone()));
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
            body = body.child(render_embed(i, embed, mono.clone(), opener.clone()));
        }
        if let Some(poll) = &m.poll {
            let this = this.clone();
            body = body.child(render_poll(poll, Rc::new(move |answer, cx| {
                this.update(cx, |this, cx| {
                    this.store.update(cx, |s, cx| s.vote_poll(channel_id, id, answer, cx));
                });
            })));
        }
        for (i, snapshot) in m.message_snapshots.iter().enumerate() {
            let inner = &snapshot.message;
            let doc = markdown::parse(&inner.content, &markdown::NoResolver);
            let mut quoted = v_flex().flex_1().min_w_0().gap(px(2.)).child(
                h_flex()
                    .gap_1()
                    .text_size(px(12.))
                    .text_color(theme::text_muted())
                    .child(icon(IconName::CornerUpRight, 12., theme::text_muted()))
                    .child("Forwarded"),
            );
            quoted = quoted.children(rich::render_doc(&doc, true, None, mono.clone()));
            if inner.content.is_empty() && inner.attachments.is_empty() && inner.embeds.is_empty() {
                quoted = quoted.child(
                    div().text_color(theme::text_muted()).child("This message has no text to show."),
                );
            }
            for (j, attachment) in inner.attachments.iter().enumerate() {
                quoted = quoted.child(render_attachment(1000 + i * 100 + j, attachment, opener.clone()));
            }
            for (j, embed) in inner.embeds.iter().enumerate() {
                quoted = quoted.child(render_embed(1000 + i * 100 + j, embed, mono.clone(), opener.clone()));
            }
            body = body.child(
                h_flex()
                    .w_full()
                    .items_stretch()
                    .gap_2()
                    .child(div().w(px(4.)).flex_shrink_0().rounded_sm().bg(theme::bg_active()))
                    .child(quoted),
            );
        }
        if !m.components.is_empty() {
            let this = this.clone();
            body = body.child(render_components(&m.components, Rc::new(move |component, value, cx| {
                let Some(custom_id) = component.custom_id.clone() else { return };
                let kind = component.kind;
                this.update(cx, |this, cx| {
                    this.store.update(cx, |s, cx| s.interact(channel_id, id, custom_id, kind, value, cx));
                });
            })));
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
            div()
                .id("avatar")
                .pt(px(2.))
                .cursor_pointer()
                .child(avatar(m.author.avatar_url(128), 40.))
                .on_click(show_profile)
                .context_menu({
                    let store = self.store.clone();
                    let author = m.author.id;
                    move |menu, _, cx| super::user_menu::user_menu(menu, store.clone(), author, guild, cx)
                })
                .into_any_element()
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
            let target = m.referenced_message.as_ref().map(|r| r.id);
            let this = this.clone();
            h_flex()
                .id("reply-preview")
                .when_some(target, |d, target| {
                    d.cursor_pointer().hover(|s| s.text_color(theme::text())).on_click(move |_, _, cx| {
                        this.update(cx, |this, cx| this.jump_to(channel_id, target, cx));
                    })
                })
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

        let used = m.interaction.as_ref().map(|interaction| {
            h_flex()
                .pl(px(52.))
                .gap_1()
                .text_size(px(12.))
                .text_color(theme::text_muted())
                .child(icon(IconName::SquareTerminal, 12., theme::text_muted()))
                .child(
                    div()
                        .flex_shrink_0()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(store.display_name(guild, &interaction.user)),
                )
                .child("used")
                .child(div().text_color(theme::accent_text()).child(format!("/{}", interaction.name)))
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
                    .child("😀")
                    .tooltip(tip("Add reaction"))
                    .on_click(move |_, window, cx| {
                        this.update(cx, |this, cx| this.open_picker(PickerTarget::React(id), window, cx));
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
            let in_guild = guild.is_some();
            let is_thread_channel = store.channels.get(&channel_id).is_some_and(|c| c.is_thread());
            // Discord suggests the message's own opening words as the thread name.
            let thread_name = {
                let first: String = m.content.lines().next().unwrap_or("").chars().take(60).collect();
                if first.trim().is_empty() { format!("Thread with {name}") } else { first }
            };
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
                let entity = this.clone();
                menu = menu.item(PopupMenuItem::new("Forward\u{2026}").on_click(move |_, _, cx| {
                    entity.update(cx, |_, cx| cx.emit(ChatEvent::Forward { channel: channel_id, message: id }));
                }));
                if in_guild && !is_thread_channel {
                    let entity = this.clone();
                    let name = thread_name.clone();
                    menu = menu.item(PopupMenuItem::new("Create Thread").on_click(move |_, _, cx| {
                        let name = name.clone();
                        entity.update(cx, |this, cx| {
                            this.store.update(cx, |s, cx| s.create_thread(channel_id, id, name, cx));
                        });
                    }));
                }
                let entity = this.clone();
                menu = menu.item(PopupMenuItem::new("Add Reaction\u{2026}").on_click(move |_, window, cx| {
                    entity.update(cx, |this, cx| this.open_picker(PickerTarget::React(id), window, cx));
                }));
                menu = menu.separator();
                let text = content.clone();
                menu = menu.item(PopupMenuItem::new("Copy Text").on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                }));
                menu = menu.item(PopupMenuItem::new("Copy Message ID").on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(id.to_string()));
                }));
                let link = format!(
                    "https://discord.com/channels/{}/{channel_id}/{id}",
                    guild.map_or_else(|| "@me".to_string(), |g| g.to_string())
                );
                menu = menu.item(PopupMenuItem::new("Copy Message Link").on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(link.clone()));
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

        let highlighted = self.reply_to == Some(id) || self.editing == Some(id) || self.flash == Some(id);
        let row = div()
            .id(("message", id.0))
            .group("message")
            .relative()
            .w_full()
            .px_4()
            .when(!compact, |d| d.mt_3())
            .py(px(2.))
            .when(msg.mentions_me, |d| {
                d.bg(theme::warning().opacity(0.10)).border_l_2().border_color(theme::warning())
            })
            .when(highlighted, |d| d.bg(theme::accent().opacity(0.10)))
            .hover(|s| s.bg(theme::bg_hover()))
            .children(used)
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
        let parent = channel.filter(|c| c.is_thread()).and_then(|c| c.parent_id).and_then(|p| store.channels.get(&p));
        let parent_link = parent.map(|p| (p.id, store.channel_title(p)));
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
        let pins_shown = self.panel.as_ref().is_some_and(|p| p.kind == PanelKind::Pins);
        let is_dm = channel.is_some_and(|c| c.is_dm());
        let is_bot = store.me.as_ref().is_some_and(|u| u.bot);

        h_flex()
            .h(px(48.))
            .px_4()
            .gap_2()
            .flex_shrink_0()
            .border_b_1()
            .border_color(theme::border())
            .child(icon(channel_icon(kind), 18., theme::text_muted()))
            .when_some(parent_link, |d, (parent_id, parent_title)| {
                d.child(
                    div()
                        .id("parent-channel")
                        .flex_shrink_0()
                        .max_w(px(180.))
                        .truncate()
                        .cursor_pointer()
                        .text_color(theme::text_muted())
                        .hover(|s| s.text_color(theme::text()))
                        .child(parent_title)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.store.update(cx, |store, cx| store.select_channel(parent_id, cx));
                        })),
                )
                .child(icon(IconName::ChevronRight, 14., theme::text_muted()))
            })
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
                    div().w(px(210.)).flex_shrink_0().child(
                        Input::new(&self.search)
                            .cleanable(true)
                            .prefix(icon(IconName::Search, 14., theme::text_muted())),
                    ),
                )
                .when(is_dm && !is_bot, |d| {
                    d.child(icon_button("call", IconName::Phone, "Start voice call").on_click(cx.listener(
                        |this, _, _, cx| {
                            let Some(channel) = this.channel else { return };
                            this.store.update(cx, |store, cx| store.start_call(channel, cx));
                        },
                    )))
                })
                .child(
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

    fn render_panel(&self, panel: &Panel, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let guild = self.channel.and_then(|c| store.channels.get(&c)).and_then(|c| c.guild_id);
        let mono = cx.theme().mono_font_family.clone();
        let empty = if panel.loading {
            "Loading\u{2026}"
        } else if panel.kind == PanelKind::Pins {
            "This channel has no pinned messages."
        } else {
            "No messages found."
        };
        v_flex()
            .id("panel")
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
                    .child(panel.title.clone()),
            )
            .when(panel.items.is_empty(), |d| {
                d.child(div().px_3().pb_3().text_color(theme::text_muted()).child(empty))
            })
            .children(panel.items.iter().enumerate().map(|(i, msg)| {
                let (channel, id) = (msg.m.channel_id, msg.m.id);
                let place = (panel.kind == PanelKind::Search)
                    .then(|| store.channels.get(&channel).map(|c| format!("#{}", store.channel_title(c))))
                    .flatten();
                v_flex()
                    .id(("panel-item", i))
                    .mx_2()
                    .mb_2()
                    .p_2()
                    .gap_1()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(theme::bg_chat())
                    .hover(|s| s.bg(theme::bg_hover()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.panel = None;
                        this.jump_to(channel, id, cx);
                    }))
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
                            )
                            .when_some(place, |d, place| {
                                d.child(div().text_size(px(11.)).text_color(theme::text_muted()).child(place))
                            }),
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
                        div()
                            .id("emoji")
                            .size(px(28.))
                            .flex()
                            .flex_shrink_0()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .cursor_pointer()
                            .text_size(px(17.))
                            .hover(|s| s.bg(theme::bg_active()))
                            .child("😀")
                            .tooltip(tip("Emoji"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_picker(PickerTarget::Composer, window, cx)
                            })),
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
        let call_panel = self.channel.and_then(|channel| {
            let view = this.clone();
            let on_profile: call::OnProfile = Rc::new(move |user, at, cx| {
                view.update(cx, |this, cx| this.show_profile(user, at, Anchor::TopLeft, cx));
            });
            call::call_panel(&self.store, channel, &self.stage_topic, on_profile, cx)
        });

        let is_forum =
            self.channel.and_then(|c| self.store.read(cx).channels.get(&c)).is_some_and(|c| c.is_forum());
        let list = if is_forum {
            self.render_forum(cx)
        } else if has_channel {
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

        let panel = self.panel.clone();
        let profile = self.profile;
        let picker_open = self.picker.is_some();
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
                if !this.composer_focused(window, cx) {
                    return;
                }
                if this.step_completion(-1, cx) {
                    cx.stop_propagation();
                } else if this.editing.is_none() && this.composer.read(cx).value().is_empty() {
                    this.edit_last_own(window, cx);
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &input::MoveDown, window, cx| {
                if this.composer_focused(window, cx) && this.step_completion(1, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &input::Enter, window, cx| {
                if let Some(selected) = this.completion.as_ref().map(|c| c.selected) {
                    this.accept_completion(selected, window, cx);
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &input::IndentInline, window, cx| {
                if let Some(selected) = this.completion.as_ref().map(|c| c.selected) {
                    this.accept_completion(selected, window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(self.render_header(cx))
            .children(call_panel)
            .child(list)
            .when(!is_forum, |d| d.child(self.render_composer(cx)))
            .when_some(self.completion.as_ref(), |d, completion| d.child(self.render_completion(completion, cx)))
            .when(picker_open, |d| d.child(self.render_picker(cx)))
            .when_some(panel, |d, panel| d.child(self.render_panel(&panel, cx)))
            .when_some(profile, |d, card| d.child(self.render_profile(card, cx)))
    }
}

#[derive(Clone, Copy)]
pub struct ProfileCard {
    user: Id,
    at: Point<Pixels>,
    /// Which corner of the card sits at the click: top-left opens it to the
    /// right, top-right to the left (as from the member list).
    side: Anchor,
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

fn media(id: impl Into<ElementId>, src: String, open: String, w: f32, h: f32, opener: Opener) -> impl IntoElement {
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
        .on_click(move |_, _, cx| opener(open.clone(), cx))
}

fn render_attachment(ix: usize, attachment: &Attachment, opener: Opener) -> AnyElement {
    if attachment.is_image() {
        let (w, h) = fit(attachment.width.unwrap_or(400), attachment.height.unwrap_or(300), 420., 320.);
        let base = if attachment.proxy_url.is_empty() { &attachment.url } else { &attachment.proxy_url };
        // Animated images lose their animation when resized by the proxy.
        let src = if attachment.filename.to_ascii_lowercase().ends_with(".gif") {
            attachment.url.clone()
        } else {
            sized_url(base, w, h)
        };
        return media(("attachment", ix), src, attachment.url.clone(), w, h, opener).into_any_element();
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

fn render_embed(ix: usize, embed: &Embed, mono: SharedString, opener: Opener) -> AnyElement {
    let has_text = embed.title.is_some()
        || embed.description.is_some()
        || embed.author.is_some()
        || !embed.fields.is_empty()
        || embed.footer.is_some();

    // Bare image/GIF links: show just the picture.
    if !has_text {
        if let Some(image) = embed.image.as_ref().or(embed.thumbnail.as_ref()) {
            let (w, h) = fit(image.width.unwrap_or(400), image.height.unwrap_or(300), 420., 320.);
            return media(("embed", ix), sized_url(image.best_url(), w, h), image.url.clone(), w, h, opener)
                .into_any_element();
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
        column = column.child(media(
            ("embed-image", ix),
            sized_url(image.best_url(), w, h),
            image.url.clone(),
            w,
            h,
            opener.clone(),
        ));
    }
    if let Some(footer) = &embed.footer {
        column = column.child(
            div().pt_1().text_size(px(11.)).text_color(theme::text_muted()).child(footer.text.clone()),
        );
    }

    let thumbnail = embed.image.is_none().then_some(embed.thumbnail.as_ref()).flatten().map(|thumb| {
        let (w, h) = fit(thumb.width.unwrap_or(80), thumb.height.unwrap_or(80), 80., 80.);
        media(("embed-thumb", ix), sized_url(thumb.best_url(), w, h), thumb.url.clone(), w, h, opener.clone())
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

fn render_poll(poll: &Poll, on_vote: Rc<dyn Fn(u32, &mut App)>) -> AnyElement {
    let total: u32 = poll.answers.iter().map(|a| poll.count(a.answer_id).0).sum();
    let closed = poll.results.as_ref().is_some_and(|r| r.is_finalized);
    v_flex()
        .mt_1()
        .max_w(px(440.))
        .p_3()
        .gap_2()
        .rounded_lg()
        .border_1()
        .border_color(theme::border())
        .bg(theme::bg_sidebar())
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::text_bright())
                .child(poll.question.text.clone().unwrap_or_default()),
        )
        .children(poll.answers.iter().map(|answer| {
            let (count, mine) = poll.count(answer.answer_id);
            let share = if total == 0 { 0. } else { count as f32 / total as f32 };
            let id = answer.answer_id;
            let on_vote = on_vote.clone();
            let glyph = answer.poll_media.emoji.as_ref().and_then(|e| e.name.clone()).filter(|_| {
                answer.poll_media.emoji.as_ref().is_some_and(|e| e.id.is_none())
            });
            div()
                .id(("poll-answer", id as usize))
                .relative()
                .h(px(36.))
                .rounded_md()
                .overflow_hidden()
                .border_1()
                .border_color(if mine { theme::accent() } else { theme::border() })
                .bg(theme::bg_input())
                .when(!closed, |d| d.cursor_pointer().hover(|s| s.border_color(theme::text_muted())))
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .h_full()
                        .w(relative(share))
                        .bg(theme::accent().opacity(if mine { 0.35 } else { 0.18 })),
                )
                .child(
                    h_flex()
                        .absolute()
                        .inset_0()
                        .px_3()
                        .gap_2()
                        .children(glyph)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(theme::text())
                                .child(answer.poll_media.text.clone().unwrap_or_default()),
                        )
                        .when(mine, |d| d.child(icon(IconName::Check, 14., theme::accent_text())))
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme::text_muted())
                                .child(format!("{count} \u{00b7} {:.0}%", share * 100.)),
                        ),
                )
                .on_click(move |_, _, cx| {
                    if !closed {
                        on_vote(id, cx)
                    }
                })
        }))
        .child(div().text_size(px(12.)).text_color(theme::text_muted()).child(format!(
            "{total} vote{}{}{}",
            if total == 1 { "" } else { "s" },
            if poll.allow_multiselect { " \u{00b7} choose any" } else { "" },
            if closed { " \u{00b7} closed" } else { "" }
        )))
        .into_any_element()
}

fn render_components(rows: &[Component], on_press: Rc<dyn Fn(&Component, Option<String>, &mut App)>) -> AnyElement {
    let mut key = 0usize;
    let mut next_key = || {
        key += 1;
        key
    };
    let mut column = v_flex().mt_1().gap_1p5();
    for row in rows {
        // Text blocks may appear at the top level; everything else sits in rows.
        if row.kind == component_kind::TEXT_DISPLAY {
            column = column.child(div().child(row.content.clone().unwrap_or_default()));
            continue;
        }
        let mut line = h_flex().flex_wrap().gap_2();
        for component in &row.components {
            match component.kind {
                component_kind::BUTTON => {
                    let is_link = component.style == component_kind::LINK_STYLE;
                    let (bg, fg) = match component.style {
                        1 => (theme::accent(), theme::on_accent()),
                        3 => (theme::online(), gpui::white()),
                        4 => (theme::danger(), gpui::white()),
                        _ => (theme::bg_active(), theme::text_bright()),
                    };
                    let glyph = component.emoji.as_ref().filter(|e| e.id.is_none()).and_then(|e| e.name.clone());
                    let pressed = component.clone();
                    let on_press = on_press.clone();
                    line = line.child(
                        h_flex()
                            .id(("component", next_key()))
                            .h(px(32.))
                            .px_3()
                            .gap_1p5()
                            .rounded_md()
                            .bg(bg)
                            .text_color(fg)
                            .font_weight(FontWeight::MEDIUM)
                            .map(|d| {
                                if component.disabled {
                                    d.opacity(0.5)
                                } else {
                                    d.cursor_pointer().hover(|s| s.opacity(0.85))
                                }
                            })
                            .children(glyph)
                            .children(component.label.clone())
                            .when(is_link, |d| d.child(icon(IconName::ExternalLink, 13., fg)))
                            .on_click(move |_, _, cx| {
                                if pressed.disabled {
                                    return;
                                }
                                match &pressed.url {
                                    Some(url) if is_link => cx.open_url(url),
                                    _ => on_press(&pressed, None, cx),
                                }
                            }),
                    );
                }
                component_kind::STRING_SELECT => {
                    let mut menu = v_flex().gap_1().child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme::text_muted())
                            .child(component.placeholder.clone().unwrap_or_else(|| "Choose an option".into())),
                    );
                    let mut options = h_flex().flex_wrap().gap_1p5();
                    for option in &component.options {
                        let pressed = component.clone();
                        let value = option.value.clone();
                        let on_press = on_press.clone();
                        options = options.child(
                            div()
                                .id(("component", next_key()))
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .border_1()
                                .border_color(theme::border())
                                .bg(theme::bg_input())
                                .cursor_pointer()
                                .hover(|s| s.bg(theme::bg_active()))
                                .child(option.label.clone())
                                .on_click(move |_, _, cx| on_press(&pressed, Some(value.clone()), cx)),
                        );
                    }
                    menu = menu.child(options);
                    line = line.child(menu);
                }
                component_kind::TEXT_DISPLAY => {
                    line = line.child(div().child(component.content.clone().unwrap_or_default()));
                }
                _ => {}
            }
        }
        column = column.child(line);
    }
    column.into_any_element()
}
