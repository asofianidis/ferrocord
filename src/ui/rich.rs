//! Renders parsed markdown ([`Doc`]) into GPUI elements.

use super::{theme, widgets::*};
use crate::markdown::{Block, Doc, RichText, Style};
use gpui::{prelude::*, *};
use std::rc::Rc;

pub type Reveal = Rc<dyn Fn(&mut Window, &mut App)>;

fn highlight(style: &Style, revealed: bool) -> HighlightStyle {
    let mut h = HighlightStyle::default();
    if style.bold {
        h.font_weight = Some(FontWeight::BOLD);
    }
    if style.italic {
        h.font_style = Some(FontStyle::Italic);
    }
    if style.underline || style.link.is_some() {
        h.underline = Some(UnderlineStyle { thickness: px(1.), ..Default::default() });
    }
    if style.strike {
        h.strikethrough = Some(StrikethroughStyle { thickness: px(1.), ..Default::default() });
    }
    if style.code {
        h.background_color = Some(theme::bg_code());
    }
    if style.link.is_some() {
        h.color = Some(theme::link());
    }
    if style.mention {
        h.color = Some(theme::accent_text());
        h.background_color = Some(theme::accent().opacity(0.18));
        h.font_weight = Some(FontWeight::MEDIUM);
    }
    if style.spoiler {
        if revealed {
            h.background_color = Some(theme::bg_active());
        } else {
            // Hidden: text is painted in its own background colour.
            h.color = Some(theme::bg_code());
            h.background_color = Some(theme::bg_code());
        }
    }
    h
}

pub fn render_text(id: usize, text: &RichText, revealed: bool, on_reveal: Option<Reveal>) -> AnyElement {
    let highlights: Vec<_> = text.spans.iter().map(|(range, s)| (range.clone(), highlight(s, revealed))).collect();
    let styled = StyledText::new(SharedString::from(text.text.clone())).with_highlights(highlights);

    let mut ranges = Vec::new();
    let mut targets: Vec<Option<String>> = Vec::new();
    for (range, style) in &text.spans {
        if style.spoiler && !revealed {
            ranges.push(range.clone());
            targets.push(None);
        } else if let Some(link) = style.link.and_then(|l| text.links.get(l as usize)) {
            ranges.push(range.clone());
            targets.push(Some(link.clone()));
        }
    }
    if ranges.is_empty() {
        return styled.into_any_element();
    }
    InteractiveText::new(id, styled)
        .on_click(ranges, move |ix, window, cx| match targets.get(ix) {
            Some(Some(url)) => cx.open_url(url),
            Some(None) => {
                if let Some(reveal) = &on_reveal {
                    reveal(window, cx)
                }
            }
            None => {}
        })
        .into_any_element()
}

pub fn render_doc(
    doc: &Doc,
    revealed: bool,
    on_reveal: Option<Reveal>,
    mono_font: SharedString,
) -> Vec<AnyElement> {
    doc.blocks
        .iter()
        .enumerate()
        .map(|(ix, block)| match block {
            Block::Text(text) => div()
                .w_full()
                .child(render_text(ix, text, revealed, on_reveal.clone()))
                .into_any_element(),
            Block::Heading(level, text) => div()
                .w_full()
                .pt_1()
                .text_color(theme::text_bright())
                .font_weight(FontWeight::BOLD)
                .text_size(px(match level {
                    1 => 22.,
                    2 => 18.,
                    _ => 16.,
                }))
                .child(render_text(ix, text, revealed, on_reveal.clone()))
                .into_any_element(),
            Block::Subtext(text) => div()
                .w_full()
                .text_size(px(12.))
                .text_color(theme::text_muted())
                .child(render_text(ix, text, revealed, on_reveal.clone()))
                .into_any_element(),
            Block::Quote(text) => h_flex()
                .w_full()
                .items_stretch()
                .gap_2()
                .child(div().w(px(4.)).flex_shrink_0().rounded_sm().bg(theme::bg_active()))
                .child(div().flex_1().min_w_0().child(render_text(ix, text, revealed, on_reveal.clone())))
                .into_any_element(),
            Block::Code { code, .. } => div()
                .w_full()
                .my_1()
                .px_2()
                .py_1p5()
                .rounded_md()
                .border_1()
                .border_color(theme::border())
                .bg(theme::bg_code())
                .font_family(mono_font.clone())
                .text_size(px(13.))
                .child(StyledText::new(SharedString::from(code.clone())))
                .into_any_element(),
            Block::Emoji(list) => h_flex()
                .gap_1()
                .flex_wrap()
                .children(list.iter().map(|emoji| {
                    let ext = if emoji.animated { "gif" } else { "png" };
                    let url = format!("https://cdn.discordapp.com/emojis/{}.{ext}?size=128", emoji.id);
                    let name = format!(":{}:", emoji.name);
                    div()
                        .id(SharedString::from(format!("emoji-{ix}-{}", emoji.id)))
                        .child(img(url).size(px(48.)).object_fit(ObjectFit::Contain))
                        .tooltip(tip(name))
                }))
                .into_any_element(),
        })
        .collect()
}
