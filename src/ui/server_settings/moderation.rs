//! Server settings tabs for AutoMod rules and member onboarding.

use super::{ServerSettings, checkbox};
use crate::{
    discord::{
        Id,
        automod::{self, Response, Rule},
        onboarding, permissions as p,
    },
    ui::{form, theme, widgets::*},
};
use gpui::{prelude::*, *};

const RESPONSES: [(&str, Response); 4] = [
    ("Block the message", Response::Block),
    ("Block and time out 1 minute", Response::BlockAndTimeout(60)),
    ("Block and time out 10 minutes", Response::BlockAndTimeout(600)),
    ("Block and time out 1 hour", Response::BlockAndTimeout(3600)),
];

fn pill(id: impl Into<ElementId>, label: String, on: bool) -> Stateful<Div> {
    div()
        .id(id)
        .px_2()
        .py_0p5()
        .rounded_md()
        .cursor_pointer()
        .text_size(px(12.))
        .border_1()
        .border_color(if on { theme::accent() } else { theme::border() })
        .when(on, |d| d.bg(theme::accent().opacity(0.18)).text_color(theme::text_bright()))
        .hover(|s| s.bg(theme::bg_hover()))
        .child(label)
}

fn label(text: &'static str) -> Div {
    div().pt_2().text_size(px(12.)).font_weight(FontWeight::BOLD).text_color(theme::text_muted()).child(text)
}

