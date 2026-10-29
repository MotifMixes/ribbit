//! Help › User Guide: Ribbit's guide, built into the program (as in
//! Converge). Its pages are `docs/manual/NN-id.md` (in name order) with
//! pictures in `docs/manual/images`, both built in by `build.rs`; they read
//! as they are on GitHub too.
//!
//! The pages use a small part of Markdown:
//! - `# Title` (the first line), `## Section`, `### Part`;
//! - paragraphs, `- ` bullets (indented two spaces: one level in) and
//!   `1. ` steps;
//! - tables (`| a | b |` rows, the first a heading, then a `|---|` row);
//! - `**bold**`, `` `Ctrl+S` `` (keys and labels), `[text](#page)` or
//!   `[text](#page/section)` links to another page, `[text](https://…)`
//!   links that open in the browser;
//! - `![caption](images/name.png)` pictures;
//! - `> ` tips.
//!
//! The window: the pages down the left with a search box above them, the
//! page on the right; Back goes where a link came from. On Android it fills
//! the screen.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use egui::{CornerRadius, RichText, Stroke, Ui, vec2};

/// Ribbit's colours for the guide.
mod theme {
    use egui::Color32;
    pub fn text() -> Color32 {
        Color32::from_gray(225)
    }
    pub fn text_dim() -> Color32 {
        Color32::from_gray(150)
    }
    /// The frog's green.
    pub fn accent() -> Color32 {
        Color32::from_rgb(46, 170, 82)
    }
    pub fn separator() -> Color32 {
        Color32::from_white_alpha(28)
    }
    pub fn button_face() -> Color32 {
        Color32::from_gray(45)
    }
    pub fn button_edge() -> Color32 {
        Color32::from_gray(75)
    }
    pub fn button_hover() -> Color32 {
        Color32::from_white_alpha(16)
    }
    pub fn tip_fill() -> Color32 {
        Color32::from_rgb(24, 40, 30)
    }
    pub fn tip_edge() -> Color32 {
        Color32::from_rgb(40, 96, 58)
    }
    pub fn font(size: f32) -> egui::FontId {
        egui::FontId::proportional(size)
    }
}

mod files {
    include!(concat!(env!("OUT_DIR"), "/manual_files.rs"));
}

/// What the manual window shows (kept while it is closed).
#[derive(Debug, Default)]
pub struct ManualState {
    pub open: bool,
    /// The page shown (index into `pages()`).
    pub page: usize,
    /// Pages to go back to.
    history: Vec<usize>,
    pub search: String,
    /// A section to scroll to on the next frame.
    scroll_to: Option<String>,
}

impl ManualState {
    /// Opens the manual at page `id` (its file name without the number:
    /// "flexible-loops"), or where it was.
    pub fn open_at(&mut self, id: Option<&str>) {
        self.open = true;
        if let Some(i) = id.and_then(page_index) {
            if i != self.page {
                self.history.push(self.page);
            }
            self.page = i;
            self.scroll_to = None;
        }
    }

    fn go(&mut self, target: &str) {
        let (id, section) = target.trim_start_matches('#').split_once('/').map_or((target.trim_start_matches('#'), None), |(p, s)| (p, Some(s)));
        if let Some(i) = page_index(id) {
            if i != self.page {
                self.history.push(self.page);
            }
            self.page = i;
            self.scroll_to = section.map(str::to_string).or(Some(String::new()));
        }
    }
}

/// Inline text.
#[derive(Debug, Clone, PartialEq)]
enum Inline {
    Text(String),
    Bold(String),
    /// A key or a label: `Ctrl+S`, `Add`.
    Key(String),
    Link { text: String, target: String },
}

#[derive(Debug, Clone, PartialEq)]
enum Block {
    Section(String),
    Part(String),
    Paragraph(Vec<Inline>),
    /// Items, each with how far in it is (0 or 1).
    Bullets(Vec<(u8, Vec<Inline>)>),
    Steps(Vec<(u8, Vec<Inline>)>),
    /// Rows of cells; the first row is the heading.
    Table(Vec<Vec<Vec<Inline>>>),
    Picture { path: String, caption: String },
    Tip(Vec<Inline>),
}

#[derive(Debug)]
pub struct Page {
    /// "flexible-loops" (the file's name without its number).
    pub id: String,
    pub title: String,
    blocks: Vec<Block>,
    /// All its words, lowercase, by section (for searching).
    sections: Vec<(String, String)>,
}

