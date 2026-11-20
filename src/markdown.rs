//! A small parser for Discord-flavoured markdown.
//!
//! The output is a list of blocks whose text carries flat, non-overlapping
//! style spans, which maps directly onto GPUI's styled text runs.

use crate::discord::Id;
use std::ops::Range;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub code: bool,
    pub spoiler: bool,
    pub mention: bool,
    /// Index into [`RichText::links`].
    pub link: Option<u32>,
}

impl Style {
    fn is_plain(&self) -> bool {
        *self == Style::default()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RichText {
    pub text: String,
    pub spans: Vec<(Range<usize>, Style)>,
    pub links: Vec<String>,
}

impl RichText {
    fn push(&mut self, s: &str, style: Style) {
        if s.is_empty() {
            return;
        }
        let start = self.text.len();
        self.text.push_str(s);
        if style.is_plain() {
            return;
        }
        if let Some((range, last)) = self.spans.last_mut() {
            if *last == style && range.end == start {
                range.end = self.text.len();
                return;
            }
        }
        self.spans.push((start..self.text.len(), style));
    }

    fn trim_end(&mut self) {
        let len = self.text.trim_end().len();
        self.text.truncate(len);
        self.spans.retain_mut(|(range, _)| {
            range.end = range.end.min(len);
            range.start < range.end
        });
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CustomEmoji {
    pub id: Id,
    pub name: String,
    pub animated: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Text(RichText),
    Heading(u8, RichText),
    /// Small, muted text (`-# `).
    Subtext(RichText),
    Quote(RichText),
    Code { lang: Option<String>, code: String },
    /// A message made only of custom emoji, shown enlarged.
    Emoji(Vec<CustomEmoji>),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Doc {
    pub blocks: Vec<Block>,
}

/// Resolves ids found in mentions to display names.
pub trait Resolver {
    fn user(&self, id: Id) -> Option<String>;
    fn channel(&self, id: Id) -> Option<String>;
    fn role(&self, id: Id) -> Option<String>;
}

pub struct NoResolver;

impl Resolver for NoResolver {
    fn user(&self, _: Id) -> Option<String> {
        None
    }
    fn channel(&self, _: Id) -> Option<String> {
        None
    }
    fn role(&self, _: Id) -> Option<String> {
        None
    }
}

pub fn parse(input: &str, resolver: &dyn Resolver) -> Doc {
    let mut doc = Doc::default();
    if input.is_empty() {
        return doc;
    }
    if let Some(emoji) = emoji_only(input) {
        doc.blocks.push(Block::Emoji(emoji));
        return doc;
    }

    // Split out fenced code blocks first; everything else is line based.
    let mut rest = input;
    while let Some(start) = rest.find("```") {
        let after = &rest[start + 3..];
        let Some(end) = after.find("```") else { break };
        parse_lines(&rest[..start], resolver, &mut doc);
        let body = &after[..end];
        let (lang, code) = match body.split_once('\n') {
            Some((first, code))
                if !first.is_empty() && first.chars().all(|c| c.is_ascii_alphanumeric() || "+-_#.".contains(c)) =>
            {
                (Some(first.to_string()), code)
            }
            _ => (None, body),
        };
        let code = code.trim_matches('\n').to_string();
        if !code.is_empty() {
            doc.blocks.push(Block::Code { lang, code });
        }
        rest = &after[end + 3..];
    }
    parse_lines(rest, resolver, &mut doc);
    doc
}

/// Parses a single line of inline markdown (used for embeds, topics, replies).
pub fn parse_inline_only(input: &str, resolver: &dyn Resolver) -> RichText {
    let mut out = RichText::default();
    inline(input, Style::default(), resolver, &mut out, 0);
    out
}

fn emoji_only(input: &str) -> Option<Vec<CustomEmoji>> {
    let mut rest = input.trim();
    let mut found = Vec::new();
    while !rest.is_empty() {
        let (emoji, len) = custom_emoji(rest)?;
        found.push(emoji);
        rest = rest[len..].trim_start();
        if found.len() > 27 {
            return None;
        }
    }
    (!found.is_empty()).then_some(found)
}

/// Parses `<:name:id>` or `<a:name:id>` at the start of `s`.
fn custom_emoji(s: &str) -> Option<(CustomEmoji, usize)> {
    let (animated, body) = if let Some(b) = s.strip_prefix("<a:") {
        (true, b)
    } else if let Some(b) = s.strip_prefix("<:") {
        (false, b)
    } else {
        return None;
    };
    let close = body.find('>')?;
    let (name, id) = body[..close].split_once(':')?;
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    let id = id.parse().ok()?;
    let len = s.len() - body.len() + close + 1;
    Some((CustomEmoji { id: Id(id), name: name.to_string(), animated }, len))
}

fn parse_lines(input: &str, resolver: &dyn Resolver, doc: &mut Doc) {
    let input = input.trim_matches('\n');
    if input.trim().is_empty() {
        return;
    }
    let mut para = String::new();
    let mut quote = String::new();

    fn flush_para(para: &mut String, resolver: &dyn Resolver, doc: &mut Doc) {
        if !para.trim().is_empty() {
            let mut text = RichText::default();
            inline(para.trim_end(), Style::default(), resolver, &mut text, 0);
            text.trim_end();
            doc.blocks.push(Block::Text(text));
        }
        para.clear();
    }
    fn flush_quote(quote: &mut String, resolver: &dyn Resolver, doc: &mut Doc) {
        if !quote.is_empty() {
            let mut text = RichText::default();
            inline(quote.trim_end(), Style::default(), resolver, &mut text, 0);
            text.trim_end();
            doc.blocks.push(Block::Quote(text));
        }
        quote.clear();
    }

    let mut lines = input.split('\n');
    while let Some(line) = lines.next() {
        if let Some(q) = line.strip_prefix(">>> ") {
            flush_para(&mut para, resolver, doc);
            quote.push_str(q);
            for l in lines.by_ref() {
                quote.push('\n');
                quote.push_str(l);
            }
            break;
        }
        if let Some(q) = line.strip_prefix("> ") {
            flush_para(&mut para, resolver, doc);
            if !quote.is_empty() {
                quote.push('\n');
            }
            quote.push_str(q);
            continue;
        }
        flush_quote(&mut quote, resolver, doc);

        let heading = [("### ", 3u8), ("## ", 2), ("# ", 1)]
            .iter()
            .find_map(|(p, level)| line.strip_prefix(p).map(|rest| (*level, rest)));
        if let Some((level, rest)) = heading {
            flush_para(&mut para, resolver, doc);
            let mut text = RichText::default();
            inline(rest.trim(), Style::default(), resolver, &mut text, 0);
            doc.blocks.push(Block::Heading(level, text));
            continue;
        }
        if let Some(rest) = line.strip_prefix("-# ") {
            flush_para(&mut para, resolver, doc);
            let mut text = RichText::default();
            inline(rest.trim(), Style::default(), resolver, &mut text, 0);
            doc.blocks.push(Block::Subtext(text));
            continue;
        }

        if !para.is_empty() {
            para.push('\n');
        }
        // Render list markers as bullets, preserving indentation.
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        match trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
            Some(item) if !item.trim().is_empty() => {
                para.push_str(&" ".repeat(indent + 1));
                para.push_str("\u{2022} ");
                para.push_str(item);
            }
            _ => para.push_str(line),
        }
    }
    flush_para(&mut para, resolver, doc);
    flush_quote(&mut quote, resolver, doc);
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric()
}

/// Finds the closing `delim` in `s`, skipping escaped characters and inline code.
fn find_close(s: &str, delim: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < s.len() {
        if bytes[i] == b'\\' {
            i += 1;
            i += s[i.min(s.len())..].chars().next().map_or(0, char::len_utf8);
            continue;
        }
        if s[i..].starts_with(delim) {
            // A single `*` must not match the start of a `**` pair.
            if delim == "*" && s[i..].starts_with("**") {
                i += 2;
                continue;
            }
            if i > 0 {
                return Some(i);
            }
        }
        i += s[i..].chars().next().map_or(1, char::len_utf8);
    }
    None
}

fn inline(input: &str, style: Style, resolver: &dyn Resolver, out: &mut RichText, depth: u8) {
    let mut plain_start = 0;
    let mut i = 0;
    let bytes = input.as_bytes();

    macro_rules! flush {
        () => {
            out.push(&input[plain_start..i], style);
        };
    }

    while i < input.len() {
        let rest = &input[i..];
        let c = bytes[i];
        let prev = input[..i].chars().next_back();

        // Escapes
        if c == b'\\' {
            if let Some(next) = rest[1..].chars().next() {
                if next.is_ascii_punctuation() {
                    flush!();
                    out.push(&rest[1..1 + next.len_utf8()], style);
                    i += 1 + next.len_utf8();
                    plain_start = i;
                    continue;
                }
            }
        }

        // Inline code
        if c == b'`' {
            let ticks = if rest.starts_with("``") { "``" } else { "`" };
            if let Some(end) = rest[ticks.len()..].find(ticks) {
                if end > 0 {
                    flush!();
                    let code = &rest[ticks.len()..ticks.len() + end];
                    out.push(code, Style { code: true, ..style });
                    i += ticks.len() * 2 + end;
                    plain_start = i;
                    continue;
                }
            }
        }

        // Paired delimiters
        if depth < 8 && matches!(c, b'*' | b'_' | b'~' | b'|') {
            let delims: &[(&str, fn(&mut Style))] = &[
                ("**", |s| s.bold = true),
                ("__", |s| s.underline = true),
                ("~~", |s| s.strike = true),
                ("||", |s| s.spoiler = true),
                ("*", |s| s.italic = true),
                ("_", |s| s.italic = true),
            ];
            let mut matched = false;
            for (delim, apply) in delims {
                if !rest.starts_with(delim) {
                    continue;
                }
                let body = &rest[delim.len()..];
                if body.starts_with(char::is_whitespace) && delim.len() == 1 {
                    continue;
                }
                // Underscores inside words (snake_case) are literal.
                if *delim == "_" && prev.is_some_and(is_word) {
                    continue;
                }
                let Some(end) = find_close(body, delim) else { continue };
                if *delim == "_" && body[end + 1..].chars().next().is_some_and(is_word) {
                    continue;
                }
                flush!();
                let mut inner = style;
                apply(&mut inner);
                inline(&body[..end], inner, resolver, out, depth + 1);
                i += delim.len() * 2 + end;
                plain_start = i;
                matched = true;
                break;
            }
            if matched {
                continue;
            }
        }

        // Angle-bracket entities: mentions, emoji, timestamps, bare links
        if c == b'<' {
            if let Some((text, len, entity_style)) = entity(rest, style, resolver, out) {
                flush!();
                out.push(&text, entity_style);
                i += len;
                plain_start = i;
                continue;
            }
        }

        // Masked links: [text](url)
        if c == b'[' {
            if let Some(close) = rest.find("](") {
                let label = &rest[1..close];
                let after = &rest[close + 2..];
                if let Some(end) = after.find(')') {
                    let url = after[..end].trim().trim_matches(|c| c == '<' || c == '>');
                    if !label.is_empty() && !label.contains('\n') && is_url(url) {
                        flush!();
                        out.links.push(url.to_string());
                        let link = Some(out.links.len() as u32 - 1);
                        inline(label, Style { link, ..style }, resolver, out, depth + 1);
                        i += close + 2 + end + 1;
                        plain_start = i;
                        continue;
                    }
                }
            }
        }

        // Bare URLs
        if c == b'h' && (rest.starts_with("http://") || rest.starts_with("https://")) && !prev.is_some_and(is_word) {
            let mut end = rest.find(|ch: char| ch.is_whitespace() || ch == '<' || ch == '>').unwrap_or(rest.len());
            while end > 0 && matches!(rest.as_bytes()[end - 1], b'.' | b',' | b';' | b':' | b'!' | b'?' | b'\'' | b'"') {
                end -= 1;
            }
            // Drop a trailing ')' unless the URL itself contains '('.
            if end > 0 && rest.as_bytes()[end - 1] == b')' && !rest[..end].contains('(') {
                end -= 1;
            }
            if end > "https://".len() {
                flush!();
                out.links.push(rest[..end].to_string());
                let link = Some(out.links.len() as u32 - 1);
                out.push(&rest[..end], Style { link, ..style });
                i += end;
                plain_start = i;
                continue;
            }
        }

        // @everyone / @here
        if c == b'@' && !prev.is_some_and(is_word) {
            if let Some(word) = ["@everyone", "@here"].iter().find(|w| rest.starts_with(**w)) {
                flush!();
                out.push(word, Style { mention: true, ..style });
                i += word.len();
                plain_start = i;
                continue;
            }
        }

        i += rest.chars().next().map_or(1, char::len_utf8);
    }
    i = input.len();
    flush!();
}

fn is_url(s: &str) -> bool {
    (s.starts_with("http://") || s.starts_with("https://")) && !s.contains(char::is_whitespace)
}

/// Parses `<...>` entities. Returns the display text, consumed length and style.
fn entity(rest: &str, style: Style, resolver: &dyn Resolver, out: &mut RichText) -> Option<(String, usize, Style)> {
    let close = rest.find('>')?;
    let body = &rest[1..close];
    let len = close + 1;
    let mention = Style { mention: true, ..style };
    let id = |s: &str| s.parse::<u64>().ok().map(Id);

    if let Some(r) = body.strip_prefix("@&") {
        let name = resolver.role(id(r)?).unwrap_or_else(|| "unknown-role".into());
        return Some((format!("@{name}"), len, mention));
    }
    if let Some(u) = body.strip_prefix("@") {
        let u = u.strip_prefix('!').unwrap_or(u);
        let name = resolver.user(id(u)?).unwrap_or_else(|| "unknown-user".into());
        return Some((format!("@{name}"), len, mention));
    }
    if let Some(c) = body.strip_prefix('#') {
        let name = resolver.channel(id(c)?).unwrap_or_else(|| "unknown-channel".into());
        return Some((format!("#{name}"), len, mention));
    }
    if let Some((emoji, elen)) = custom_emoji(rest) {
        return Some((format!(":{}:", emoji.name), elen, style));
    }
    if let Some(t) = body.strip_prefix("t:") {
        let (secs, format) = t.split_once(':').unwrap_or((t, "f"));
        let secs: i64 = secs.parse().ok()?;
        let time = chrono::DateTime::from_timestamp(secs, 0)?.with_timezone(&chrono::Local);
        let text = match format {
            "t" => time.format("%H:%M").to_string(),
            "T" => time.format("%H:%M:%S").to_string(),
            "d" => time.format("%Y-%m-%d").to_string(),
            "D" => time.format("%B %-d, %Y").to_string(),
            "F" => time.format("%A, %B %-d, %Y %H:%M").to_string(),
            "R" => relative_time(time),
            _ => time.format("%B %-d, %Y %H:%M").to_string(),
        };
        return Some((text, len, Style { code: true, ..style }));
    }
    if is_url(body) {
        out.links.push(body.to_string());
        let link = Some(out.links.len() as u32 - 1);
        return Some((body.to_string(), len, Style { link, ..style }));
    }
    None
}

fn relative_time(time: chrono::DateTime<chrono::Local>) -> String {
    let delta = chrono::Local::now().signed_duration_since(time);
    let secs = delta.num_seconds();
    let (n, unit) = match secs.abs() {
        s if s < 60 => (s, "second"),
        s if s < 3600 => (s / 60, "minute"),
        s if s < 86_400 => (s / 3600, "hour"),
        s if s < 2_592_000 => (s / 86_400, "day"),
        s if s < 31_536_000 => (s / 2_592_000, "month"),
        s => (s / 31_536_000, "year"),
    };
    let plural = if n == 1 { "" } else { "s" };
    if secs >= 0 {
        format!("{n} {unit}{plural} ago")
    } else {
        format!("in {n} {unit}{plural}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(input: &str) -> RichText {
        match parse(input, &NoResolver).blocks.into_iter().next() {
            Some(Block::Text(t)) => t,
            other => panic!("expected text block, got {other:?}"),
        }
    }

    #[test]
    fn plain_text_has_no_spans() {
        let t = text("hello world");
        assert_eq!(t.text, "hello world");
        assert!(t.spans.is_empty());
    }

    #[test]
    fn bold_and_italic() {
        let t = text("a **bold** and *it* _too_");
        assert_eq!(t.text, "a bold and it too");
        assert_eq!(t.spans.len(), 3);
        assert!(t.spans[0].1.bold && t.spans[0].0 == (2..6));
        assert!(t.spans[1].1.italic);
        assert!(t.spans[2].1.italic);
    }

    #[test]
    fn nested_styles() {
        let t = text("**bold *both* bold**");
        assert_eq!(t.text, "bold both bold");
        assert!(t.spans.iter().all(|(_, s)| s.bold));
        assert!(t.spans[1].1.italic);
    }

    #[test]
    fn snake_case_is_literal() {
        let t = text("some_var_name here");
        assert_eq!(t.text, "some_var_name here");
        assert!(t.spans.is_empty());
    }

    #[test]
    fn unclosed_delimiters_are_literal() {
        assert_eq!(text("2 * 3 = 6").text, "2 * 3 = 6");
        assert_eq!(text("**oops").text, "**oops");
    }

    #[test]
    fn inline_code_is_not_parsed() {
        let t = text("see `**x**` ok");
        assert_eq!(t.text, "see **x** ok");
        assert!(t.spans[0].1.code && !t.spans[0].1.bold);
    }

    #[test]
    fn code_block_with_language() {
        let doc = parse("before\n```rust\nfn main() {}\n```\nafter", &NoResolver);
        assert_eq!(doc.blocks.len(), 3);
        assert_eq!(
            doc.blocks[1],
            Block::Code { lang: Some("rust".into()), code: "fn main() {}".into() }
        );
    }

    #[test]
    fn urls_and_masked_links() {
        let t = text("go to https://example.com/a?b=1, or [here](https://x.y/z).");
        assert_eq!(t.text, "go to https://example.com/a?b=1, or here.");
        assert_eq!(t.links, vec!["https://example.com/a?b=1", "https://x.y/z"]);
        assert_eq!(t.spans[0].1.link, Some(0));
        assert_eq!(t.spans[1].1.link, Some(1));
    }

    #[test]
    fn mentions_resolve() {
        struct R;
        impl Resolver for R {
            fn user(&self, id: Id) -> Option<String> {
                (id == Id(1)).then(|| "ann".into())
            }
            fn channel(&self, _: Id) -> Option<String> {
                Some("general".into())
            }
            fn role(&self, _: Id) -> Option<String> {
                None
            }
        }
        let doc = parse("hi <@1> <@!2> in <#5> <@&9>", &R);
        let Block::Text(t) = &doc.blocks[0] else { panic!() };
        assert_eq!(t.text, "hi @ann @unknown-user in #general @unknown-role");
        assert!(t.spans.iter().all(|(_, s)| s.mention));
    }

    #[test]
    fn emoji_only_messages_are_enlarged() {
        let doc = parse("<:wave:123> <a:party:456>", &NoResolver);
        let Block::Emoji(list) = &doc.blocks[0] else { panic!() };
        assert_eq!(list.len(), 2);
        assert!(list[1].animated);
        assert_eq!(text("hi <:wave:123>").text, "hi :wave:");
    }

    #[test]
    fn quotes_and_headings() {
        let doc = parse("# Title\n> quoted\n> more\nplain\n-# small", &NoResolver);
        assert!(matches!(&doc.blocks[0], Block::Heading(1, t) if t.text == "Title"));
        assert!(matches!(&doc.blocks[1], Block::Quote(t) if t.text == "quoted\nmore"));
        assert!(matches!(&doc.blocks[2], Block::Text(t) if t.text == "plain"));
        assert!(matches!(&doc.blocks[3], Block::Subtext(t) if t.text == "small"));
    }

    #[test]
    fn escapes() {
        let t = text("\\*not italic\\*");
        assert_eq!(t.text, "*not italic*");
        assert!(t.spans.is_empty());
    }

    #[test]
    fn spans_stay_on_char_boundaries() {
        let t = text("héllo **wörld** ✨ ~~x~~");
        for (range, _) in &t.spans {
            assert!(t.text.is_char_boundary(range.start) && t.text.is_char_boundary(range.end));
        }
    }
}