impl ServerSettings {
    pub(super) fn load_automod(&mut self, cx: &mut Context<Self>) {
        let (this, guild) = (cx.weak_entity(), self.guild);
        self.store.update(cx, |store, cx| {
            store.fetch_automod(guild, cx, move |rules, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.automod = Some(rules);
                    cx.notify();
                });
            });
        });
    }

    pub(super) fn load_onboarding(&mut self, cx: &mut Context<Self>) {
        let (this, guild) = (cx.weak_entity(), self.guild);
        self.store.update(cx, |store, cx| {
            store.fetch_onboarding(guild, cx, move |value, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.onboarding = Some(value);
                    this.onboarding_dirty = false;
                    cx.notify();
                });
            });
        });
    }

    fn put_rule(&mut self, rule: Rule, cx: &mut Context<Self>) {
        let list = self.automod.get_or_insert_with(Vec::new);
        match list.iter_mut().find(|r| r.id == rule.id) {
            Some(existing) => *existing = rule,
            None => list.push(rule),
        }
        cx.notify();
    }

    fn save_rule(&mut self, rule: Option<Id>, body: serde_json::Value, cx: &mut Context<Self>) {
        let (this, guild) = (cx.weak_entity(), self.guild);
        self.store.update(cx, |s, cx| {
            s.save_automod_rule(guild, rule, body, cx, move |rule, cx| {
                let _ = this.update(cx, |this, cx| this.put_rule(rule, cx));
            });
        });
    }

    /// The form for a new keyword rule, or for editing one.
    fn keyword_form(&mut self, rule: Option<Rule>, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.weak_entity();
        let editing = rule.as_ref().map(|r| r.id);
        let selected = rule
            .as_ref()
            .map(|r| r.response())
            .and_then(|resp| RESPONSES.iter().position(|(_, x)| *x == resp))
            .unwrap_or(0);
        let labels: Vec<&str> = RESPONSES.iter().map(|(l, _)| *l).collect();
        let spec = form::FormSpec::new(if editing.is_some() { "Edit Rule" } else { "New Keyword Rule" }, "Save", move |r, _, cx| {
            let name = r.values[0].clone();
            let words = automod::split_words(&r.values[1]);
            let allow = automod::split_words(&r.values[2]);
            if name.is_empty() || words.is_empty() {
                return;
            }
            let response = RESPONSES[r.choice.min(RESPONSES.len() - 1)].1;
            let body = match editing {
                Some(_) => automod::edit_keyword_rule(&name, &words, &allow, response),
                None => automod::new_keyword_rule(&name, &words, &allow, response),
            };
            let _ = this.update(cx, |this, cx| this.save_rule(editing, body, cx));
        })
        .description("Separate words with commas. Use * as a wildcard, as in *scam*.")
        .field(form::Field::new("Rule name", rule.as_ref().map(|r| r.name.clone()).unwrap_or_default()))
        .field(form::Field::new("Words to block", rule.as_ref().map(|r| r.keywords.join(", ")).unwrap_or_default()))
        .field(form::Field::new("Words to allow anyway", rule.as_ref().map(|r| r.allow.join(", ")).unwrap_or_default()))
        .choice("Response", &labels, selected);
        form::open_form(spec, window, cx);
    }

    pub(super) fn render_automod(&self, cx: &mut Context<Self>) -> AnyElement {
        if !self.store.read(cx).can(self.guild, p::MANAGE_GUILD) {
            return div().text_color(theme::text_muted()).child("You need Manage Server to change AutoMod.").into_any_element();
        }
        let Some(rules) = self.automod.clone() else {
            return div().text_color(theme::text_muted()).child("Loading AutoMod rules\u{2026}").into_any_element();
        };
        v_flex()
            .gap_2()
            .child(div().text_size(px(12.)).text_color(theme::text_muted()).child(
                "AutoMod blocks messages before anyone sees them. Moderators and admins are not affected.",
            ))
            .child(
                Self::button("automod-new", "New Keyword Rule", true)
                    .w(px(170.))
                    .on_click(cx.listener(|this, _, window, cx| this.keyword_form(None, window, cx))),
            )
            .when(rules.is_empty(), |d| d.child(div().text_color(theme::text_muted()).child("No rules yet.")))
            .children(rules.into_iter().map(|rule| {
                let id = rule.id;
                let enabled = rule.enabled;
                let editable = rule.trigger == automod::KEYWORD;
                let edit = rule.clone();
                h_flex()
                    .id(("rule", id.0))
                    .p_3()
                    .gap_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .id(("rule-enabled", id.0))
                            .cursor_pointer()
                            .child(checkbox(enabled))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(r) = this.automod.as_mut().and_then(|l| l.iter_mut().find(|r| r.id == id)) {
                                    r.enabled = !enabled;
                                }
                                this.save_rule(Some(id), serde_json::json!({ "enabled": !enabled }), cx);
                            })),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(div().font_weight(FontWeight::SEMIBOLD).text_color(theme::text_bright()).child(rule.name.clone()))
                                    .child(div().text_size(px(12.)).text_color(theme::text_muted()).child(rule.kind())),
                            )
                            .child(div().text_size(px(13.)).child(rule.summary()))
                            .child(div().text_size(px(12.)).text_color(theme::text_muted()).child(format!("When matched, it {}.", rule.does()))),
                    )
                    .when(editable, |d| {
                        d.child(Self::button(("rule-edit", id.0), "Edit", false).on_click(cx.listener(
                            move |this, _, window, cx| this.keyword_form(Some(edit.clone()), window, cx),
                        )))
                    })
                    .child(Self::button(("rule-delete", id.0), "Delete", false).on_click(cx.listener(
                        move |this, _, window, cx| {
                            let (store, guild, this_entity) = (this.store.clone(), this.guild, cx.weak_entity());
                            form::confirm(window, cx, "Delete rule?", "AutoMod stops checking for it.", "Delete", move |cx| {
                                store.update(cx, |s, cx| {
                                    s.delete_automod_rule(guild, id, cx, move |cx| {
                                        let _ = this_entity.update(cx, |this, cx| {
                                            if let Some(list) = this.automod.as_mut() {
                                                list.retain(|r| r.id != id);
                                            }
                                            cx.notify();
                                        });
                                    });
                                });
                            });
                        },
                    )))
            }))
            .into_any_element()
    }

    fn edit_onboarding(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut serde_json::Value)) {
        if let Some(o) = self.onboarding.as_mut() {
            change(o);
            self.onboarding_dirty = true;
            cx.notify();
        }
    }

    pub(super) fn render_onboarding(&self, cx: &mut Context<Self>) -> AnyElement {
        let store = self.store.read(cx);
        if !store.can(self.guild, p::MANAGE_GUILD | p::MANAGE_ROLES) {
            return div()
                .text_color(theme::text_muted())
                .child("You need Manage Server and Manage Roles to change onboarding.")
                .into_any_element();
        }
        let Some(o) = self.onboarding.clone() else {
            return div().text_color(theme::text_muted()).child("Loading onboarding\u{2026}").into_any_element();
        };
        let channels = self.text_channels(cx);
        let roles: Vec<(Id, String)> = store.ordered_roles(self.guild).into_iter().map(|r| (r.id, r.name)).collect();
        let defaults = onboarding::default_channels(&o);
        let prompts = onboarding::prompts(&o);
        let enabled = onboarding::enabled(&o);

        let mut col = v_flex()
            .gap_2()
            .child(div().text_size(px(12.)).text_color(theme::text_muted()).child(
                "New members pick answers to these questions, which give them roles and channels. \
                 Discord asks for at least seven default channels before onboarding can be turned on.",
            ))
            .child(
                h_flex()
                    .id("onboarding-enabled")
                    .gap_2()
                    .cursor_pointer()
                    .child(checkbox(enabled))
                    .child("Show onboarding to new members")
                    .on_click(cx.listener(move |this, _, _, cx| this.edit_onboarding(cx, |o| onboarding::set_enabled(o, !enabled)))),
            )
            .child(label("DEFAULT CHANNELS"))
            .child(h_flex().flex_wrap().gap_1().children(channels.iter().map(|(id, name)| {
                let id = *id;
                pill(("default-channel", id.0), format!("#{name}"), defaults.contains(&id))
                    .on_click(cx.listener(move |this, _, _, cx| this.edit_onboarding(cx, |o| onboarding::toggle_default_channel(o, id))))
            })))
            .child(label("QUESTIONS"));

        for (pi, prompt) in prompts.into_iter().enumerate() {
            let mut card = v_flex()
                .p_3()
                .gap_1p5()
                .rounded_lg()
                .border_1()
                .border_color(theme::border())
                .child(
                    h_flex()
                        .gap_2()
                        .child(div().flex_1().font_weight(FontWeight::SEMIBOLD).text_color(theme::text_bright()).child(prompt.title.clone()))
                        .child(div().text_size(px(12.)).text_color(theme::text_muted()).child(if prompt.single_select {
                            "One answer"
                        } else {
                            "Several answers"
                        }))
                        .child(
                            h_flex()
                                .id(("prompt-required", pi))
                                .gap_1()
                                .cursor_pointer()
                                .text_size(px(12.))
                                .child(checkbox(prompt.required))
                                .child("Required")
                                .on_click(cx.listener(move |this, _, _, cx| this.edit_onboarding(cx, |o| onboarding::toggle_required(o, pi)))),
                        )
                        .child(Self::button(("prompt-delete", pi), "Delete", false).on_click(cx.listener(move |this, _, _, cx| {
                            this.edit_onboarding(cx, |o| onboarding::remove_prompt(o, pi))
                        }))),
                );
            for (oi, option) in prompt.options.into_iter().enumerate() {
                card = card.child(
                    v_flex()
                        .pl_2()
                        .gap_1()
                        .child(div().text_color(theme::text_bright()).child(option.title.clone()))
                        .child(h_flex().flex_wrap().gap_1().children(roles.iter().map(|(rid, rname)| {
                            let rid = *rid;
                            pill(SharedString::from(format!("opt-role-{pi}-{oi}-{rid}")), format!("@{rname}"), option.roles.contains(&rid))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.edit_onboarding(cx, |o| onboarding::toggle_option_role(o, pi, oi, rid))
                                }))
                        })))
                        .child(h_flex().flex_wrap().gap_1().children(channels.iter().map(|(cid, cname)| {
                            let cid = *cid;
                            pill(SharedString::from(format!("opt-chan-{pi}-{oi}-{cid}")), format!("#{cname}"), option.channels.contains(&cid))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.edit_onboarding(cx, |o| onboarding::toggle_option_channel(o, pi, oi, cid))
                                }))
                        }))),
                );
            }
            col = col.child(card);
        }

        col.child(
            h_flex()
                .pt_2()
                .gap_2()
                .child(Self::button("prompt-add", "Add Question", false).on_click(cx.listener(|_, _, window, cx| {
                    let this_entity = cx.weak_entity();
                    let spec = form::FormSpec::new("Add Question", "Add", move |r, _, cx| {
                        let title = r.values[0].clone();
                        let answers = automod::split_words(&r.values[1]);
                        if title.is_empty() || answers.is_empty() {
                            return;
                        }
                        let single = r.choice == 0;
                        let _ = this_entity.update(cx, |this, cx| {
                            this.edit_onboarding(cx, |o| onboarding::add_prompt(o, &title, &answers, single))
                        });
                    })
                    .description("Give each answer roles or channels afterwards.")
                    .field(form::Field::new("Question", "").placeholder("What brings you here?"))
                    .field(form::Field::new("Answers, separated by commas", ""))
                    .choice("Members pick", &["One answer", "Several answers"], 0);
                    form::open_form(spec, window, cx);
                })))
                .when(self.onboarding_dirty, |d| {
                    d.child(Self::button("onboarding-save", "Save Changes", true).on_click(cx.listener(|this, _, _, cx| {
                        let Some(o) = this.onboarding.as_ref() else { return };
                        let body = onboarding::save_body(o);
                        let (this_entity, guild) = (cx.weak_entity(), this.guild);
                        this.store.update(cx, |s, cx| {
                            s.save_onboarding(guild, body, cx, move |value, cx| {
                                let _ = this_entity.update(cx, |this, cx| {
                                    // Keep local edits if the reply is empty.
                                    if value.is_object() {
                                        this.onboarding = Some(value);
                                    }
                                    this.onboarding_dirty = false;
                                    cx.notify();
                                });
                            });
                        });
                    })))
                    .child(Self::button("onboarding-discard", "Discard", false).on_click(cx.listener(|this, _, _, cx| this.load_onboarding(cx))))
                }),
        )
        .into_any_element()
    }
}