/// Every page, in order.
pub fn pages() -> &'static [Page] {
    static PAGES: OnceLock<Vec<Page>> = OnceLock::new();
    PAGES.get_or_init(|| files::PAGES.iter().map(|(stem, text)| parse(stem, text)).collect())
}

fn page_index(id: &str) -> Option<usize> {
    pages().iter().position(|p| p.id == id)
}

/// A section's anchor: its title in lowercase, words joined by dashes.
fn slug(title: &str) -> String {
    title
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn parse(stem: &str, text: &str) -> Page {
    let id = stem.split_once('-').map_or(stem, |(n, rest)| if n.chars().all(|c| c.is_ascii_digit()) { rest } else { stem });
    let mut title = id.to_string();
    let mut blocks = Vec::new();
    let mut paragraph: Vec<&str> = Vec::new();
    let mut list: Option<(bool, Vec<(u8, Vec<Inline>)>)> = None;
    let mut tip: Vec<&str> = Vec::new();
    let mut table: Vec<Vec<Vec<Inline>>> = Vec::new();
    let flush = |blocks: &mut Vec<Block>,
                 paragraph: &mut Vec<&str>,
                 list: &mut Option<(bool, Vec<(u8, Vec<Inline>)>)>,
                 tip: &mut Vec<&str>,
                 table: &mut Vec<Vec<Vec<Inline>>>| {
        if !table.is_empty() {
            blocks.push(Block::Table(std::mem::take(table)));
        }
        if !paragraph.is_empty() {
            blocks.push(Block::Paragraph(inline(&paragraph.join(" "))));
            paragraph.clear();
        }
        if let Some((steps, items)) = list.take() {
            blocks.push(if steps { Block::Steps(items) } else { Block::Bullets(items) });
        }
        if !tip.is_empty() {
            blocks.push(Block::Tip(inline(&tip.join(" "))));
            tip.clear();
        }
    };
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() {
            flush(&mut blocks, &mut paragraph, &mut list, &mut tip, &mut table);
            continue;
        }
        if t.starts_with('|') {
            if !paragraph.is_empty() || list.is_some() || !tip.is_empty() {
                flush(&mut blocks, &mut paragraph, &mut list, &mut tip, &mut table);
            }
            let cells: Vec<&str> = t.trim_matches('|').split('|').map(str::trim).collect();
            // The heading's underline.
            if !cells.iter().all(|c| !c.is_empty() && c.chars().all(|ch| matches!(ch, '-' | ':'))) {
                table.push(cells.into_iter().map(inline).collect());
            }
            continue;
        }
        if !table.is_empty() {
            flush(&mut blocks, &mut paragraph, &mut list, &mut tip, &mut table);
        }
        if let Some(h) = t.strip_prefix("# ") {
            flush(&mut blocks, &mut paragraph, &mut list, &mut tip, &mut table);
            title = h.trim().to_string();
        } else if let Some(h) = t.strip_prefix("## ") {
            flush(&mut blocks, &mut paragraph, &mut list, &mut tip, &mut table);
            blocks.push(Block::Section(h.trim().to_string()));
        } else if let Some(h) = t.strip_prefix("### ") {
            flush(&mut blocks, &mut paragraph, &mut list, &mut tip, &mut table);
            blocks.push(Block::Part(h.trim().to_string()));
        } else if let Some(rest) = t.strip_prefix("![") {
            flush(&mut blocks, &mut paragraph, &mut list, &mut tip, &mut table);
            if let Some((caption, path)) = rest.split_once("](") {
                blocks.push(Block::Picture { path: path.trim_end_matches(')').to_string(), caption: caption.to_string() });
            }
        } else if let Some(rest) = t.strip_prefix('>') {
            if !paragraph.is_empty() || list.is_some() {
                flush(&mut blocks, &mut paragraph, &mut list, &mut tip, &mut table);
            }
            tip.push(rest.trim());
        } else if let Some(item) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
            if !paragraph.is_empty() || list.as_ref().is_some_and(|(s, _)| *s) {
                flush(&mut blocks, &mut paragraph, &mut list, &mut tip, &mut table);
            }
            let depth = u8::from(line.starts_with("  ") && list.is_some());
            list.get_or_insert((false, Vec::new())).1.push((depth, inline(item)));
        } else if let Some(item) = numbered(t) {
            if !paragraph.is_empty() || list.as_ref().is_some_and(|(s, _)| !*s) {
                flush(&mut blocks, &mut paragraph, &mut list, &mut tip, &mut table);
            }
            list.get_or_insert((true, Vec::new())).1.push((0, inline(item)));
        } else if line.starts_with("  ") && list.is_some() {
            // A list item's next line.
            if let Some((_, last)) = list.as_mut().and_then(|(_, items)| items.last_mut()) {
                last.push(Inline::Text(" ".into()));
                last.extend(inline(t));
            }
        } else if !tip.is_empty() {
            tip.push(t);
        } else {
            if list.is_some() {
                flush(&mut blocks, &mut paragraph, &mut list, &mut tip, &mut table);
            }
            paragraph.push(t);
        }
    }
    flush(&mut blocks, &mut paragraph, &mut list, &mut tip, &mut table);
    // The words of each section, for the search.
    let mut sections = vec![(String::new(), title.to_lowercase())];
    for b in &blocks {
        match b {
            Block::Section(h) => sections.push((h.clone(), h.to_lowercase())),
            other => {
                if let Some((_, words)) = sections.last_mut() {
                    words.push(' ');
                    words.push_str(&plain(other).to_lowercase());
                }
            }
        }
    }
    Page { id: id.to_string(), title, blocks, sections }
}

