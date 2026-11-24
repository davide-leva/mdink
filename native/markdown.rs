use comrak::{
    Arena, Options,
    nodes::{AstNode, ListType, NodeValue, TableAlignment},
    parse_document,
};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Style {
    pub font_override: Option<usize>,
    pub bold: bool,
    pub italic: bool,
    pub mono: bool,
    pub strike: bool,
    pub link: Option<String>,
}

impl Style {
    pub fn font(&self) -> usize {
        if let Some(font) = self.font_override {
            return font;
        }
        if self.mono {
            4
        } else {
            usize::from(self.bold) + 2 * usize::from(self.italic)
        }
    }
}

#[derive(Clone, Debug)]
pub enum Inline {
    Text(String, Style),
    Break,
    Image(String),
}

#[derive(Clone, Debug)]
pub enum Block {
    Paragraph(Vec<Inline>),
    Heading(u8, Vec<Inline>),
    Code(String),
    Quote(Vec<Block>),
    List {
        start: Option<usize>,
        items: Vec<(Option<bool>, Vec<Block>)>,
    },
    Table(Vec<TableAlignment>, Vec<Vec<Vec<Inline>>>),
    Rule,
}

fn line_break_tag(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    let Some(inner) = lower.strip_prefix("<br").and_then(|s| s.strip_suffix('>')) else {
        return false;
    };
    inner.trim().trim_end_matches('/').trim().is_empty()
}

fn inline<'a>(node: &'a AstNode<'a>, style: Style, result: &mut Vec<Inline>) {
    let value = node.data.borrow().value.clone();
    let mut next = style.clone();
    match value {
        NodeValue::Text(text) => result.push(Inline::Text(text.into_owned(), style)),
        NodeValue::SoftBreak => result.push(Inline::Text(" ".into(), style)),
        NodeValue::LineBreak => result.push(Inline::Break),
        NodeValue::HtmlInline(text) => {
            if line_break_tag(&text) {
                result.push(Inline::Break);
            } else {
                result.push(Inline::Text(text, style));
            }
        }
        NodeValue::Code(code) => {
            next.mono = true;
            result.push(Inline::Text(code.literal, next));
        }
        NodeValue::Image(link) => result.push(Inline::Image(link.url)),
        NodeValue::FootnoteReference(note) => {
            result.push(Inline::Text(format!("[{}]", note.ix), style))
        }
        _ => {
            match value {
                NodeValue::Strong => next.bold = true,
                NodeValue::Emph => next.italic = true,
                NodeValue::Strikethrough => next.strike = true,
                NodeValue::Link(link) => next.link = Some(link.url),
                _ => {}
            }
            for child in node.children() {
                inline(child, next.clone(), result);
            }
        }
    }
}

fn spans<'a>(node: &'a AstNode<'a>) -> Vec<Inline> {
    let mut result = Vec::new();
    for child in node.children() {
        inline(child, Style::default(), &mut result);
    }
    result
}

fn blocks<'a>(node: &'a AstNode<'a>) -> Vec<Block> {
    let mut result = Vec::new();
    let mut notes = Vec::new();
    for child in node.children() {
        match &child.data.borrow().value {
            NodeValue::Paragraph => result.push(Block::Paragraph(spans(child))),
            NodeValue::Heading(h) => result.push(Block::Heading(h.level, spans(child))),
            NodeValue::CodeBlock(code) => result.push(Block::Code(code.literal.clone())),
            NodeValue::HtmlBlock(html) => {
                let mut text = html.literal.trim();
                let mut spans = Vec::new();
                while let Some(start) = text.find('<') {
                    if start > 0 {
                        spans.push(Inline::Text(text[..start].into(), Style::default()));
                    }
                    text = &text[start..];
                    let Some(end) = text.find('>') else {
                        break;
                    };
                    let tag = &text[..=end];
                    if line_break_tag(tag) {
                        spans.push(Inline::Break);
                    } else {
                        spans.push(Inline::Text(tag.into(), Style::default()));
                    }
                    text = &text[end + 1..];
                }
                if !text.is_empty() {
                    spans.push(Inline::Text(text.into(), Style::default()));
                }
                result.push(Block::Paragraph(spans));
            }
            NodeValue::ThematicBreak => result.push(Block::Rule),
            NodeValue::BlockQuote => result.push(Block::Quote(blocks(child))),
            NodeValue::List(list) => {
                let items = child
                    .children()
                    .map(|item| {
                        let checked = match &item.data.borrow().value {
                            NodeValue::TaskItem(task) => Some(task.symbol.is_some()),
                            _ => None,
                        };
                        (checked, blocks(item))
                    })
                    .collect();
                result.push(Block::List {
                    start: (list.list_type == ListType::Ordered).then_some(list.start),
                    items,
                });
            }
            NodeValue::Table(table) => {
                let rows = child
                    .children()
                    .map(|row| row.children().map(spans).collect())
                    .collect();
                result.push(Block::Table(table.alignments.clone(), rows));
            }
            NodeValue::FootnoteDefinition(note) => {
                let mut content = blocks(child);
                let label = Inline::Text(format!("[{}] ", note.name), Style::default());
                if let Some(Block::Paragraph(spans)) = content.first_mut() {
                    spans.insert(0, label);
                }
                notes.extend(content);
            }
            _ => result.extend(blocks(child)),
        }
    }
    if !notes.is_empty() {
        result.push(Block::Rule);
        result.extend(notes);
    }
    result
}

pub fn parse(source: &str) -> Vec<Block> {
    let arena = Arena::new();
    let mut options = Options::default();
    options.extension.table = true;
    options.extension.strikethrough = true;
    options.extension.autolink = true;
    options.extension.tasklist = true;
    options.extension.footnotes = true;
    let root = parse_document(&arena, source, &options);
    let notes: std::collections::HashMap<_, _> = root
        .descendants()
        .filter_map(|node| {
            if let NodeValue::FootnoteReference(note) = &node.data.borrow().value {
                Some((note.name.clone(), note.ix))
            } else {
                None
            }
        })
        .collect();
    for node in root.descendants() {
        if let NodeValue::FootnoteDefinition(note) = &mut node.data.borrow_mut().value
            && let Some(index) = notes.get(&note.name)
        {
            note.name = index.to_string();
        }
    }
    blocks(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn breaks_and_literal_code() {
        let result = parse(
            "prima<br />seconda<BR>terza\n\n`<br />`\n\n```\n<br />\n```\n\n<script>x</script>",
        );
        let Block::Paragraph(spans) = &result[0] else {
            panic!()
        };
        assert_eq!(
            spans.iter().filter(|s| matches!(s, Inline::Break)).count(),
            2
        );
        assert!(
            matches!(&result[1], Block::Paragraph(s) if matches!(&s[0], Inline::Text(t, style) if t == "<br />" && style.mono))
        );
        assert!(matches!(&result[2], Block::Code(s) if s.contains("<br />")));
        assert!(
            matches!(&result[3], Block::Paragraph(s) if matches!(&s[0], Inline::Text(t, _) if t.contains("<script>")))
        );
    }
}
