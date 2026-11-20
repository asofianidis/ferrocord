//! A server's scheduled events: browse, mark interest, and for organisers
//! create, start, end, cancel and delete them.

use super::{
    form::{self, Field, FormSpec},
    theme,
    widgets::*,
};
use crate::{
    discord::{
        Id,
        events::{self, Place, ScheduledEvent},
    },
    store::{Store, StoreEvent},
};
use gpui::{prelude::*, *};
use gpui_kit::assets::IconName;

/// Create, edit and run events.
pub const MANAGE_EVENTS: u64 = 1 << 33;

pub enum EventsEvent {
    Close,
}

pub struct EventsView {
    store: Entity<Store>,
    guild: Id,
    _subs: Vec<Subscription>,
}

impl EventEmitter<EventsEvent> for EventsView {}

impl EventsView {
    pub fn new(store: Entity<Store>, guild: Id, cx: &mut Context<Self>) -> Self {
        let subs = vec![cx.subscribe(&store, |_, _, _: &StoreEvent, cx| cx.notify())];
        store.update(cx, |s, cx| s.refresh_events(guild, cx));
        Self { store, guild, _subs: subs }
    }

    fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let store = self.store.read(cx);
        let mut places: Vec<(Id, bool, String)> = store
            .channels
            .values()
            .filter(|c| c.guild_id == Some(self.guild) && (c.kind == 2 || c.kind == 13))
            .map(|c| (c.id, c.kind == 13, c.name.clone().unwrap_or_default()))
            .collect();
        places.sort_by(|a, b| a.2.cmp(&b.2));
        let mut labels = vec!["Somewhere else".to_string()];
        labels.extend(places.iter().map(|(_, stage, name)| if *stage { format!("Stage: {name}") } else { format!("Voice: {name}") }));
        let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let tomorrow = chrono::Local::now().date_naive() + chrono::Duration::days(1);
        let default_start = format!("{} 19:00", tomorrow.format("%Y-%m-%d"));
        let (handle, guild) = (self.store.clone(), self.guild);
        let spec = FormSpec::new("Create Event", "Create", move |r, _, cx| {
            let [name, description, start, location] = <[String; 4]>::try_from(r.values).unwrap_or_default();
            let report = |cx: &mut App, msg: &str| handle.update(cx, |_, cx| cx.emit(StoreEvent::Error(msg.into())));
            if name.is_empty() {
                return report(cx, "Give the event a name.");
            }
            let Some(start) = events::parse_local(&start) else {
                return report(cx, "Write the start as YYYY-MM-DD HH:MM, for example 2026-12-24 18:30.");
            };
            let place = match r.choice {
                0 if location.is_empty() => return report(cx, "Say where the event happens."),
                0 => Place::Elsewhere { location, end: start + chrono::Duration::hours(1) },
                n => {
                    let Some((id, stage, _)) = places.get(n - 1) else { return };
                    Place::Channel { id: *id, stage: *stage }
                }
            };
            let body = events::create_body(&name, &description, start, &place);
            handle.update(cx, |s, cx| s.create_event(guild, body, cx));
        })
        .description("Events somewhere else last an hour; organisers can end them early.")
        .field(Field::new("Name", ""))
        .field(Field::new("Description", ""))
        .field(Field::new("Starts (YYYY-MM-DD HH:MM)", default_start))
        .field(Field::new("Location, if somewhere else", ""))
        .choice("Where", &label_refs, 0);
        form::open_form(spec, window, cx);
    }

    fn small_button(id: impl Into<ElementId>, label: &'static str, primary: bool) -> Stateful<Div> {
        div()
            .id(id)
            .px_2p5()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .text_size(px(13.))
            .map(|d| if primary { d.bg(theme::accent()).text_color(theme::on_accent()) } else { d.bg(theme::bg_input()) })
            .hover(|s| s.opacity(0.85))
            .child(label)
    }

    fn event_card(&self, event: ScheduledEvent, can_manage: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let id = event.id;
        let interested = store.interested.contains(&id);
        let when = match (event.start(), event.end()) {
            (Some(s), Some(e)) if s.date_naive() == e.date_naive() => {
                format!("{} \u{2013} {}", s.format("%a %b %-d, %H:%M"), e.format("%H:%M"))
            }
            (Some(s), _) => s.format("%a %b %-d, %H:%M").to_string(),
            _ => String::new(),
        };
        let place = match event.entity_type {
            events::EXTERNAL => event.location().unwrap_or_default(),
            _ => event
                .channel_id
                .and_then(|c| store.channels.get(&c))
                .and_then(|c| c.name.clone())
                .unwrap_or_else(|| "a voice channel".into()),
        };
        let guild = self.guild;
        let status = event.status;
        v_flex()
            .p_3()
            .gap_1p5()
            .rounded_lg()
            .border_1()
            .border_color(if event.is_live() { theme::online() } else { theme::border() })
            .bg(theme::bg_chat())
            .child(
                h_flex()
                    .gap_2()
                    .text_size(px(12.))
                    .text_color(if event.is_live() { theme::online() } else { theme::accent_text() })
                    .child(if event.is_live() { "HAPPENING NOW".to_string() } else { when.to_uppercase() }),
            )
            .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).text_color(theme::text_bright()).child(event.name.clone()))
            .children(event.description.clone().filter(|d| !d.is_empty()).map(|d| div().text_color(theme::text()).child(d)))
            .child(
                h_flex()
                    .gap_3()
                    .text_size(px(12.))
                    .text_color(theme::text_muted())
                    .child(h_flex().gap_1().child(icon(IconName::Volume, 12., theme::text_muted())).child(place))
                    .child(h_flex().gap_1().child(icon(IconName::Users, 12., theme::text_muted())).child(format!(
                        "{} interested",
                        event.user_count.unwrap_or(0)
                    ))),
            )
            .child(
                h_flex()
                    .pt_1()
                    .gap_2()
                    .child(
                        Self::small_button(("interest", id.0), if interested { "\u{2713} Interested" } else { "Interested" }, interested)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.store.update(cx, |s, cx| s.toggle_interest(guild, id, cx));
                            })),
                    )
                    .when(can_manage && status == 1, |d| {
                        d.child(Self::small_button(("start", id.0), "Start", false).on_click(cx.listener(move |this, _, _, cx| {
                            this.store.update(cx, |s, cx| s.set_event_status(guild, id, 2, cx));
                        })))
                        .child(Self::small_button(("cancel", id.0), "Cancel Event", false).on_click(cx.listener(
                            move |this, _, _, cx| this.store.update(cx, |s, cx| s.set_event_status(guild, id, 4, cx)),
                        )))
                    })
                    .when(can_manage && status == 2, |d| {
                        d.child(Self::small_button(("end", id.0), "End Event", false).on_click(cx.listener(move |this, _, _, cx| {
                            this.store.update(cx, |s, cx| s.set_event_status(guild, id, 3, cx));
                        })))
                    })
                    .when(can_manage, |d| {
                        d.child(Self::small_button(("delete-event", id.0), "Delete", false).on_click(cx.listener(
                            move |this, _, window, cx| {
                                let store = this.store.clone();
                                form::confirm(window, cx, "Delete event?", "Interested members are no longer reminded.", "Delete", move |cx| {
                                    store.update(cx, |s, cx| s.delete_event(guild, id, cx));
                                });
                            },
                        )))
                    }),
            )
    }
}

