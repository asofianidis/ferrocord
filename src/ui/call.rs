//! Voice UI: the call panel shown above a voice, stage or call conversation,
//! and the connection panel at the bottom of the sidebar.

use super::{theme, widgets::*};
use crate::{
    discord::{Id, channel_kind},
    store::{Store, VoiceMember, VoiceStatus},
};
use gpui::{prelude::*, *};
use gpui_component::{
    input::{Input, InputState},
    menu::{ContextMenuExt as _, PopupMenuItem},
};
use gpui_kit::assets::IconName;
use std::rc::Rc;

/// Opens a user's profile card at a window position.
pub type OnProfile = Rc<dyn Fn(Id, Point<Pixels>, &mut App)>;

/// An avatar with a green ring while its owner is speaking.
pub fn voice_avatar(url: impl Into<SharedString>, size: f32, speaking: bool) -> impl IntoElement {
    div()
        .flex_shrink_0()
        .rounded_full()
        .p(px(2.))
        .border_2()
        .border_color(if speaking { theme::online() } else { gpui::transparent_black() })
        .child(avatar(url, size))
}

/// Small mute and deafen markers for someone in voice.
pub fn voice_flags(member: &VoiceMember) -> Div {
    let muted = member.self_mute || member.mute;
    let deafened = member.self_deaf || member.deaf;
    let server = |on: bool| if on { theme::danger() } else { theme::text_muted() };
    h_flex()
        .gap_1()
        .when(muted, |d| d.child(icon(IconName::MicOff, 13., server(member.mute))))
        .when(deafened, |d| d.child(icon(IconName::HeadphoneOff, 13., server(member.deaf))))
        .when(member.streaming, |d| d.child(icon(IconName::MonitorUp, 13., theme::danger())))
        .when(member.video, |d| d.child(icon(IconName::Video, 13., theme::text_muted())))
}

fn button(id: impl Into<ElementId>, label: impl Into<SharedString>, style: ButtonStyle) -> Stateful<Div> {
    let (bg, fg) = match style {
        ButtonStyle::Primary => (theme::accent(), theme::on_accent()),
        ButtonStyle::Danger => (theme::danger(), gpui::white()),
        ButtonStyle::Plain => (theme::bg_input(), theme::text()),
    };
    div()
        .id(id)
        .px_3()
        .py_1p5()
        .rounded_md()
        .cursor_pointer()
        .bg(bg)
        .text_color(fg)
        .font_weight(FontWeight::MEDIUM)
        .hover(|s| s.opacity(0.85))
        .child(label.into())
}

#[derive(Clone, Copy)]
enum ButtonStyle {
    Primary,
    Danger,
    Plain,
}

fn status_text(status: &VoiceStatus) -> (String, Hsla) {
    match status {
        VoiceStatus::Joining => ("Joining\u{2026}".into(), theme::warning()),
        VoiceStatus::Connecting => ("Connecting\u{2026}".into(), theme::warning()),
        VoiceStatus::Connected => ("Voice Connected".into(), theme::online()),
        VoiceStatus::Failed(reason) => (format!("Voice failed: {reason}"), theme::danger()),
    }
}

