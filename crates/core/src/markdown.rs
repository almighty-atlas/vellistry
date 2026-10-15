use crate::TocEntry;
use pulldown_cmark::{html, CowStr, Event, Options, Parser, Tag, TagEnd};
use std::collections::HashMap;

pub(crate) struct Block {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

pub(crate) struct Rendered {
    pub html: String,
    pub title: String,
    pub toc: Vec<TocEntry>,
    pub blocks: Vec<Block>,
}

fn text_of(events: &[(Event<'_>, std::ops::Range<usize>)]) -> String {
    let mut text = String::new();
    for (event, _) in events {
        match event {
            Event::Text(value)
            | Event::Code(value)
            | Event::InlineMath(value)
            | Event::DisplayMath(value) => text.push_str(value),
            // Inline boundaries do not introduce whitespace in rendered text.
            Event::End(
                TagEnd::Emphasis
                | TagEnd::Strong
                | TagEnd::Strikethrough
                | TagEnd::Link
                | TagEnd::Image,
            ) => {}
            Event::SoftBreak | Event::HardBreak | Event::End(_) => text.push(' '),
            Event::TaskListMarker(done) => text.push_str(if *done { "[x] " } else { "[ ] " }),
            _ => {}
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn slug(text: &str) -> String {
    let mut value = String::new();
    let mut separator = false;
    for c in text.chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() || c == '_' {
            if separator && !value.is_empty() {
                value.push('-');
            }
            value.push(c);
            separator = false;
        } else if c.is_whitespace() || c == '-' {
            separator = true;
        }
    }
    if value.is_empty() {
        "section".into()
    } else {
        value
    }
}

// Slice rather than rewrite frontmatter so parser ranges can be translated back
// into byte offsets in the unchanged source, including BOMs and CRLF line endings.
fn frontmatter_end(source: &str) -> usize {
    let bom = if source.starts_with('\u{feff}') {
        '\u{feff}'.len_utf8()
    } else {
        0
    };
    let mut lines = source[bom..].split_inclusive('\n');
    let Some(first) = lines.next() else {
        return 0;
    };
    if first.trim_end_matches(['\r', '\n']) != "---" {
        return 0;
    }
    let mut end = bom + first.len();
    for line in lines {
        end += line.len();
        if matches!(line.trim_end_matches(['\r', '\n']), "---" | "...") {
            return end;
        }
    }
    // An unclosed delimiter is ordinary Markdown, not frontmatter.
    0
}

fn escape_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn local_image_path(destination: &str) -> bool {
    !destination.is_empty()
        && !destination.starts_with('/')
        && !destination.contains(['\\', ':', '?'])
        && !destination.chars().any(char::is_control)
        && url::Url::parse(destination).is_err()
}

pub(crate) fn render(source: &str, fallback_title: &str) -> Rendered {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_GFM;
    let source_base = frontmatter_end(source);
    let mut events: Vec<_> = Parser::new_ext(&source[source_base..], options)
        .into_offset_iter()
        .map(|(event, range)| (event, range.start + source_base..range.end + source_base))
        // Never accept raw HTML, including forged local-image placeholders.
        .filter(|(event, _)| !matches!(event, Event::Html(_) | Event::InlineHtml(_)))
        .collect();
    let mut toc = Vec::new();
    let mut used = std::collections::HashSet::new();
    for i in 0..events.len() {
        if let Event::Start(Tag::Heading { level, .. }) = &events[i].0 {
            let level = *level as u8;
            let end = (i + 1..events.len())
                .find(|&j| matches!(events[j].0, Event::End(TagEnd::Heading(_))))
                .unwrap_or(i);
            let text = text_of(&events[i + 1..end]);
            let base = slug(&text);
            let mut id = base.clone();
            let mut n = 1;
            while !used.insert(id.clone()) {
                id = format!("{base}-{n}");
                n += 1;
            }
            toc.push(TocEntry {
                level,
                id: id.clone(),
                text,
                source_start: events[i].1.start,
            });
            if let Event::Start(Tag::Heading { id: existing, .. }) = &mut events[i].0 {
                *existing = Some(CowStr::from(id));
            }
        }
    }
    let title = toc
        .iter()
        .find(|entry| entry.level == 1)
        .map(|entry| entry.text.clone())
        .unwrap_or_else(|| fallback_title.into());
    let mut ranges = Vec::new();
    let mut depth = 0;
    let mut begin = 0;
    for (i, (event, _)) in events.iter().enumerate() {
        match event {
            Event::Start(_) => {
                if depth == 0 {
                    begin = i;
                }
                depth += 1;
            }
            Event::End(_) => {
                depth -= 1;
                if depth == 0 {
                    ranges.push((begin, i));
                }
            }
            _ if depth == 0 => ranges.push((i, i)),
            _ => {}
        }
    }
    let mut before = HashMap::new();
    let mut after = std::collections::HashSet::new();
    let mut blocks = Vec::new();
    for (begin, end) in ranges {
        let start = events[begin].1.start;
        let finish = events[end].1.end;
        before.insert(
            begin,
            format!("<div data-source-start=\"{start}\" data-source-end=\"{finish}\">"),
        );
        after.insert(end);
        let text = text_of(&events[begin..=end]);
        if !text.is_empty() {
            blocks.push(Block {
                start,
                end: finish,
                text,
            });
        }
    }
    if blocks.is_empty() {
        blocks.push(Block {
            start: source_base,
            end: source.len(),
            text: String::new(),
        });
    }
    let mut images = HashMap::new();
    let mut image_ends = HashMap::new();
    for (i, (event, _)) in events.iter().enumerate() {
        if let Event::Start(Tag::Image { dest_url, .. }) = event {
            let mut depth = 1;
            let end = (i + 1..events.len())
                .find(|&j| {
                    match events[j].0 {
                        Event::Start(Tag::Image { .. }) => depth += 1,
                        Event::End(TagEnd::Image) => depth -= 1,
                        _ => {}
                    }
                    depth == 0
                })
                .unwrap_or(i);
            let alt = text_of(&events[i + 1..end]);
            let placeholder = if local_image_path(dest_url) {
                Event::Html(
                    format!(
                        "<img data-local-src=\"{}\" alt=\"{}\">",
                        escape_attribute(dest_url),
                        escape_attribute(&alt)
                    )
                    .into(),
                )
            } else {
                Event::Text(alt.into())
            };
            images.insert(i, placeholder);
            image_ends.insert(i, end);
        }
    }
    let mut output = Vec::new();
    let mut skip_image_until = None;
    for (i, (event, _)) in events.into_iter().enumerate() {
        if let Some(prefix) = before.remove(&i) {
            output.push(Event::Html(prefix.into()));
        }
        if skip_image_until.is_none_or(|end| i > end) {
            if let Some(image) = images.remove(&i) {
                skip_image_until = image_ends.remove(&i);
                output.push(image);
            } else {
                // Preserve task state without allowing interactive HTML input elements.
                output.push(match event {
                    Event::TaskListMarker(done) => {
                        Event::Text(if done { "☑ " } else { "☐ " }.into())
                    }
                    other => other,
                });
            }
        }
        if after.contains(&i) {
            output.push(Event::Html("</div>".into()));
        }
    }
    let mut raw = String::new();
    html::push_html(&mut raw, output.into_iter());
    let html = ammonia::Builder::default()
        .rm_tag_attributes("img", &["src", "srcset", "title", "width", "height"])
        .add_tag_attributes("img", &["data-local-src", "alt"])
        .add_tag_attributes("blockquote", &["class"])
        .add_generic_attributes(&["data-source-start", "data-source-end"])
        .add_tag_attributes("h1", &["id"])
        .add_tag_attributes("h2", &["id"])
        .add_tag_attributes("h3", &["id"])
        .add_tag_attributes("h4", &["id"])
        .add_tag_attributes("h5", &["id"])
        .add_tag_attributes("h6", &["id"])
        .clean(&raw)
        .to_string();
    Rendered {
        html,
        title,
        toc,
        blocks,
    }
}