impl Render for EventsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let can_manage = store.can(self.guild, MANAGE_EVENTS);
        let events = store.upcoming_events(self.guild);
        let cards: Vec<_> = events.into_iter().map(|e| self.event_card(e, can_manage, cx).into_any_element()).collect();
        div()
            .id("events-backdrop")
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::black().opacity(0.55))
            .on_click(cx.listener(|_, _, _, cx| cx.emit(EventsEvent::Close)))
            .child(
                v_flex()
                    .id("events")
                    .w(px(560.))
                    .max_h(px(620.))
                    .p_5()
                    .gap_3()
                    .rounded_xl()
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::bg_sidebar())
                    .shadow_lg()
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().flex_1().text_size(px(20.)).font_weight(FontWeight::BOLD).text_color(theme::text_bright()).child("Events"))
                            .when(can_manage, |d| {
                                d.child(
                                    Self::small_button("create-event", "Create Event", true)
                                        .on_click(cx.listener(|this, _, window, cx| this.create(window, cx))),
                                )
                            })
                            .child(icon_button("events-close", IconName::X, "Close").on_click(cx.listener(|_, _, _, cx| cx.emit(EventsEvent::Close)))),
                    )
                    .child(
                        v_flex()
                            .id("event-list")
                            .flex_1()
                            .min_h_0()
                            .gap_3()
                            .overflow_y_scroll()
                            .when(cards.is_empty(), |d| d.child(div().text_color(theme::text_muted()).child("No upcoming events.")))
                            .children(cards),
                    ),
            )
    }
}
