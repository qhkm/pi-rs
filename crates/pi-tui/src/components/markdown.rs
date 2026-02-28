use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd, HeadingLevel};
use unicode_width::UnicodeWidthStr;
use crate::components::traits::{Component, InputResult};

/// Theme functions for rendering Markdown elements.
/// Each function takes the content string and returns an ANSI-styled string.
pub struct MarkdownTheme {
    pub heading: Box<dyn Fn(&str, u8) -> String + Send>,
    pub link: Box<dyn Fn(&str) -> String + Send>,
    pub code: Box<dyn Fn(&str) -> String + Send>,
    pub code_block_border: Box<dyn Fn(&str) -> String + Send>,
    pub quote: Box<dyn Fn(&str) -> String + Send>,
    pub bold: Box<dyn Fn(&str) -> String + Send>,
    pub italic: Box<dyn Fn(&str) -> String + Send>,
    pub list_bullet: Box<dyn Fn(&str) -> String + Send>,
    pub hr: Box<dyn Fn(&str) -> String + Send>,
}

impl Default for MarkdownTheme {
    fn default() -> Self {
        Self {
            heading: Box::new(|text, level| {
                let prefix = "#".repeat(level as usize);
                format!("\x1b[1;36m{} {}\x1b[0m", prefix, text)
            }),
            link: Box::new(|text| format!("\x1b[4;34m{}\x1b[0m", text)),
            code: Box::new(|text| format!("\x1b[48;5;236m\x1b[97m {text} \x1b[0m")),
            code_block_border: Box::new(|text| format!("\x1b[90m{}\x1b[0m", text)),
            quote: Box::new(|text| format!("\x1b[90m▎\x1b[0m {}", text)),
            bold: Box::new(|text| format!("\x1b[1m{}\x1b[0m", text)),
            italic: Box::new(|text| format!("\x1b[3m{}\x1b[0m", text)),
            list_bullet: Box::new(|text| format!("\x1b[36m•\x1b[0m {}", text)),
            hr: Box::new(|text| format!("\x1b[90m{}\x1b[0m", text)),
        }
    }
}

pub struct Markdown {
    text: String,
    padding_x: u16,
    padding_y: u16,
    theme: MarkdownTheme,
    dirty: bool,
    cached_lines: Vec<String>,
    cached_width: u16,
}

impl Markdown {
    pub fn new(
        text: impl Into<String>,
        padding_x: u16,
        padding_y: u16,
        theme: MarkdownTheme,
    ) -> Self {
        Self {
            text: text.into(),
            padding_x,
            padding_y,
            theme,
            dirty: true,
            cached_lines: Vec::new(),
            cached_width: 0,
        }
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.dirty = true;
        self.cached_lines.clear();
        self.cached_width = 0;
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    fn render_internal(&self, width: u16) -> Vec<String> {
        let inner_width = (width as i32 - 2 * self.padding_x as i32).max(1) as usize;
        let pad = " ".repeat(self.padding_x as usize);

        let options = Options::all();
        let parser = Parser::new_ext(&self.text, options);

        let mut renderer = MarkdownRenderer::new(inner_width, &self.theme);
        renderer.process(parser);

        let mut lines: Vec<String> = Vec::new();

        // Top padding
        for _ in 0..self.padding_y {
            lines.push(String::new());
        }

        for line in &renderer.lines {
            if pad.is_empty() {
                lines.push(line.clone());
            } else {
                lines.push(format!("{}{}", pad, line));
            }
        }

        // Bottom padding
        for _ in 0..self.padding_y {
            lines.push(String::new());
        }

        lines
    }
}

impl Component for Markdown {
    fn render(&self, width: u16) -> Vec<String> {
        if !self.dirty && self.cached_width == width {
            return self.cached_lines.clone();
        }
        self.render_internal(width)
    }

