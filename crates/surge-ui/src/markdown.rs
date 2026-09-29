//! Markdown → gpui element renderer.
//!
//! Parses markdown text via `pulldown-cmark` and produces gpui `Div` elements
//! with proper styling for headers, bold, italic, code blocks, inline code,
//! tables, lists, blockquotes, and horizontal rules.

use gpui_kit::*;
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

use crate::theme;

/// Render markdown text into a list of gpui elements.
pub fn render_markdown(text: &str) -> Div {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let parser = Parser::new_ext(text, options);

    let mut renderer = MarkdownRenderer::new();
    for event in parser {
        renderer.process(event);
    }
    renderer.finish()
}

struct MarkdownRenderer {
    /// Top-level container
    root: Vec<AnyElement>,
    /// Current inline text accumulator
    inline_buf: Vec<InlineSpan>,
    /// Stack of active formatting
    format_stack: Vec<FormatTag>,
    /// Current list state
    list_stack: Vec<ListKind>,
    /// Marker stays beside wrapping item text instead of becoming a flex span.
    list_marker: Option<String>,
    /// Code block accumulator
    code_buf: Option<CodeBlock>,
    /// Table state
    table: Option<TableState>,
    /// Are we inside a blockquote?
    in_blockquote: bool,
}

#[derive(Clone)]
struct InlineSpan {
    text: String,
    bold: bool,
    italic: bool,
    strikethrough: bool,
    code: bool,
    link: bool,
}

#[derive(Clone)]
enum FormatTag {
    Bold,
    Italic,
    Strikethrough,
    Link(String),
}

enum ListKind {
    Ordered(u64),
    Unordered,
}

struct CodeBlock {
    lang: Option<String>,
    content: String,
}

struct TableState {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
    current_row: Vec<String>,
    current_cell: String,
    in_head: bool,
}

impl MarkdownRenderer {
    fn new() -> Self {
        Self {
            root: Vec::new(),
            inline_buf: Vec::new(),
            format_stack: Vec::new(),
            list_stack: Vec::new(),
            list_marker: None,
            code_buf: None,
            table: None,
            in_blockquote: false,
        }
    }

    fn is_bold(&self) -> bool {
        self.format_stack
            .iter()
            .any(|f| matches!(f, FormatTag::Bold))
    }

    fn is_italic(&self) -> bool {
        self.format_stack
            .iter()
            .any(|f| matches!(f, FormatTag::Italic))
    }

    fn is_link(&self) -> bool {
        self.format_stack.iter().any(|f| matches!(f, FormatTag::Link(_)))
    }

    fn is_strikethrough(&self) -> bool {
        self.format_stack
            .iter()
            .any(|f| matches!(f, FormatTag::Strikethrough))
    }

    fn push_text(&mut self, text: &str) {
        // If inside a code block, append to code buffer
        if let Some(code) = &mut self.code_buf {
            code.content.push_str(text);
            return;
        }

        // If inside a table cell, append to cell
        if let Some(table) = &mut self.table {
            table.current_cell.push_str(text);
            return;
        }

        self.inline_buf.push(InlineSpan {
            text: text.to_string(),
            bold: self.is_bold(),
            italic: self.is_italic(),
            strikethrough: self.is_strikethrough(),
            code: false,
            link: self.is_link(),
        });
    }