/// "1. Do this" → "Do this".
fn numbered(t: &str) -> Option<&str> {
    let (n, rest) = t.split_once(". ")?;
    (!n.is_empty() && n.len() <= 2 && n.chars().all(|c| c.is_ascii_digit())).then_some(rest)
}

/// A block's words without markup.
fn plain(b: &Block) -> String {
    let text = |items: &[Inline]| {
        items
            .iter()
            .map(|i| match i {
                Inline::Text(t) | Inline::Bold(t) | Inline::Key(t) => t.as_str(),
                Inline::Link { text, .. } => text.as_str(),
            })
            .collect::<String>()
    };
    match b {
        Block::Section(t) | Block::Part(t) => t.clone(),
        Block::Paragraph(i) | Block::Tip(i) => text(i),
        Block::Bullets(items) | Block::Steps(items) => items.iter().map(|(_, i)| text(i)).collect::<Vec<_>>().join(" "),
        Block::Table(rows) => rows.iter().flatten().map(|c| text(c)).collect::<Vec<_>>().join(" "),
        Block::Picture { caption, .. } => caption.clone(),
    }
}

/// Inline markup: **bold**, `keys`, [links](#page).
fn inline(text: &str) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut rest = text;
    let mut plain_text = String::new();
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix("**")
            && let Some((bold, after)) = r.split_once("**")
        {
            push_text(&mut out, &mut plain_text);
            out.push(Inline::Bold(bold.to_string()));
            rest = after;
        } else if let Some(r) = rest.strip_prefix('`')
            && let Some((key, after)) = r.split_once('`')
        {
            push_text(&mut out, &mut plain_text);
            out.push(Inline::Key(key.to_string()));
            rest = after;
        } else if let Some(r) = rest.strip_prefix('[')
            && let Some((label, after)) = r.split_once("](")
            && let Some((target, after)) = after.split_once(')')
        {
            push_text(&mut out, &mut plain_text);
            out.push(Inline::Link { text: label.to_string(), target: target.to_string() });
            rest = after;
        } else {
            let c = rest.chars().next().expect("not empty");
            plain_text.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    push_text(&mut out, &mut plain_text);
    out
}

fn push_text(out: &mut Vec<Inline>, text: &mut String) {
    if !text.is_empty() {
        out.push(Inline::Text(std::mem::take(text)));
    }
}

/// The body text's size.
const BODY: f32 = 14.5;
/// The page's widest text column.
const COLUMN: f32 = 720.0;