/// The panel above a voice channel's, stage's or call's messages.
pub fn call_panel(
    store_entity: &Entity<Store>,
    channel: Id,
    topic_input: &Entity<InputState>,
    on_profile: OnProfile,
    cx: &App,
) -> Option<AnyElement> {
    let store = store_entity.read(cx);
    let info = store.channels.get(&channel)?;
    let members = store.voice_members(channel).to_vec();
    let is_dm = info.is_dm();
    let is_stage = info.kind == channel_kind::STAGE;
    if !info.is_voice() && !(is_dm && (store.calls.contains_key(&channel) || !members.is_empty())) {
        return None;
    }
    let guild = info.guild_id;
    let me = store.my_id();
    let joined = store.voice_session.as_ref().filter(|s| s.channel == channel);
    let status = joined.map(|s| s.status.clone());
    let is_bot_on_dm = is_dm && store.me.as_ref().is_some_and(|u| u.bot);
    let moderator = guild.is_some() && store.is_stage_moderator(channel);
    let topic = store.stages.get(&channel).cloned();
    let my_member = members.iter().find(|m| m.user == me).cloned();

    let tile = |member: &VoiceMember, size: f32| -> AnyElement {
        let user = member.user;
        let (name, url) = match store.users.get(&user) {
            Some(u) => (store.display_name(guild, u), u.avatar_url(128)),
            None => ("Unknown".to_string(), String::new()),
        };
        let speaking = store.speaking.contains(&user);
        let on_profile = on_profile.clone();
        let element = v_flex()
            .id(("voice-tile", user.0))
            .w(px(size + 40.))
            .items_center()
            .gap_1()
            .cursor_pointer()
            .child(voice_avatar(url, size, speaking))
            .child(
                h_flex()
                    .max_w(px(size + 40.))
                    .gap_1()
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.))
                            .text_color(if speaking { theme::text_bright() } else { theme::text() })
                            .child(name),
                    )
                    .child(voice_flags(member))
                    .when(member.hand_raised, |d| d.child(icon(IconName::Hand, 13., theme::warning()))),
            )
            .on_click(move |event, _, cx| on_profile(user, event.position(), cx));
        let entity = store_entity.clone();
        let member = member.clone();
        let stage_moderator = is_stage && moderator && user != me;
        element
            .context_menu(move |menu, _, cx| {
                let mut menu = menu;
                if stage_moderator {
                    let entity = entity.clone();
                    let on_stage = member.suppress;
                    let label = if on_stage { "Invite to Speak" } else { "Move to Audience" };
                    menu = menu
                        .item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                            entity.update(cx, |s, cx| s.set_on_stage(channel, Some(member.user), on_stage, cx));
                        }))
                        .separator();
                }
                super::user_menu::user_menu(menu, entity.clone(), user, guild, cx)
            })
            .into_any_element()
    };

    let mut actions = h_flex().gap_2().flex_wrap();
    let entity = store_entity.clone();
    match &status {
        None if is_bot_on_dm => {}
        None => {
            let label = if is_stage {
                "Join Stage"
            } else if is_dm {
                if members.is_empty() { "Start Call" } else { "Join Call" }
            } else {
                "Join Voice"
            };
            actions = actions.child(button("voice-join", label, ButtonStyle::Primary).on_click(move |_, _, cx| {
                entity.update(cx, |s, cx| {
                    if is_dm { s.start_call(channel, cx) } else { s.join_voice(channel, cx) }
                });
            }));
        }
        Some(status) => {
            let (text, color) = status_text(status);
            actions = actions
                .child(div().text_size(px(12.)).text_color(color).child(text))
                .child(button("voice-leave", if is_stage { "Leave Quietly" } else { "Disconnect" }, ButtonStyle::Danger)
                    .on_click(move |_, _, cx| entity.update(cx, |s, cx| s.leave_voice(cx))));
            if is_stage {
                if let Some(mine) = &my_member {
                    let entity = store_entity.clone();
                    if mine.suppress {
                        if moderator {
                            actions = actions.child(button("stage-speak", "Speak", ButtonStyle::Primary).on_click(
                                move |_, _, cx| entity.update(cx, |s, cx| s.set_on_stage(channel, None, true, cx)),
                            ));
                        } else {
                            let raised = mine.hand_raised;
                            let label = if raised { "Lower Hand" } else { "Raise Hand" };
                            actions = actions.child(button("stage-hand", label, ButtonStyle::Plain).on_click(
                                move |_, _, cx| entity.update(cx, |s, cx| s.set_hand_raised(channel, !raised, cx)),
                            ));
                        }
                    } else {
                        actions = actions.child(
                            button("stage-audience", "Move to Audience", ButtonStyle::Plain).on_click(
                                move |_, _, cx| entity.update(cx, |s, cx| s.set_on_stage(channel, None, false, cx)),
                            ),
                        );
                    }
                }
            }
        }
    }
    if is_stage && moderator {
        let entity = store_entity.clone();
        if topic.is_some() {
            actions = actions.child(
                button("stage-end", "End Stage", ButtonStyle::Plain)
                    .on_click(move |_, _, cx| entity.update(cx, |s, cx| s.end_stage(channel, cx))),
            );
        } else {
            let input = topic_input.clone();
            let fallback = info.name.clone().unwrap_or_else(|| "Stage".into());
            actions = actions
                .child(div().w(px(220.)).child(Input::new(topic_input)))
                .child(button("stage-start", "Start Stage", ButtonStyle::Primary).on_click(move |_, _, cx| {
                    let typed = input.read(cx).value().trim().to_string();
                    let topic = if typed.is_empty() { fallback.clone() } else { typed };
                    entity.update(cx, |s, cx| s.start_stage(channel, topic, cx));
                }));
        }
    }

    let section = |title: String| {
        div()
            .pt_1()
            .text_size(px(11.))
            .font_weight(FontWeight::BOLD)
            .text_color(theme::text_muted())
            .child(title.to_uppercase())
    };
    let mut body = v_flex().gap_2();
    let header_title;
    if is_stage {
        let speakers: Vec<_> = members.iter().filter(|m| !m.suppress).collect();
        let audience: Vec<_> = members.iter().filter(|m| m.suppress).collect();
        let hands: Vec<_> = audience.iter().filter(|m| m.hand_raised).collect();
        header_title = h_flex()
            .gap_2()
            .child(icon(IconName::Radio, 18., if topic.is_some() { theme::online() } else { theme::text_muted() }))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::text_bright())
                    .child(topic.clone().unwrap_or_else(|| "This stage is not live.".into())),
            );
        body = body
            .child(section(format!("Speakers \u{2014} {}", speakers.len())))
            .child(h_flex().flex_wrap().gap_3().children(speakers.iter().map(|m| tile(m, 56.))))
            .child(section(format!("Audience \u{2014} {}", audience.len())))
            .child(h_flex().flex_wrap().gap_2().children(audience.iter().map(|m| tile(m, 32.))));
        if moderator && !hands.is_empty() {
            body = body.child(section(format!("Raised hands \u{2014} {}", hands.len()))).children(hands.iter().map(
                |m| {
                    let user = m.user;
                    let name = store.users.get(&user).map(|u| store.display_name(guild, u)).unwrap_or_default();
                    let entity = store_entity.clone();
                    h_flex()
                        .gap_2()
                        .child(icon(IconName::Hand, 14., theme::warning()))
                        .child(div().flex_1().child(name))
                        .child(button(("accept-speaker", user.0), "Invite to Speak", ButtonStyle::Primary).on_click(
                            move |_, _, cx| entity.update(cx, |s, cx| s.set_on_stage(channel, Some(user), true, cx)),
                        ))
                },
            ));
        }
    } else {
        let heading = if members.is_empty() {
            "Nobody is here yet.".to_string()
        } else {
            format!("{} in voice", members.len())
        };
        header_title = h_flex().child(section(heading));
        body = body.child(h_flex().flex_wrap().gap_3().children(members.iter().map(|m| tile(m, 48.))));
    }

    Some(
        v_flex()
            .flex_shrink_0()
            .max_h(px(380.))
            .id("call-panel")
            .overflow_y_scroll()
            .px_4()
            .py_3()
            .gap_2()
            .border_b_1()
            .border_color(theme::border())
            .bg(theme::bg_sidebar())
            // Controls stay at the top so a long participant list cannot push them out of view.
            .child(h_flex().gap_3().flex_wrap().child(div().flex_1().min_w(px(160.)).child(header_title)).child(actions))
            .child(body)
            .into_any_element(),
    )
}

