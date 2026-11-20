//! The server rail on the far left.

use super::{theme, widgets::*};
use crate::{
    discord::Id,
    store::{Store, StoreEvent},
};
use gpui::{prelude::*, *};
use gpui_component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::assets::IconName;

pub struct Rail {
    store: Entity<Store>,
    _subs: Vec<Subscription>,
}

impl Rail {
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        let subs = vec![cx.subscribe(&store, |_, _, event: &StoreEvent, cx| {
            if matches!(
                event,
                StoreEvent::Guilds | StoreEvent::Selection | StoreEvent::Unread | StoreEvent::Channels
            ) {
                cx.notify();
            }
        })];
        Self { store, _subs: subs }
    }

    fn item(
        &self,
        key: u64,
        target: Option<Id>,
        name: String,
        visual: AnyElement,
        selected: bool,
        unread: bool,
        mentions: u32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name_for_menu = name.clone();
        let pill_height = if selected {
            36.
        } else if unread {
            8.
        } else {
            0.
        };
        let row = div()
            .id(("rail-item", key))
            .relative()
            .w_full()
            .h(px(48.))
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top(px((48. - pill_height) / 2.))
                    .w(px(4.))
                    .h(px(pill_height))
                    .rounded_r_md()
                    .bg(theme::text_bright()),
            )
            .child(div().size(px(46.)).child(visual))
            .when(mentions > 0, |d| {
                d.child(div().absolute().right(px(8.)).bottom(px(0.)).child(badge(mentions)))
            })
            .tooltip(tip(name))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.store.update(cx, |store, cx| store.select_guild(target, cx));
            }));
        let Some(guild) = target else { return row.into_any_element() };
        let store = self.store.clone();
        let muted = store.read(cx).muted.contains(&guild);
        let guild_name = name_for_menu;
        row.context_menu(move |menu, _, _| {
            let (mute, leave) = (store.clone(), store.clone());
            let store = store.clone();
            let guild_name = guild_name.clone();
            menu.item(PopupMenuItem::new("Mark Server as Read").on_click(move |_, _, cx| {
                store.update(cx, |store, cx| store.mark_guild_read(guild, cx));
            }))
            .item(PopupMenuItem::new(if muted { "Unmute Server" } else { "Mute Server" }).on_click(
                move |_, _, cx| mute.update(cx, |store, cx| store.toggle_mute(guild, cx)),
            ))
            .item(PopupMenuItem::new("Leave Server").on_click(move |_, window, cx| {
                let answer = window.prompt(
                    PromptLevel::Warning,
                    &format!("Leave {guild_name}?"),
                    Some("You will need a new invite to rejoin."),
                    &["Leave", "Cancel"],
                    cx,
                );
                let leave = leave.clone();
                cx.spawn(async move |cx| {
                    if answer.await == Ok(0) {
                        let _ = leave.update(cx, |store, cx| store.leave_guild(guild, cx));
                    }
                })
                .detach();
            }))
            .separator()
            .item(PopupMenuItem::new("Copy Server ID").on_click(move |_, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(guild.to_string()));
            }))
        })
        .into_any_element()
    }
}

impl Render for Rail {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let selected = store.selected_guild;

        let home_selected = selected.is_none();
        let home_visual = div()
            .size_full()
            .map(|d| if home_selected { d.rounded_xl() } else { d.rounded_full() })
            .flex()
            .items_center()
            .justify_center()
            .bg(if home_selected { theme::accent() } else { theme::bg_chat() })
            .child(icon(
                IconName::MessageCircle,
                22.,
                if home_selected { theme::on_accent() } else { theme::text() },
            ))
            .into_any_element();
        let dm_unread = store.dm_unread();

        struct Entry {
            id: Id,
            name: String,
            icon: Option<String>,
            unread: bool,
            mentions: u32,
        }
        let entries: Vec<Entry> = store
            .guilds
            .iter()
            .map(|g| {
                let (unread, mentions) = store.guild_unread(g.guild.id);
                Entry { id: g.guild.id, name: g.guild.name.clone(), icon: g.guild.icon_url(128), unread, mentions }
            })
            .collect();

        let mut items = Vec::with_capacity(entries.len() + 2);
        items.push(
            self.item(0, None, "Direct Messages".into(), home_visual, home_selected, dm_unread > 0, dm_unread, cx)
,
        );
        items.push(
            div().w(px(32.)).h(px(2.)).flex_shrink_0().rounded_full().bg(theme::bg_active()).into_any_element(),
        );
        for entry in entries {
            let is_selected = selected == Some(entry.id);
            let radius = if is_selected { px(14.) } else { px(23.) };
            let visual = match entry.icon {
                Some(url) => img(url)
                    .size_full()
                    .rounded(radius)
                    .object_fit(ObjectFit::Cover)
                    .with_loading(move || div().size_full().rounded(radius).bg(theme::bg_chat()).into_any_element())
                    .into_any_element(),
                None => div()
                    .size_full()
                    .rounded(radius)
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(theme::bg_chat())
                    .text_size(px(14.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(initials(&entry.name))
                    .into_any_element(),
            };
            items.push(
                self.item(
                    entry.id.0,
                    Some(entry.id),
                    entry.name,
                    visual,
                    is_selected,
                    entry.unread,
                    entry.mentions,
                    cx,
                )
                .into_any_element(),
            );
        }

        div()
            .id("rail")
            .w(px(68.))
            .h_full()
            .flex_shrink_0()
            .bg(theme::bg_rail())
            .py_2()
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .overflow_y_scroll()
            .children(items)
    }
}