/// Help › User Guide.
pub fn window(ctx: &egui::Context, state: &mut ManualState) {
    if !state.open {
        return;
    }
    let phone = cfg!(target_os = "android");
    let screen = ctx.content_rect();
    let mut open = true;
    let mut window = egui::Window::new("User Guide").id(egui::Id::new("user_guide")).open(&mut open).collapsible(false);
    window = if phone {
        // The whole screen, as an app's page on a phone.
        window.fixed_pos(screen.min + vec2(6.0, 6.0)).fixed_size(screen.size() - vec2(30.0, 50.0)).resizable(false)
    } else {
        let size = vec2(1040.0, 760.0).min(screen.size() - vec2(40.0, 60.0));
        window.default_size(size).pivot(egui::Align2::CENTER_CENTER).default_pos(screen.center()).resizable(true)
    };
    let sidebar_width = if phone { 180.0 } else { 230.0 };
    window.show(ctx, |ui| {
        let height = ui.available_height();
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(vec2(sidebar_width, height), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_width(sidebar_width);
                sidebar(ui, state);
            });
            ui.separator();
            ui.allocate_ui_with_layout(vec2(ui.available_width(), height), egui::Layout::top_down(egui::Align::Min), |ui| {
                page(ui, state);
            });
        });
    });
    if !open {
        state.open = false;
    }
}

fn sidebar(ui: &mut Ui, state: &mut ManualState) {
    ui.add(
        egui::TextEdit::singleline(&mut state.search)
            .hint_text("Search the guide")
            .desired_width(f32::INFINITY),
    )
    .on_hover_text("Search every page: the sections that mention all the words you type");
    ui.add_space(6.0);
    let query: Vec<String> = state.search.split_whitespace().map(str::to_lowercase).collect();
    egui::ScrollArea::vertical().id_salt("manual_pages").auto_shrink([false, false]).show(ui, |ui| {
        if query.is_empty() {
            for (i, p) in pages().iter().enumerate() {
                if entry(ui, &p.title, 13.0, i == state.page).clicked() && i != state.page {
                    state.history.push(state.page);
                    state.page = i;
                    state.scroll_to = Some(String::new());
                }
            }
            return;
        }
        let mut found = 0;
        for (i, p) in pages().iter().enumerate() {
            for (section, words) in &p.sections {
                if !query.iter().all(|q| words.contains(q.as_str())) {
                    continue;
                }
                found += 1;
                let text = if section.is_empty() { p.title.clone() } else { format!("{} › {section}", p.title) };
                if entry(ui, &text, 12.5, false).clicked() {
                    if i != state.page {
                        state.history.push(state.page);
                    }
                    state.page = i;
                    state.scroll_to = Some(slug(section));
                }
            }
        }
        if found == 0 {
            ui.label(RichText::new("Nothing found").color(theme::text_dim()));
        }
    });
}