    fn flush_inline(&mut self) -> Option<Div> {
        if self.inline_buf.is_empty() {
            return None;
        }

        let spans: Vec<_> = std::mem::take(&mut self.inline_buf);
        // One text run per paragraph, styled by range, so a paragraph wraps
        // as a whole. (Separate boxes per span wrapped each span on its own
        // and pushed the next span — inline code, bold — onto a new line.)
        let mut text = String::new();
        let mut highlights: Vec<(std::ops::Range<usize>, HighlightStyle)> = Vec::new();
        for span in spans {
            let start = text.len();
            text.push_str(&span.text);
            let range = start..text.len();
            let mut style = HighlightStyle::default();
            let mut styled = false;
            if span.bold {
                style.font_weight = Some(FontWeight::BOLD);
                styled = true;
            }
            if span.italic {
                style.font_style = Some(FontStyle::Italic);
                styled = true;
            }
            if span.strikethrough {
                style.strikethrough = Some(StrikethroughStyle {
                    thickness: px(1.0),
                    color: None,
                });
                styled = true;
            }
            if span.link {
                style.color = Some(theme::accent());
                style.underline = Some(UnderlineStyle {
                    thickness: px(1.0),
                    color: Some(theme::accent().opacity(0.5)),
                    wavy: false,
                });
                styled = true;
            }
            if span.code {
                style.color = Some(theme::warning());
                style.background_color = Some(theme::sidebar_bg());
                styled = true;
            }
            if styled && !range.is_empty() {
                highlights.push((range, style));
            }
        }
        let line = div()
            .w_full()
            .min_w_0()
            .child(StyledText::new(text).with_highlights(highlights));

        Some(if let Some(marker) = self.list_marker.take() {
            div()
                .w_full()
                .min_w_0()
                .flex()
                .items_start()
                .gap(px(6.0))
                .child(div().flex_shrink_0().child(marker))
                .child(line.flex_1())
        } else {
            line
        })
    }

    fn flush_paragraph(&mut self) {
        if let Some(line) = self.flush_inline() {
            let el = line.mb(px(8.0));
            if self.in_blockquote {
                self.root.push(
                    div()
                        .pl(px(12.0))
                        .border_l_2()
                        .border_color(theme::text_muted())
                        .text_color(theme::text_muted())
                        .mb(px(8.0))
                        .child(el)
                        .into_any_element(),
                );
            } else {
                self.root.push(el.into_any_element());
            }
        }
    }