    fn invalidate(&mut self) {
        self.dirty = true;
        self.cached_lines.clear();
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }
}

// ============================================================================
// Internal renderer
// ============================================================================

struct MarkdownRenderer<'t> {
    lines: Vec<String>,
    current_line: String,
    width: usize,
    theme: &'t MarkdownTheme,
    // State stack
    in_heading: Option<u8>,
    in_code_block: bool,
    code_lang: String,
    in_quote: bool,
    in_bold: bool,
    in_italic: bool,
    in_strikethrough: bool,
    in_link: bool,
    link_url: String,
    list_stack: Vec<ListKind>,
    list_item_index: Vec<usize>,
    in_table: bool,
    table_row: Vec<String>,
    table_header: bool,
    table_rows: Vec<Vec<String>>,
    table_alignments: Vec<pulldown_cmark::Alignment>,
}

#[derive(Debug, Clone, Copy)]
enum ListKind {
    Bullet,
    Ordered,
}

impl<'t> MarkdownRenderer<'t> {
    fn new(width: usize, theme: &'t MarkdownTheme) -> Self {
        Self {
            lines: Vec::new(),
            current_line: String::new(),
            width,
            theme,
            in_heading: None,
            in_code_block: false,
            code_lang: String::new(),
            in_quote: false,
            in_bold: false,
            in_italic: false,
            in_strikethrough: false,
            in_link: false,
            link_url: String::new(),
            list_stack: Vec::new(),
            list_item_index: Vec::new(),
            in_table: false,
            table_row: Vec::new(),
            table_header: false,
            table_rows: Vec::new(),
            table_alignments: Vec::new(),
        }
    }

    fn push_line(&mut self) {
        let line = std::mem::take(&mut self.current_line);
        self.lines.push(line);
    }

    fn push_blank(&mut self) {
        self.lines.push(String::new());
    }

    fn flush_table(&mut self) {
        if self.table_rows.is_empty() { return; }

        // Calculate column widths
        let num_cols = self.table_rows.iter().map(|r| r.len()).max().unwrap_or(0);
        let mut col_widths = vec![0usize; num_cols];
        for row in &self.table_rows {
            for (i, cell) in row.iter().enumerate() {
                let w = strip_ansi_width(cell);
                if w > col_widths[i] {
                    col_widths[i] = w;
                }
            }
        }

        let render_row = |row: &Vec<String>, widths: &[usize]| -> String {
            let mut s = String::from("│ ");
            for (i, cell) in row.iter().enumerate() {
                let cw = strip_ansi_width(cell);
                let pad = widths.get(i).copied().unwrap_or(0).saturating_sub(cw);
                s.push_str(cell);
                s.push_str(&" ".repeat(pad));
                s.push_str(" │ ");
            }
            s
        };

        let separator = |widths: &[usize], kind: char| -> String {
            let mut s = String::new();
            s.push('├');
            for (i, &w) in widths.iter().enumerate() {
                s.push_str(&"─".repeat(w + 2));
                if i + 1 < widths.len() { s.push('┼'); } else { s.push('┤'); }
            }
            s
        };

        for (i, row) in self.table_rows.clone().iter().enumerate() {
            self.lines.push(render_row(row, &col_widths));
            if i == 0 {
                self.lines.push(separator(&col_widths, '─'));
            }
        }
        self.push_blank();
        self.table_rows.clear();
    }

    fn apply_inline_style(&self, text: &str) -> String {
        let mut s = text.to_string();
        if self.in_bold {
            s = (self.theme.bold)(&s);
        }
        if self.in_italic {
            s = (self.theme.italic)(&s);
        }
        if self.in_strikethrough {
            s = format!("\x1b[9m{}\x1b[0m", s);
        }
        if self.in_link {
            s = (self.theme.link)(&s);
        }
        s
    }