/// A line of the sidebar: the whole width, wrapped text, a quiet
/// highlight under the pointer. Drawn at a fixed size, so nothing moves
/// as the pointer passes over it.
fn entry(ui: &mut Ui, text: &str, size: f32, selected: bool) -> egui::Response {
    let pad = vec2(8.0, 3.0);
    let width = ui.available_width();
    let color = if selected { egui::Color32::WHITE } else { theme::text() };
    let galley = ui.painter().layout(text.to_string(), theme::font(size), color, width - 2.0 * pad.x);
    let (rect, response) = ui.allocate_exact_size(vec2(width, galley.size().y + 2.0 * pad.y), egui::Sense::click());
    let fill = if selected {
        Some(theme::accent())
    } else if response.hovered() {
        Some(theme::button_hover())
    } else {
        None
    };
    if let Some(fill) = fill {
        ui.painter().rect_filled(rect, CornerRadius::same(5), fill);
    }
    ui.painter().galley(rect.min + pad, galley, color);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn page(ui: &mut Ui, state: &mut ManualState) {
    let all = pages();
    let Some(page) = all.get(state.page) else {
        ui.label("The guide has no pages.");
        return;
    };
    ui.horizontal(|ui| {
        if ui.add_enabled(!state.history.is_empty(), egui::Button::new("‹ Back")).on_hover_text("Back to the page you came from").clicked()
            && let Some(back) = state.history.pop()
        {
            state.page = back;
            state.scroll_to = Some(String::new());
        }
        ui.label(RichText::new(format!("{} of {}", state.page + 1, all.len())).color(theme::text_dim()).size(11.5));
    });
    ui.add_space(4.0);
    let scroll_to = state.scroll_to.take();
    let mut go: Option<String> = None;
    let mut area = egui::ScrollArea::vertical().id_salt(("manual_page", state.page)).auto_shrink([false, false]);
    if scroll_to.as_deref() == Some("") {
        area = area.vertical_scroll_offset(0.0);
    }
    area.show(ui, |ui| {
        let width = ui.available_width().min(COLUMN);
        ui.set_max_width(width);
        ui.add_space(6.0);
        ui.label(RichText::new(&page.title).size(26.0).strong().color(theme::text()));
        ui.add_space(10.0);
        for block in &page.blocks {
            match block {
                Block::Section(h) => {
                    ui.add_space(14.0);
                    let r = ui.label(RichText::new(h).size(18.0).strong().color(theme::text()));
                    if scroll_to.as_deref().is_some_and(|s| !s.is_empty() && s == slug(h)) {
                        r.scroll_to_me(Some(egui::Align::TOP));
                    }
                    let y = r.rect.bottom() + 3.0;
                    ui.painter().hline(r.rect.left()..=r.rect.left() + width, y, Stroke::new(1.0, theme::separator()));
                    ui.add_space(8.0);
                }
                Block::Part(h) => {
                    ui.add_space(8.0);
                    ui.label(RichText::new(h).size(15.0).strong().color(theme::text()));
                    ui.add_space(2.0);
                }
                Block::Paragraph(items) => {
                    words(ui, items, &mut go);
                    ui.add_space(6.0);
                }
                Block::Bullets(items) | Block::Steps(items) => {
                    let steps = matches!(block, Block::Steps(_));
                    let mut n = 0;
                    for (depth, item) in items {
                        if *depth == 0 {
                            n += 1;
                        }
                        ui.horizontal_top(|ui| {
                            ui.add_space(22.0 * f32::from(*depth));
                            let mark = match (steps, *depth) {
                                (true, 0) => format!("{n}."),
                                (_, 0) => "•".to_string(),
                                _ => "◦".to_string(),
                            };
                            ui.add_sized(vec2(22.0, 18.0), egui::Label::new(RichText::new(mark).size(BODY).color(theme::text_dim())));
                            ui.vertical(|ui| words(ui, item, &mut go));
                        });
                        ui.add_space(2.0);
                    }
                    ui.add_space(6.0);
                }
                Block::Table(rows) => {
                    egui::Frame::new()
                        .stroke(Stroke::new(1.0, theme::separator()))
                        .corner_radius(CornerRadius::same(6))
                        .inner_margin(egui::Margin::symmetric(10, 6))
                        .show(ui, |ui| {
                            // The columns share the page's width.
                            let columns = rows.first().map_or(1, Vec::len).max(1) as f32;
                            let gap = 18.0;
                            let column = ((width - 22.0 - gap * (columns - 1.0)) / columns).max(60.0);
                            egui::Grid::new(("manual_table", page.id.as_str(), rows.len(), plain(block).len()))
                                .striped(true)
                                .spacing(vec2(gap, 6.0))
                                .min_col_width(column)
                                .max_col_width(column)
                                .show(ui, |ui| {
                                    for (r, row) in rows.iter().enumerate() {
                                        for cell in row {
                                            ui.vertical(|ui| {
                                                ui.set_width(column);
                                                if r == 0 {
                                                    let text: String = plain(&Block::Paragraph(cell.clone()));
                                                    ui.label(RichText::new(text).size(BODY).strong().color(theme::text()));
                                                } else {
                                                    words(ui, cell, &mut go);
                                                }
                                            });
                                        }
                                        ui.end_row();
                                    }
                                });
                        });
                    ui.add_space(8.0);
                }
                Block::Tip(items) => {
                    egui::Frame::new()
                        .fill(theme::tip_fill())
                        .stroke(Stroke::new(1.0, theme::tip_edge()))
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(egui::Margin::symmetric(12, 8))
                        .show(ui, |ui| {
                            ui.set_width(width - 24.0);
                            words(ui, items, &mut go);
                        });
                    ui.add_space(8.0);
                }
                Block::Picture { path, caption } => {
                    picture(ui, path, caption, width);
                    ui.add_space(10.0);
                }
            }
        }
        // The next and previous pages.
        ui.add_space(18.0);
        ui.separator();
        ui.horizontal(|ui| {
            if state.page > 0 && ui.button(format!("‹ {}", all[state.page - 1].title)).clicked() {
                go = Some(format!("#{}", all[state.page - 1].id));
            }
            if let Some(next) = all.get(state.page + 1) {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(format!("{} ›", next.title)).clicked() {
                        go = Some(format!("#{}", next.id));
                    }
                });
            }
        });
        ui.add_space(12.0);
    });
    if let Some(target) = go {
        state.go(&target);
    }
}