    fn process(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self.start_tag(tag),
            Event::End(tag) => self.end_tag(tag),
            Event::Text(text) => self.push_text(&text),
            Event::Code(code) => {
                // If inside table cell
                if let Some(table) = &mut self.table {
                    table.current_cell.push_str(&code);
                    return;
                }
                self.inline_buf.push(InlineSpan {
                    text: code.to_string(),
                    bold: false,
                    italic: false,
                    strikethrough: false,
                    code: true,
                    link: false,
                });
            },
            Event::SoftBreak => self.push_text(" "),
            Event::HardBreak => {
                self.flush_paragraph();
            },
            Event::Rule => {
                self.flush_paragraph();
                self.root.push(
                    div()
                        .w_full()
                        .h(px(1.0))
                        .bg(theme::text_muted())
                        .my(px(12.0))
                        .into_any_element(),
                );
            },
            Event::TaskListMarker(checked) => {
                let marker = if checked { "☑ " } else { "☐ " };
                self.push_text(marker);
            },
            _ => {},
        }
    }

    fn start_tag(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => {},
            Tag::Heading { level, .. } => {
                self.flush_paragraph();
                // heading level will be handled in end_tag
                let _ = level;
            },
            Tag::Strong => self.format_stack.push(FormatTag::Bold),
            Tag::Emphasis => self.format_stack.push(FormatTag::Italic),
            Tag::Strikethrough => self.format_stack.push(FormatTag::Strikethrough),
            Tag::Link { dest_url, .. } => {
                self.format_stack
                    .push(FormatTag::Link(dest_url.to_string()));
            },
            Tag::CodeBlock(kind) => {
                self.flush_paragraph();
                let lang = match kind {
                    CodeBlockKind::Fenced(lang) => {
                        let l = lang.to_string();
                        if l.is_empty() { None } else { Some(l) }
                    },
                    CodeBlockKind::Indented => None,
                };
                self.code_buf = Some(CodeBlock {
                    lang,
                    content: String::new(),
                });
            },
            Tag::List(start) => {
                self.flush_paragraph();
                match start {
                    Some(n) => self.list_stack.push(ListKind::Ordered(n)),
                    None => self.list_stack.push(ListKind::Unordered),
                }
            },
            Tag::Item => {
                // Add bullet/number prefix
                let prefix = match self.list_stack.last_mut() {
                    Some(ListKind::Unordered) => "• ".to_string(),
                    Some(ListKind::Ordered(n)) => {
                        let s = format!("{}. ", n);
                        *n += 1;
                        s
                    },
                    None => "• ".to_string(),
                };
                let indent = self.list_stack.len().saturating_sub(1);
                let padding = "  ".repeat(indent);
                self.list_marker = Some(format!("{padding}{prefix}"));
            },
            Tag::BlockQuote(_) => {
                self.flush_paragraph();
                self.in_blockquote = true;
            },
            Tag::Table(_) => {
                self.flush_paragraph();
                self.table = Some(TableState {
                    headers: Vec::new(),
                    rows: Vec::new(),
                    current_row: Vec::new(),
                    current_cell: String::new(),
                    in_head: false,
                });
            },
            Tag::TableHead => {
                if let Some(t) = &mut self.table {
                    t.in_head = true;
                }
            },
            Tag::TableRow => {
                if let Some(t) = &mut self.table {
                    t.current_row.clear();
                }
            },
            Tag::TableCell => {
                if let Some(t) = &mut self.table {
                    t.current_cell.clear();
                }
            },
            _ => {},
        }
    }

    fn end_tag(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush_paragraph();
            },
            TagEnd::Heading(level) => {
                let spans: Vec<_> = std::mem::take(&mut self.inline_buf);
                let text: String = spans.iter().map(|s| s.text.as_str()).collect();

                let (size, weight) = match level as u8 {
                    1 => (px(24.0), FontWeight::BOLD),
                    2 => (px(20.0), FontWeight::BOLD),
                    3 => (px(17.0), FontWeight::SEMIBOLD),
                    4 => (px(15.0), FontWeight::SEMIBOLD),
                    _ => (px(14.0), FontWeight::SEMIBOLD),
                };

                self.root.push(
                    div()
                        .text_size(size)
                        .font_weight(weight)
                        .text_color(theme::text_primary())
                        .mt(px(16.0))
                        .mb(px(8.0))
                        .child(text)
                        .into_any_element(),
                );
            },
            TagEnd::Strong => {
                self.format_stack.retain(|f| !matches!(f, FormatTag::Bold));
            },
            TagEnd::Emphasis => {
                self.format_stack
                    .retain(|f| !matches!(f, FormatTag::Italic));
            },
            TagEnd::Strikethrough => {
                self.format_stack
                    .retain(|f| !matches!(f, FormatTag::Strikethrough));
            },
            TagEnd::Link => {
                self.format_stack
                    .retain(|f| !matches!(f, FormatTag::Link(_)));
            },
            TagEnd::CodeBlock => {
                if let Some(code) = self.code_buf.take() {
                    let content = code.content.trim_end().to_string();
                    let lang_label = code.lang.unwrap_or_default();

                    let mut block = div()
                        .w_full()
                        .rounded(px(6.0))
                        .bg(hsla(0.0, 0.0, 0.08, 1.0))
                        .border_1()
                        .border_color(hsla(0.0, 0.0, 0.2, 1.0))
                        .mb(px(8.0))
                        .overflow_x_hidden();

                    // Language label
                    if !lang_label.is_empty() {
                        block = block.child(
                            div()
                                .px(px(12.0))
                                .py(px(4.0))
                                .text_xs()
                                .text_color(theme::text_muted())
                                .border_b_1()
                                .border_color(hsla(0.0, 0.0, 0.2, 1.0))
                                .child(lang_label),
                        );
                    }

                    // Code content
                    block = block.child(
                        div()
                            .px(px(12.0))
                            .py(px(8.0))
                            .font_family(crate::ui::MONO)
                            .text_sm()
                            .text_color(hsla(0.0, 0.0, 0.85, 1.0))
                            .child(content),
                    );

                    self.root.push(block.into_any_element());
                }
            },
            TagEnd::List(_) => {
                self.list_stack.pop();
                if self.list_stack.is_empty() {
                    self.root.push(div().mb(px(8.0)).into_any_element());
                }
            },
            TagEnd::Item => {
                self.flush_paragraph();
                self.list_marker = None;
            },
            TagEnd::BlockQuote(_) => {
                self.flush_paragraph();
                self.in_blockquote = false;
            },
            TagEnd::Table => {
                if let Some(table) = self.table.take() {
                    self.root.push(render_table(table).into_any_element());
                }
            },
            TagEnd::TableHead => {
                if let Some(t) = &mut self.table {
                    t.in_head = false;
                    t.headers = t.current_row.clone();
                }
            },
            TagEnd::TableRow => {
                if let Some(t) = &mut self.table
                    && !t.in_head
                {
                    t.rows.push(t.current_row.clone());
                }
            },
            TagEnd::TableCell => {
                if let Some(t) = &mut self.table {
                    t.current_row.push(t.current_cell.clone());
                }
            },
            _ => {},
        }
    }

    fn finish(mut self) -> Div {
        self.flush_paragraph();
        let mut container = div().w_full().min_w_0().flex().flex_col();
        for el in self.root {
            container = container.child(el);
        }
        container
    }
}