    fn process<'a>(&mut self, parser: impl Iterator<Item = Event<'a>>) {
        for event in parser {
            match event {
                Event::Start(tag) => self.handle_start(tag),
                Event::End(tag) => self.handle_end(tag),
                Event::Text(text) => self.handle_text(&text),
                Event::Code(code) => {
                    let styled = (self.theme.code)(&code);
                    self.current_line.push_str(&styled);
                }
                Event::Html(_) | Event::InlineHtml(_) => {
                    // Skip raw HTML
                }
                Event::SoftBreak => {
                    self.current_line.push(' ');
                }
                Event::HardBreak => {
                    self.push_line();
                }
                Event::Rule => {
                    let hr = "─".repeat(self.width);
                    self.lines.push((self.theme.hr)(&hr));
                    self.push_blank();
                }
                Event::TaskListMarker(checked) => {
                    let marker = if checked { "[x] " } else { "[ ] " };
                    self.current_line.push_str(marker);
                }
                _ => {}
            }
        }

        if !self.current_line.is_empty() {
            self.push_line();
        }

        if self.in_table {
            self.flush_table();
        }
    }

    fn handle_start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Heading { level, .. } => {
                let n = heading_level(level);
                self.in_heading = Some(n);
                self.push_blank();
            }
            Tag::Paragraph => {}
            Tag::BlockQuote(_) => {
                self.in_quote = true;
            }
            Tag::CodeBlock(kind) => {
                self.in_code_block = true;
                self.code_lang = match kind {
                    pulldown_cmark::CodeBlockKind::Fenced(lang) => lang.to_string(),
                    pulldown_cmark::CodeBlockKind::Indented => String::new(),
                };
                let border = "─".repeat(self.width.min(40));
                self.lines.push((self.theme.code_block_border)(&border));
            }
            Tag::List(start) => {
                let kind = if start.is_some() { ListKind::Ordered } else { ListKind::Bullet };
                self.list_stack.push(kind);
                self.list_item_index.push(start.unwrap_or(1) as usize);
            }
            Tag::Item => {
                let depth = self.list_stack.len();
                let indent = "  ".repeat(depth.saturating_sub(1));
                let bullet = match self.list_stack.last() {
                    Some(ListKind::Bullet) => (self.theme.list_bullet)(""),
                    Some(ListKind::Ordered) => {
                        let idx = self.list_item_index.last().copied().unwrap_or(1);
                        format!("\x1b[36m{}.\x1b[0m", idx)
                    }
                    None => "•".to_string(),
                };
                self.current_line = format!("{}{} ", indent, bullet);
            }
            Tag::Emphasis => { self.in_italic = true; }
            Tag::Strong => { self.in_bold = true; }
            Tag::Strikethrough => { self.in_strikethrough = true; }
            Tag::Link { dest_url, .. } => {
                self.in_link = true;
                self.link_url = dest_url.to_string();
            }
            Tag::Table(alignments) => {
                self.in_table = true;
                self.table_alignments = alignments;
                self.table_rows.clear();
            }
            Tag::TableHead => {
                self.table_header = true;
                self.table_row.clear();
            }
            Tag::TableRow => {
                self.table_row.clear();
            }
            Tag::TableCell => {
                self.current_line.clear();
            }
            Tag::Image { dest_url, title, .. } => {
                let alt = if title.is_empty() { dest_url.to_string() } else { title.to_string() };
                let styled = (self.theme.link)(&format!("[img: {}]", alt));
                self.current_line.push_str(&styled);
            }
            Tag::HtmlBlock => {}
            Tag::FootnoteDefinition(_) => {}
            Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition => {}
            Tag::MetadataBlock(_) => {}
            _ => {}
        }
    }

    fn handle_end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Heading(_) => {
                if let Some(level) = self.in_heading.take() {
                    let text = std::mem::take(&mut self.current_line);
                    let styled = (self.theme.heading)(&text, level);
                    self.lines.push(styled);
                    self.push_blank();
                }
            }
            TagEnd::Paragraph => {
                if !self.current_line.is_empty() {
                    // Word-wrap paragraph
                    let text = std::mem::take(&mut self.current_line);
                    let wrapped = wrap_ansi(&text, self.width);
                    if self.in_quote {
                        for line in wrapped {
                            self.lines.push((self.theme.quote)(&line));
                        }
                    } else {
                        self.lines.extend(wrapped);
                    }
                }
                self.push_blank();
            }
            TagEnd::BlockQuote(_) => {
                if !self.current_line.is_empty() {
                    let text = std::mem::take(&mut self.current_line);
                    self.lines.push((self.theme.quote)(&text));
                }
                self.in_quote = false;
            }
            TagEnd::CodeBlock => {
                // Code block content was pushed line-by-line in handle_text
                let border = "─".repeat(self.width.min(40));
                self.lines.push((self.theme.code_block_border)(&border));
                self.push_blank();
                self.in_code_block = false;
                self.code_lang.clear();
            }
            TagEnd::List(_) => {
                self.list_stack.pop();
                self.list_item_index.pop();
                self.push_blank();
            }
            TagEnd::Item => {
                if !self.current_line.is_empty() {
                    let text = std::mem::take(&mut self.current_line);
                    // Word-wrap item text preserving bullet prefix
                    self.lines.push(text);
                }
                // Advance ordered list index
                if let (Some(ListKind::Ordered), Some(idx)) =
                    (self.list_stack.last(), self.list_item_index.last_mut())
                {
                    *idx += 1;
                }
            }
            TagEnd::Emphasis => { self.in_italic = false; }
            TagEnd::Strong => { self.in_bold = false; }
            TagEnd::Strikethrough => { self.in_strikethrough = false; }
            TagEnd::Link => {
                self.in_link = false;
                self.link_url.clear();
            }
            TagEnd::Table => {
                self.flush_table();
                self.in_table = false;
            }
            TagEnd::TableHead => {
                let row = std::mem::take(&mut self.table_row);
                self.table_rows.push(row);
            }
            TagEnd::TableRow => {
                let row = std::mem::take(&mut self.table_row);
                self.table_rows.push(row);
            }
            TagEnd::TableCell => {
                let cell = std::mem::take(&mut self.current_line);
                self.table_row.push(cell);
            }
            TagEnd::Image => {}
            TagEnd::HtmlBlock => {}
            TagEnd::FootnoteDefinition => {}
            TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition => {}
            TagEnd::MetadataBlock(_) => {}
            _ => {}
            TagEnd::Paragraph => {
                // Already handled above — this branch is unreachable due to the
                // pattern matching order, but we include it for exhaustiveness.
            }
        }
    }

    fn handle_text(&mut self, text: &str) {
        if self.in_code_block {
            // Each line of code block text goes on its own output line
            for line in text.split('\n') {
                self.lines.push(format!("  {}", line));
            }
            // Remove trailing empty pushed from final \n in code block
            if self.lines.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
                self.lines.pop();
            }
            return;
        }

        let styled = self.apply_inline_style(text);
        self.current_line.push_str(&styled);
    }
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// Return the display width of a string, stripping ANSI escape sequences.
fn strip_ansi_width(s: &str) -> usize {
    let stripped = strip_ansi(s);
    UnicodeWidthStr::width(stripped.as_str())
}