/// A paragraph's words, wrapped; a link clicked sets `go`.
fn words(ui: &mut Ui, items: &[Inline], go: &mut Option<String>) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for item in items {
            match item {
                Inline::Text(t) => {
                    for w in t.split_inclusive(' ') {
                        ui.label(RichText::new(w).size(BODY).color(theme::text()));
                    }
                }
                Inline::Bold(t) => {
                    for w in t.split_inclusive(' ') {
                        ui.label(RichText::new(w).size(BODY).strong().color(theme::text()));
                    }
                }
                Inline::Key(t) => {
                    ui.add_space(1.0);
                    egui::Frame::new()
                        .fill(theme::button_face())
                        .stroke(Stroke::new(1.0, theme::button_edge()))
                        .corner_radius(CornerRadius::same(4))
                        .inner_margin(egui::Margin::symmetric(5, 0))
                        .show(ui, |ui| {
                            ui.add(egui::Label::new(RichText::new(t).size(BODY - 1.5).color(theme::text())).extend());
                        });
                    ui.add_space(1.0);
                }
                Inline::Link { text, target } => {
                    let link = egui::Link::new(RichText::new(text).size(BODY).color(theme::accent()).underline());
                    let web = target.starts_with("http");
                    let r = ui.add(link).on_hover_text(if web { target.as_str() } else { "Go to that page" });
                    if r.clicked() {
                        if web {
                            ui.ctx().open_url(egui::OpenUrl::new_tab(target));
                        } else {
                            *go = Some(target.clone());
                        }
                    }
                }
            }
        }
    });
}