fn render_table(table: TableState) -> Div {
    let mut container = div()
        .w_full()
        .rounded(px(6.0))
        .border_1()
        .border_color(hsla(0.0, 0.0, 0.2, 1.0))
        .overflow_x_hidden()
        .mb(px(8.0))
        .text_sm();

    // Header row
    if !table.headers.is_empty() {
        let mut row = div()
            .w_full()
            .flex()
            .bg(hsla(0.0, 0.0, 0.12, 1.0))
            .border_b_1()
            .border_color(hsla(0.0, 0.0, 0.2, 1.0));

        for cell in &table.headers {
            row = row.child(
                div()
                    .flex_1()
                    .px(px(10.0))
                    .py(px(6.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::text_primary())
                    .min_w(px(80.0))
                    .child(cell.clone()),
            );
        }
        container = container.child(row);
    }

    // Data rows
    for (i, row_data) in table.rows.iter().enumerate() {
        let bg = if i % 2 == 0 {
            hsla(0.0, 0.0, 0.06, 1.0)
        } else {
            hsla(0.0, 0.0, 0.08, 1.0)
        };

        let mut row = div().w_full().flex().bg(bg);

        // Add border between rows (except last)
        if i < table.rows.len() - 1 {
            row = row.border_b_1().border_color(hsla(0.0, 0.0, 0.15, 1.0));
        }

        for cell in row_data {
            row = row.child(
                div()
                    .flex_1()
                    .px(px(10.0))
                    .py(px(5.0))
                    .text_color(theme::text_primary())
                    .min_w(px(80.0))
                    .child(cell.clone()),
            );
        }
        container = container.child(row);
    }

    container
}

#[cfg(test)]
mod tests {
    use super::{MarkdownRenderer, Tag, TagEnd};

    #[test]
    fn list_marker_is_separate_from_wrapping_text() {
        let mut renderer = MarkdownRenderer::new();
        renderer.start_tag(Tag::List(None));
        renderer.start_tag(Tag::Item);
        renderer.push_text("Long requirement that must wrap within the document pane");
        assert_eq!(renderer.list_marker.as_deref(), Some("• "));
        assert_eq!(renderer.inline_buf.len(), 1);
        assert!(renderer.inline_buf[0].text.starts_with("Long requirement"));
        assert!(renderer.flush_inline().is_some());
        assert!(renderer.list_marker.is_none());
    }

    #[test]
    fn ordered_markers_advance_and_empty_items_do_not_leak() {
        let mut renderer = MarkdownRenderer::new();
        renderer.start_tag(Tag::List(Some(3)));
        renderer.start_tag(Tag::Item);
        assert_eq!(renderer.list_marker.as_deref(), Some("3. "));
        renderer.end_tag(TagEnd::Item);
        assert!(renderer.list_marker.is_none());
        renderer.start_tag(Tag::Item);
        assert_eq!(renderer.list_marker.as_deref(), Some("4. "));
    }
}