/// Strip ANSI escape sequences from a string.
fn strip_ansi(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            // Skip until end of escape sequence
            match chars.peek() {
                Some(&'[') => {
                    chars.next(); // consume '['
                    // Skip CSI sequence: params end at a letter
                    loop {
                        match chars.next() {
                            Some(c) if c.is_ascii_alphabetic() => break,
                            None => break,
                            _ => {}
                        }
                    }
                }
                Some(&']') => {
                    chars.next(); // consume ']'
                    loop {
                        match chars.next() {
                            Some('\x07') | None => break,
                            Some('\x1b') => { chars.next(); break; }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        } else {
            result.push(ch);
        }
    }
    result
}

/// Word-wrap a string (which may contain ANSI codes) to fit within `max_width` columns.
/// This is a best-effort implementation that splits on spaces.
fn wrap_ansi(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 { return vec![String::new()]; }

    let mut lines = Vec::new();
    let mut current_visible_width = 0usize;
    let mut current_line = String::new();

    // Split by words (preserving ANSI codes inline)
    // Simple approach: split the *visible* text and rebuild
    let visible = strip_ansi(text);
    let words: Vec<&str> = visible.split_whitespace().collect();
    if words.is_empty() {
        return vec![String::new()];
    }

    // Re-wrap the visible text, then re-apply the original styled string
    // For simplicity, we wrap the stripped text and use it directly.
    // Full ANSI-preserving wrap is a complex problem; this covers the common case.
    let mut current = String::new();
    let mut width = 0usize;

    for word in &words {
        let ww = UnicodeWidthStr::width(*word);
        if width == 0 {
            current.push_str(word);
            width = ww;
        } else if width + 1 + ww <= max_width {
            current.push(' ');
            current.push_str(word);
            width += 1 + ww;
        } else {
            lines.push(current.clone());
            current = word.to_string();
            width = ww;
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }

    if lines.is_empty() { vec![String::new()] } else { lines }
}