/// The "Voice Connected" panel above the user panel.
pub fn connection_panel(store_entity: &Entity<Store>, cx: &App) -> Option<AnyElement> {
    let store = store_entity.read(cx);
    let session = store.voice_session.as_ref()?;
    let channel = store.channels.get(&session.channel);
    let place = match channel {
        Some(c) => match c.guild_id.and_then(|g| store.guild(g)) {
            Some(guild) => format!("{} / {}", store.channel_title(c), guild.guild.name),
            None => store.channel_title(c),
        },
        None => "Voice".into(),
    };
    let (text, color) = status_text(&session.status);
    let target = session.channel;
    let open = store_entity.clone();
    let leave = store_entity.clone();
    Some(
        h_flex()
            .px_3()
            .py_2()
            .gap_2()
            .flex_shrink_0()
            .border_t_1()
            .border_color(theme::border())
            .bg(theme::bg_rail())
            .child(icon(IconName::AudioLines, 18., color))
            .child(
                v_flex()
                    .id("voice-connection")
                    .flex_1()
                    .min_w_0()
                    .cursor_pointer()
                    .child(div().truncate().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(color).child(text))
                    .child(div().truncate().text_size(px(12.)).text_color(theme::text_muted()).child(place))
                    .on_click(move |_, _, cx| open.update(cx, |s, cx| s.select_channel(target, cx))),
            )
            .child(
                icon_button("voice-disconnect", IconName::PhoneOff, "Disconnect")
                    .on_click(move |_, _, cx| leave.update(cx, |s, cx| s.leave_voice(cx))),
            )
            .into_any_element(),
    )
}