/// A picture, as wide as the page allows (never larger than it was
/// taken), with its caption. A click shows it at full size.
fn picture(ui: &mut Ui, path: &str, caption: &str, width: f32) {
    let Some(texture) = texture(ui.ctx(), path) else {
        ui.label(RichText::new(format!("(Picture missing: {path})")).color(theme::text_dim()));
        return;
    };
    let size = texture.size_vec2();
    let shown = if size.x > width { size * (width / size.x) } else { size };
    let r = ui.add(egui::Image::new((texture.id(), shown)).corner_radius(6.0).sense(egui::Sense::click()));
    ui.painter().rect_stroke(r.rect, CornerRadius::same(6), Stroke::new(1.0, theme::separator()), egui::StrokeKind::Outside);
    let zoom_id = egui::Id::new(("manual_zoom", path));
    if r.on_hover_cursor(egui::CursorIcon::ZoomIn).clicked() {
        ui.ctx().data_mut(|d| d.insert_temp(zoom_id, true));
    }
    if ui.ctx().data(|d| d.get_temp::<bool>(zoom_id)).unwrap_or(false) {
        let mut open = true;
        egui::Window::new(caption)
            .id(zoom_id.with("window"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_pos(ui.ctx().content_rect().left_top() + vec2(10.0, 10.0))
            .show(ui.ctx(), |ui| {
                let room = ui.ctx().content_rect().size() - vec2(40.0, 60.0);
                let k = (room.x / size.x).min(room.y / size.y).min(1.0);
                if ui.add(egui::Image::new((texture.id(), size * k)).sense(egui::Sense::click())).clicked() {
                    ui.ctx().data_mut(|d| d.insert_temp(zoom_id, false));
                }
            });
        if !open {
            ui.ctx().data_mut(|d| d.insert_temp(zoom_id, false));
        }
    }
    if !caption.is_empty() {
        ui.label(RichText::new(caption).size(12.0).italics().color(theme::text_dim()));
    }
}

/// Picture `path`'s texture, decoded once.
fn texture(ctx: &egui::Context, path: &str) -> Option<egui::TextureHandle> {
    type Cache = Arc<std::sync::Mutex<HashMap<String, Option<egui::TextureHandle>>>>;
    let id = egui::Id::new("manual_textures");
    let cache: Cache = ctx.data_mut(|d| d.get_temp_mut_or_default::<Cache>(id).clone());
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    cache
        .entry(path.to_string())
        .or_insert_with(|| {
            let bytes = files::IMAGES.iter().find(|(p, _)| *p == path)?.1;
            let image = decode_png(bytes)?;
            Some(ctx.load_texture(format!("manual:{path}"), image, egui::TextureOptions::LINEAR))
        })
        .clone()
}

pub(crate) fn decode_png(bytes: &[u8]) -> Option<egui::ColorImage> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width as usize, info.height as usize);
    let px = &buf[..info.buffer_size()];
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => px.to_vec(),
        png::ColorType::Rgb => px.chunks_exact(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => px.chunks_exact(2).flat_map(|c| [c[0], c[0], c[0], c[1]]).collect(),
        png::ColorType::Grayscale => px.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return None,
    };
    Some(egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markup_becomes_blocks_and_inline_parts() {
        let page = parse(
            "07-example",
            "# Example\n\nSome **bold** text with `Ctrl+S` and [a link](#other).\n\n## First part\n\n- one\n- two\n  more\n\n1. Do this\n2. Then that\n\n> Tip: a tip\n> that goes on.\n\n![The window](images/x.png)\n",
        );
        assert_eq!(page.id, "example");
        assert_eq!(page.title, "Example");
        assert_eq!(
            page.blocks[0],
            Block::Paragraph(vec![
                Inline::Text("Some ".into()),
                Inline::Bold("bold".into()),
                Inline::Text(" text with ".into()),
                Inline::Key("Ctrl+S".into()),
                Inline::Text(" and ".into()),
                Inline::Link { text: "a link".into(), target: "#other".into() },
                Inline::Text(".".into()),
            ])
        );
        assert_eq!(page.blocks[1], Block::Section("First part".into()));
        assert!(matches!(&page.blocks[2], Block::Bullets(items) if items.len() == 2 && items[1].1.len() == 3));
        assert!(matches!(&page.blocks[3], Block::Steps(items) if items.len() == 2));
        assert!(matches!(&page.blocks[4], Block::Tip(_)));
        assert_eq!(page.blocks[5], Block::Picture { path: "images/x.png".into(), caption: "The window".into() });
        assert_eq!(slug("Making and placing them"), "making-and-placing-them");
    }

    #[test]
    fn nested_bullets_and_tables_are_read() {
        let page = parse("01-x", "# X

- one
  - inner
- two

| A | B |
|---|---|
| `k` | v |
");
        assert_eq!(page.blocks.len(), 2);
        assert!(matches!(&page.blocks[0], Block::Bullets(items) if items.iter().map(|i| i.0).collect::<Vec<_>>() == vec![0, 1, 0]));
        assert!(matches!(&page.blocks[1], Block::Table(rows) if rows.len() == 2 && rows[1][0] == vec![Inline::Key("k".into())]));
    }

    #[test]
    fn every_page_has_a_title_every_link_a_page_and_every_picture_a_file() {
        assert!(!pages().is_empty(), "the guide has pages");
        assert!(page_index("getting-started").is_some(), "Getting started has a page");
        for p in pages() {
            assert!(!p.title.is_empty());
            for b in &p.blocks {
                let links: Vec<&Inline> = match b {
                    Block::Paragraph(i) | Block::Tip(i) => i.iter().collect(),
                    Block::Bullets(items) | Block::Steps(items) => items.iter().flat_map(|(_, i)| i).collect(),
                    Block::Table(rows) => rows.iter().flatten().flatten().collect(),
                    Block::Picture { path, .. } => {
                        assert!(files::IMAGES.iter().any(|(f, _)| f == path), "{}: no picture {path}", p.id);
                        assert!(decode_png(files::IMAGES.iter().find(|(f, _)| f == path).unwrap().1).is_some(), "{path} does not decode");
                        Vec::new()
                    }
                    _ => Vec::new(),
                };
                for l in links {
                    if let Inline::Link { target, .. } = l {
                        if target.starts_with("https://") {
                            continue;
                        }
                        let t = target.trim_start_matches('#');
                        let (page, section) = t.split_once('/').map_or((t, None), |(a, b)| (a, Some(b)));
                        let i = page_index(page).unwrap_or_else(|| panic!("{}: link to missing page {target}", p.id));
                        if let Some(s) = section {
                            assert!(pages()[i].sections.iter().any(|(h, _)| slug(h) == s), "{}: no section {target}", p.id);
                        }
                    }
                }
            }
        }
    }
}
