//! The block focus view's state (#76): any rich block in an answer (a
//! code block, a table, a display formula, a Mermaid diagram, a picture)
//! opens over the chat to be read large, edited, zoomed and exported.
//!
//! One system for every kind: a [`Block`] says what it is ([`Kind`]) and
//! what the view offers for it ([`Caps`]); [`Focus`] holds the draft being
//! edited, its [`Versions`] (the original is always kept), the zoom and
//! pan of a drawing ([`ZoomPan`]) and a table's sort and filter. A new kind
//! is a new [`Kind`] arm with its [`Caps`], its markdown
//! ([`Focus::markdown`]) and its body in `view/focus_view.rs`.
//!
//! "Save to answer" puts the edited block back into the answer it came
//! from ([`replace_block`]) and keeps the edit beside the chat
//! ([`Edits`]), so the answer still reads that way when the chat reopens.
//!
//! GPUI-free, so it is unit tested.

use std::path::{Path, PathBuf};

/// What a block is.
#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// A fenced code block and its fence's language ("" when none).
    Code { lang: String },
    /// A Markdown table.
    Table,
    /// A display formula (LaTeX).
    Math,
    /// A Mermaid diagram (its source).
    Mermaid,
    /// A picture: an SVG (its source is the block's text) or a raster
    /// image the view holds (the text is its name).
    Image { svg: bool },
}

/// What the focus view offers for a kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caps {
    /// A source editor.
    pub edit: bool,
    /// A drawing beside the source that follows each edit.
    pub preview: bool,
    /// Zoom and pan on the drawing.
    pub zoom: bool,
    /// The grid: sort, filter, cell edits, rows and columns.
    pub grid: bool,
    /// Can go back into the answer ("Save to answer").
    pub saves: bool,
}

impl Kind {
    pub fn label(&self) -> &'static str {
        match self {
            Kind::Code { .. } => "Code",
            Kind::Table => "Table",
            Kind::Math => "Math",
            Kind::Mermaid => "Diagram",
            Kind::Image { .. } => "Picture",
        }
    }

    pub fn caps(&self) -> Caps {
        let c = Caps {
            edit: false,
            preview: false,
            zoom: false,
            grid: false,
            saves: true,
        };
        match self {
            Kind::Code { .. } => Caps { edit: true, ..c },
            Kind::Table => Caps { grid: true, ..c },
            Kind::Math | Kind::Mermaid => Caps {
                edit: true,
                preview: true,
                zoom: true,
                ..c
            },
            Kind::Image { .. } => Caps {
                zoom: true,
                saves: false,
                ..c
            },
        }
    }

    /// The source editor's language (syntax colours).
    pub fn editor_language(&self) -> &str {
        match self {
            Kind::Code { lang } => lang.as_str(),
            Kind::Math => "latex",
            Kind::Mermaid => "mermaid",
            Kind::Table | Kind::Image { .. } => "text",
        }
    }
}

/// A block as the answer has it.
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub kind: Kind,
    /// Code, LaTeX, Mermaid source, an SVG, or a table's Markdown.
    pub source: String,
    /// A table's cells.
    pub table: Option<Table>,
}

impl Block {
    pub fn code(lang: &str, code: &str) -> Self {
        Block {
            kind: Kind::Code {
                lang: lang.to_owned(),
            },
            source: code.to_owned(),
            table: None,
        }
    }

    pub fn math(latex: &str) -> Self {
        Block {
            kind: Kind::Math,
            source: latex.trim().to_owned(),
            table: None,
        }
    }

    pub fn mermaid(source: &str) -> Self {
        Block {
            kind: Kind::Mermaid,
            source: source.to_owned(),
            table: None,
        }
    }

    pub fn table(headers: &[String], rows: &[Vec<String>], markdown: &str) -> Self {
        Block {
            kind: Kind::Table,
            source: markdown.to_owned(),
            table: Some(Table {
                headers: headers.to_vec(),
                rows: rows.to_vec(),
            }),
        }
    }
}

// ----------------------------------------------------------------- table

/// A table's cells.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Table {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

/// A number in a cell, read loosely: "1,200", "$5", "12%", "-3.5".
fn number(s: &str) -> Option<f64> {
    let t: String = s
        .trim()
        .chars()
        .filter(|c| !matches!(c, ',' | '$' | '%' | '€' | '£' | ' '))
        .collect();
    if t.is_empty() {
        return None;
    }
    t.parse::<f64>().ok()
}

impl Table {
    pub fn cols(&self) -> usize {
        self.headers
            .len()
            .max(self.rows.iter().map(Vec::len).max().unwrap_or(0))
    }

    pub fn cell(&self, row: usize, col: usize) -> &str {
        self.rows
            .get(row)
            .and_then(|r| r.get(col))
            .map_or("", String::as_str)
    }

    /// Every row as wide as the table.
    fn square(&mut self) {
        let n = self.cols();
        self.headers.resize(n, String::new());
        for r in &mut self.rows {
            r.resize(n, String::new());
        }
    }

    /// Sorts the rows by column `col`: as numbers when every filled cell
    /// is one, else as text (case-insensitive). Empty cells go last.
    pub fn sort(&mut self, col: usize, ascending: bool) {
        let numeric = self
            .rows
            .iter()
            .map(|r| r.get(col).map_or("", String::as_str))
            .filter(|c| !c.trim().is_empty())
            .all(|c| number(c).is_some());
        self.rows.sort_by(|a, b| {
            let (x, y) = (
                a.get(col).map_or("", String::as_str),
                b.get(col).map_or("", String::as_str),
            );
            match (x.trim().is_empty(), y.trim().is_empty()) {
                (true, true) => return std::cmp::Ordering::Equal,
                (true, false) => return std::cmp::Ordering::Greater,
                (false, true) => return std::cmp::Ordering::Less,
                _ => {}
            }
            let o = if numeric {
                number(x)
                    .unwrap_or(0.)
                    .partial_cmp(&number(y).unwrap_or(0.))
                    .unwrap_or(std::cmp::Ordering::Equal)
            } else {
                x.to_lowercase().cmp(&y.to_lowercase())
            };
            if ascending { o } else { o.reverse() }
        });
    }

    /// The rows a filter keeps: those with a cell containing every word
    /// of `query` (case-insensitive). All rows for an empty query.
    pub fn visible(&self, query: &str) -> Vec<usize> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        (0..self.rows.len())
            .filter(|&i| {
                let line = self.rows[i].join(" \u{1}").to_lowercase();
                words.iter().all(|w| line.contains(w.as_str()))
            })
            .collect()
    }

    /// Sets a cell (`row` None: the header).
    pub fn set(&mut self, row: Option<usize>, col: usize, value: &str) {
        self.square();
        let v = value.replace(['\n', '\r'], " ");
        match row {
            None => {
                if let Some(h) = self.headers.get_mut(col) {
                    *h = v;
                }
            }
            Some(r) => {
                if let Some(c) = self.rows.get_mut(r).and_then(|r| r.get_mut(col)) {
                    *c = v;
                }
            }
        }
    }

    /// Adds an empty row after `after` (at the end when None); returns its index.
    pub fn add_row(&mut self, after: Option<usize>) -> usize {
        self.square();
        let at = after.map_or(self.rows.len(), |a| (a + 1).min(self.rows.len()));
        self.rows.insert(at, vec![String::new(); self.cols()]);
        at
    }

    pub fn remove_row(&mut self, row: usize) {
        if row < self.rows.len() {
            self.rows.remove(row);
        }
    }

    /// Adds a column at the end, headed `name`; returns its index.
    pub fn add_col(&mut self, name: &str) -> usize {
        self.square();
        self.headers.push(name.to_owned());
        for r in &mut self.rows {
            r.push(String::new());
        }
        self.headers.len() - 1
    }

    /// Removes a column; a table keeps at least one.
    pub fn remove_col(&mut self, col: usize) {
        self.square();
        if self.headers.len() <= 1 || col >= self.headers.len() {
            return;
        }
        self.headers.remove(col);
        for r in &mut self.rows {
            if col < r.len() {
                r.remove(col);
            }
        }
    }

    /// The table as GitHub Markdown (pipes in cells escaped).
    pub fn to_markdown(&self) -> String {
        let n = self.cols().max(1);
        let cell = |s: &str| s.replace('|', "\\|");
        let line = |cells: &[String]| {
            let mut v: Vec<String> = cells.iter().map(|c| cell(c)).collect();
            v.resize(n, String::new());
            format!("| {} |", v.join(" | "))
        };
        let mut out = vec![line(&self.headers)];
        out.push(format!("|{}|", vec![" --- "; n].join("|")));
        out.extend(self.rows.iter().map(|r| line(r)));
        out.join("\n")
    }
}

// -------------------------------------------------------------- versions

/// A block's versions: the original first, then each saved edit. The
/// original is never dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Versions {
    items: Vec<String>,
    current: usize,
}

impl Versions {
    pub fn new(original: &str) -> Self {
        Versions {
            items: vec![original.to_owned()],
            current: 0,
        }
    }

    /// A block saved before: its original and each edit since.
    pub fn with_history(history: Vec<String>) -> Self {
        let items = if history.is_empty() {
            vec![String::new()]
        } else {
            history
        };
        Versions {
            current: items.len() - 1,
            items,
        }
    }

    #[cfg(test)]
    pub fn original(&self) -> &str {
        &self.items[0]
    }

    pub fn current(&self) -> &str {
        &self.items[self.current]
    }

    pub fn index(&self) -> usize {
        self.current
    }

    pub fn count(&self) -> usize {
        self.items.len()
    }

    pub fn all(&self) -> &[String] {
        &self.items
    }

    /// Keeps `text` as the newest version (nothing when it is the current
    /// one). Returns whether it was new.
    pub fn push(&mut self, text: &str) -> bool {
        if text == self.current() {
            return false;
        }
        self.items.push(text.to_owned());
        self.current = self.items.len() - 1;
        true
    }

    /// Makes version `i` current; returns its text.
    pub fn pick(&mut self, i: usize) -> &str {
        self.current = i.min(self.items.len() - 1);
        self.current()
    }
}

// --------------------------------------------------------------- zoom/pan

/// The drawing's zoom and pan. `pan` is where the drawing's top-left sits
/// in the viewport, in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZoomPan {
    pub zoom: f32,
    pub pan: (f32, f32),
}

pub const ZOOM_MIN: f32 = 0.1;
pub const ZOOM_MAX: f32 = 8.0;

impl Default for ZoomPan {
    fn default() -> Self {
        ZoomPan {
            zoom: 1.0,
            pan: (0., 0.),
        }
    }
}

impl ZoomPan {
    /// Zooms by `factor` keeping the point under `anchor` (viewport px)
    /// where it is.
    pub fn zoom_at(&mut self, factor: f32, anchor: (f32, f32)) {
        let z = (self.zoom * factor).clamp(ZOOM_MIN, ZOOM_MAX);
        let k = z / self.zoom;
        self.pan = (
            anchor.0 - (anchor.0 - self.pan.0) * k,
            anchor.1 - (anchor.1 - self.pan.1) * k,
        );
        self.zoom = z;
    }

    pub fn pan_by(&mut self, dx: f32, dy: f32) {
        self.pan = (self.pan.0 + dx, self.pan.1 + dy);
    }

    /// The whole drawing (`content`, at zoom 1) centred in `viewport`
    /// with a margin, never above 2x.
    pub fn fit(&mut self, content: (f32, f32), viewport: (f32, f32)) {
        let (cw, ch) = (content.0.max(1.), content.1.max(1.));
        let (vw, vh) = ((viewport.0 - 48.).max(1.), (viewport.1 - 48.).max(1.));
        self.zoom = (vw / cw).min(vh / ch).clamp(ZOOM_MIN, 2.0);
        self.pan = (
            (viewport.0 - cw * self.zoom) / 2.,
            (viewport.1 - ch * self.zoom) / 2.,
        );
    }

    pub fn percent(&self) -> u32 {
        (self.zoom * 100.).round() as u32
    }
}

// ---------------------------------------------------------------- mermaid

/// A flowchart's direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    /// Left to right.
    LR,
    /// Top to bottom.
    TB,
}

/// The header line of a flowchart (`graph TD`, `flowchart LR`): its line
/// index, keyword and direction word.
fn flow_header(source: &str) -> Option<(usize, String, Option<String>)> {
    let (i, line) = source
        .lines()
        .enumerate()
        .find(|(_, l)| !l.trim().is_empty() && !l.trim_start().starts_with("%%"))?;
    let mut words = line.split_whitespace();
    let kw = words.next()?;
    if !kw.eq_ignore_ascii_case("graph") && !kw.eq_ignore_ascii_case("flowchart") {
        return None;
    }
    let dir = words
        .next()
        .map(|w| w.trim_end_matches(';').to_owned())
        .filter(|w| !w.is_empty());
    Some((i, kw.to_owned(), dir))
}

/// A flowchart's direction, None for other diagrams.
pub fn mermaid_direction(source: &str) -> Option<Dir> {
    let (_, _, d) = flow_header(source)?;
    Some(match d.as_deref().map(str::to_ascii_uppercase).as_deref() {
        Some("LR") | Some("RL") => Dir::LR,
        _ => Dir::TB,
    })
}

/// The source with the flowchart turned to `dir`; unchanged for other
/// diagrams.
pub fn set_mermaid_direction(source: &str, dir: Dir) -> String {
    let Some((i, kw, _)) = flow_header(source) else {
        return source.to_owned();
    };
    let word = match dir {
        Dir::LR => "LR",
        Dir::TB => "TB",
    };
    let mut out: Vec<String> = source.lines().map(str::to_owned).collect();
    let line = &out[i];
    let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
    // Anything after the direction (`graph TD;`) is kept.
    let rest = line
        .split_whitespace()
        .skip(2)
        .collect::<Vec<_>>()
        .join(" ");
    // `graph TD;` keeps its semicolon after the new direction.
    let semi = line
        .split_whitespace()
        .nth(1)
        .is_some_and(|w| w.ends_with(';'));
    out[i] = format!(
        "{indent}{kw} {word}{}{}",
        if semi { ";" } else { "" },
        if rest.is_empty() {
            String::new()
        } else {
            format!(" {rest}")
        }
    );
    let mut s = out.join("\n");
    if source.ends_with('\n') {
        s.push('\n');
    }
    s
}

// ------------------------------------------------------------------ focus

/// The focus view's state.
#[derive(Clone, Debug)]
pub struct Focus {
    pub block: Block,
    pub versions: Versions,
    /// The text being edited (code, LaTeX, Mermaid); a table's Markdown.
    pub draft: String,
    /// A table being edited.
    pub table: Option<Table>,
    pub view: ZoomPan,
    /// The view has not been fitted to its drawing yet.
    pub needs_fit: bool,
    pub wrap: bool,
    /// A table's sort: column and ascending.
    pub sort: Option<(usize, bool)>,
    pub filter: String,
}

impl Focus {
    pub fn new(block: Block) -> Self {
        Self::with_history(block, Vec::new())
    }

    /// A block saved before: `history` is its original and each saved
    /// version, the last being the block as the answer has it now.
    pub fn with_history(block: Block, history: Vec<String>) -> Self {
        let versions = if history.len() > 1 && history.last() == Some(&block.source) {
            Versions::with_history(history)
        } else {
            Versions::new(&block.source)
        };
        Focus {
            draft: block.source.clone(),
            table: block.table.clone(),
            versions,
            view: ZoomPan::default(),
            needs_fit: true,
            wrap: false,
            sort: None,
            filter: String::new(),
            block,
        }
    }

    pub fn caps(&self) -> Caps {
        self.block.kind.caps()
    }

    /// The text the edits have made: a table's Markdown, else the draft.
    pub fn text(&self) -> String {
        match &self.table {
            Some(t) if self.block.kind == Kind::Table => t.to_markdown(),
            _ => self.draft.clone(),
        }
    }

    /// Whether the draft differs from the version shown in the answer.
    pub fn edited(&self) -> bool {
        match &self.table {
            Some(t) if self.block.kind == Kind::Table => Some(t) != self.block.table.as_ref(),
            _ => self.draft != self.block.source,
        }
    }

    /// The block as Markdown, as it goes into an answer.
    pub fn markdown(&self) -> String {
        let text = self.text();
        match &self.block.kind {
            Kind::Code { lang } => format!("```{lang}\n{}\n```", text.trim_end_matches('\n')),
            Kind::Math => format!("$$\n{}\n$$", text.trim()),
            Kind::Mermaid => format!("```mermaid\n{}\n```", text.trim_end_matches('\n')),
            Kind::Table => text,
            Kind::Image { .. } => text,
        }
    }

    /// Sorts the table by `col`: ascending first, then descending.
    pub fn sort_by(&mut self, col: usize) {
        let asc = !matches!(self.sort, Some((c, true)) if c == col);
        self.sort = Some((col, asc));
        if let Some(t) = &mut self.table {
            t.sort(col, asc);
        }
    }

    /// Takes the edits as the block's new version: returns the old and the
    /// new source, or None when nothing changed.
    pub fn commit(&mut self) -> Option<(String, String)> {
        if !self.edited() {
            return None;
        }
        let old = self.block.source.clone();
        let new = self.text();
        self.versions.push(&new);
        self.block.source = new.clone();
        self.block.table = self.table.clone();
        Some((old, new))
    }

    /// Shows version `i` in the editor (not yet saved).
    pub fn pick_version(&mut self, i: usize) {
        let text = self.versions.pick(i).to_owned();
        if self.block.kind == Kind::Table {
            self.table = Some(parse_table(&text));
        }
        self.draft = text;
    }
}

/// A Markdown table read back into cells (the header, the rule, rows).
pub fn parse_table(md: &str) -> Table {
    let mut lines = md
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with('|') || l.contains('|'));
    let cells = |l: &str| -> Vec<String> {
        let inner = l.trim().trim_start_matches('|');
        let inner = inner.strip_suffix('|').unwrap_or(inner);
        let mut out = Vec::new();
        let mut cur = String::new();
        let mut chars = inner.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\\' if chars.peek() == Some(&'|') => {
                    cur.push('|');
                    chars.next();
                }
                '|' => out.push(std::mem::take(&mut cur).trim().to_owned()),
                c => cur.push(c),
            }
        }
        out.push(cur.trim().to_owned());
        out
    };
    let headers = lines.next().map(cells).unwrap_or_default();
    let rows = lines.filter(|l| !is_rule(l)).map(cells).collect();
    Table { headers, rows }
}

// ------------------------------------------------------- back into answers

/// A table line's cells, for comparing tables written with different
/// spacing.
fn table_cells(line: &str) -> Option<Vec<String>> {
    let t = line.trim();
    if !t.contains('|') {
        return None;
    }
    let row = parse_table(t);
    Some(row.headers)
}

fn is_rule(line: &str) -> bool {
    let t = line.trim();
    t.contains('-') && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ' | '\t'))
}

/// The answer with block `old` (as [`Block::source`] had it) replaced by
/// `new`. Code, formulas and diagrams are found by their text; a table by
/// its cells, however its pipes were spaced. None when the block is not
/// in the answer.
pub fn replace_block(answer: &str, kind: &Kind, old: &str, new: &str) -> Option<String> {
    if *kind != Kind::Table {
        let needle = old.trim_matches('\n');
        if needle.is_empty() {
            return None;
        }
        let at = answer.rfind(needle)?;
        let mut out = String::with_capacity(answer.len() + new.len());
        out.push_str(&answer[..at]);
        out.push_str(new.trim_matches('\n'));
        out.push_str(&answer[at + needle.len()..]);
        return Some(out);
    }
    let want: Vec<Vec<String>> = old
        .lines()
        .filter(|l| !l.trim().is_empty() && !is_rule(l))
        .filter_map(table_cells)
        .collect();
    if want.is_empty() {
        return None;
    }
    let lines: Vec<&str> = answer.split('\n').collect();
    // Each run of table lines in the answer, compared cell by cell.
    let mut i = 0;
    let mut found = None;
    while i < lines.len() {
        if table_cells(lines[i]).is_none() {
            i += 1;
            continue;
        }
        let start = i;
        while i < lines.len() && lines[i].contains('|') {
            i += 1;
        }
        let have: Vec<Vec<String>> = lines[start..i]
            .iter()
            .filter(|l| !is_rule(l))
            .filter_map(|l| table_cells(l))
            .collect();
        if have == want {
            found = Some((start, i));
        }
    }
    let (s, e) = found?;
    let mut out: Vec<String> = lines[..s].iter().map(|l| (*l).to_owned()).collect();
    out.push(new.trim_matches('\n').to_owned());
    out.extend(lines[e..].iter().map(|l| (*l).to_owned()));
    Some(out.join("\n"))
}

// ------------------------------------------------------------ saved edits

/// One block saved back into an answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    /// Which answer: its place among the chat's answers (0 first).
    pub answer: usize,
    pub kind: String,
    /// Every version, the original first, the one the answer shows last.
    pub versions: Vec<String>,
}

/// The block edits of one chat, kept beside it as JSON.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Edits {
    pub list: Vec<Edit>,
}

fn kind_key(k: &Kind) -> &'static str {
    match k {
        Kind::Code { .. } => "code",
        Kind::Table => "table",
        Kind::Math => "math",
        Kind::Mermaid => "mermaid",
        Kind::Image { .. } => "image",
    }
}

fn kind_of(key: &str) -> Kind {
    match key {
        "table" => Kind::Table,
        "math" => Kind::Math,
        "mermaid" => Kind::Mermaid,
        "image" => Kind::Image { svg: true },
        _ => Kind::Code {
            lang: String::new(),
        },
    }
}

impl Edits {
    /// Where a chat's edits are kept.
    pub fn path(dir: &Path, chat: &str) -> PathBuf {
        let safe: String = chat
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        dir.join(format!("{safe}.json"))
    }

    pub fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        Self::from_json(&text)
    }

    pub fn from_json(text: &str) -> Self {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
            return Self::default();
        };
        let list = v
            .get("edits")
            .and_then(|e| e.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|e| {
                        Some(Edit {
                            answer: e.get("answer")?.as_u64()? as usize,
                            kind: e.get("kind")?.as_str()?.to_owned(),
                            versions: e
                                .get("versions")?
                                .as_array()?
                                .iter()
                                .filter_map(|s| s.as_str().map(str::to_owned))
                                .collect(),
                        })
                    })
                    .filter(|e| e.versions.len() >= 2)
                    .collect()
            })
            .unwrap_or_default();
        Edits { list }
    }

    pub fn to_json(&self) -> String {
        let edits: Vec<serde_json::Value> = self
            .list
            .iter()
            .map(|e| {
                serde_json::json!({
                    "answer": e.answer,
                    "kind": e.kind,
                    "versions": e.versions,
                })
            })
            .collect();
        serde_json::to_string_pretty(&serde_json::json!({ "edits": edits }))
            .unwrap_or_else(|_| "{}".to_owned())
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(path, self.to_json()).map_err(|e| e.to_string())
    }

    /// Records a save: `versions` is the block's whole history, the last
    /// being what the answer now shows. A block edited again keeps one
    /// record (found by its earlier text).
    pub fn record(&mut self, answer: usize, kind: &Kind, versions: &[String]) {
        let Some(original) = versions.first() else {
            return;
        };
        let key = kind_key(kind);
        match self
            .list
            .iter_mut()
            .find(|e| e.answer == answer && e.kind == key && e.versions.first() == Some(original))
        {
            Some(e) => e.versions = versions.to_vec(),
            None => self.list.push(Edit {
                answer,
                kind: key.to_owned(),
                versions: versions.to_vec(),
            }),
        }
    }

    /// The history kept for a block the answer now shows as `current`.
    pub fn history(&self, answer: usize, current: &str) -> Vec<String> {
        self.list
            .iter()
            .find(|e| e.answer == answer && e.versions.last().map(String::as_str) == Some(current))
            .map(|e| e.versions.clone())
            .unwrap_or_default()
    }

    /// Puts each saved block back into the answers (`answers[i]` is the
    /// chat's i-th answer), in the order they were saved.
    pub fn apply(&self, answers: &mut [String]) {
        for e in &self.list {
            let (Some(old), Some(new)) = (e.versions.first(), e.versions.last()) else {
                continue;
            };
            let Some(a) = answers.get_mut(e.answer) else {
                continue;
            };
            let kind = kind_of(&e.kind);
            if let Some(t) = replace_block(a, &kind, old, new) {
                *a = t;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_owned()).collect()
    }

    fn table() -> Table {
        Table {
            headers: s(&["Name", "Score"]),
            rows: vec![
                s(&["bo", "10"]),
                s(&["Al", "9"]),
                s(&["cy", ""]),
                s(&["Di", "1,200"]),
            ],
        }
    }

    #[test]
    fn every_kind_says_what_the_view_offers() {
        assert!(Kind::Code { lang: "rs".into() }.caps().edit);
        assert!(!Kind::Code { lang: "rs".into() }.caps().zoom);
        assert!(Kind::Table.caps().grid);
        assert!(Kind::Math.caps().preview && Kind::Math.caps().zoom);
        assert!(Kind::Mermaid.caps().edit && Kind::Mermaid.caps().zoom);
        let pic = Kind::Image { svg: true }.caps();
        assert!(pic.zoom && !pic.edit && !pic.saves);
    }

    #[test]
    fn tables_sort_numbers_as_numbers_and_text_without_case() {
        let mut t = table();
        t.sort(1, true);
        let names: Vec<_> = t.rows.iter().map(|r| r[0].as_str()).collect();
        assert_eq!(names, ["Al", "bo", "Di", "cy"]);
        t.sort(1, false);
        let names: Vec<_> = t.rows.iter().map(|r| r[0].as_str()).collect();
        assert_eq!(names, ["Di", "bo", "Al", "cy"]);
        t.sort(0, true);
        let names: Vec<_> = t.rows.iter().map(|r| r[0].as_str()).collect();
        assert_eq!(names, ["Al", "bo", "cy", "Di"]);
    }

    #[test]
    fn tables_filter_edit_and_grow() {
        let mut t = table();
        assert_eq!(t.visible("al"), [1]);
        assert_eq!(t.visible(""), [0, 1, 2, 3]);
        assert_eq!(t.visible("d 200"), [3]);
        t.set(Some(0), 1, "11");
        t.set(None, 0, "Who");
        assert_eq!((t.headers[0].as_str(), t.cell(0, 1)), ("Who", "11"));
        let r = t.add_row(Some(0));
        assert_eq!((r, t.rows.len(), t.rows[1].len()), (1, 5, 2));
        let c = t.add_col("Note");
        assert_eq!((c, t.rows[3].len()), (2, 3));
        t.remove_col(0);
        assert_eq!(t.headers, s(&["Score", "Note"]));
        t.remove_row(1);
        assert_eq!(t.rows.len(), 4);
        t.remove_col(0);
        t.remove_col(0);
        assert_eq!(t.headers.len(), 1, "a table keeps one column");
    }

    #[test]
    fn tables_round_trip_through_markdown() {
        let mut t = table();
        t.set(Some(0), 0, "a|b");
        let md = t.to_markdown();
        assert!(md.starts_with("| Name | Score |\n| --- | --- |\n| a\\|b | 10 |"));
        assert_eq!(parse_table(&md), t);
    }

    #[test]
    fn versions_keep_the_original() {
        let mut v = Versions::new("one");
        assert!(!v.push("one"));
        assert!(v.push("two"));
        assert!(v.push("three"));
        assert_eq!((v.count(), v.current(), v.original()), (3, "three", "one"));
        assert_eq!(v.pick(0), "one");
        assert!(v.push("four"));
        assert_eq!(v.all(), s(&["one", "two", "three", "four"]));
    }

    #[test]
    fn zoom_keeps_the_point_under_the_cursor() {
        let mut z = ZoomPan::default();
        z.zoom_at(2.0, (100., 50.));
        assert_eq!((z.zoom, z.pan), (2.0, (-100., -50.)));
        z.zoom_at(100.0, (0., 0.));
        assert_eq!(z.zoom, ZOOM_MAX);
        z.fit((400., 200.), (848., 248.));
        assert_eq!(z.zoom, 1.0);
        assert_eq!(z.pan, (224., 24.));
        z.fit((10., 10.), (1000., 1000.));
        assert_eq!(z.zoom, 2.0, "small drawings stop at 2x");
        z.pan_by(5., -5.);
        assert_eq!(z.percent(), 200);
    }

    #[test]
    fn flowcharts_turn_between_lr_and_tb() {
        let src = "%% c\ngraph TD;\n  A-->B\n";
        assert_eq!(mermaid_direction(src), Some(Dir::TB));
        let lr = set_mermaid_direction(src, Dir::LR);
        assert_eq!(lr, "%% c\ngraph LR;\n  A-->B\n");
        assert_eq!(mermaid_direction(&lr), Some(Dir::LR));
        assert_eq!(
            set_mermaid_direction("flowchart\nA-->B", Dir::LR),
            "flowchart LR\nA-->B"
        );
        assert_eq!(
            set_mermaid_direction("graph TD; A-->B", Dir::LR),
            "graph LR; A-->B"
        );
        assert_eq!(mermaid_direction("sequenceDiagram\nA->>B: hi"), None);
        assert_eq!(
            set_mermaid_direction("pie\n\"a\": 1", Dir::LR),
            "pie\n\"a\": 1"
        );
    }

    #[test]
    fn edits_go_back_into_the_answer() {
        let answer = "Intro\n\n```rust\nfn a() {}\n```\n\nand\n\n$$\nx^2\n$$\n";
        let mut f = Focus::new(Block::code("rust", "fn a() {}"));
        assert!(f.commit().is_none(), "nothing changed");
        f.draft = "fn b() {}".into();
        assert!(f.edited());
        let (old, new) = f.commit().unwrap();
        let out = replace_block(answer, &f.block.kind, &old, &new).unwrap();
        assert!(out.contains("```rust\nfn b() {}\n```"));
        assert_eq!(f.versions.all(), s(&["fn a() {}", "fn b() {}"]));
        let out = replace_block(&out, &Kind::Math, "x^2", "x^3").unwrap();
        assert!(out.contains("$$\nx^3\n$$"));
        assert!(replace_block(answer, &Kind::Math, "y", "z").is_none());
    }

    #[test]
    fn tables_are_found_by_their_cells() {
        let answer = "Scores:\n\n|Name|Score|\n|:-|-:|\n|bo|10|\n|Al|9|\n\nEnd";
        let t = Table {
            headers: s(&["Name", "Score"]),
            rows: vec![s(&["bo", "10"]), s(&["Al", "9"])],
        };
        let mut f = Focus::new(Block::table(&t.headers, &t.rows, &t.to_markdown()));
        f.sort_by(1);
        assert_eq!(f.sort, Some((1, true)));
        let (old, new) = f.commit().unwrap();
        let out = replace_block(answer, &Kind::Table, &old, &new).unwrap();
        assert_eq!(
            out,
            "Scores:\n\n| Name | Score |\n| --- | --- |\n| Al | 9 |\n| bo | 10 |\n\nEnd"
        );
    }

    #[test]
    fn saved_edits_come_back_when_the_chat_reopens() {
        let mut e = Edits::default();
        e.record(1, &Kind::Math, &s(&["x^2", "x^3"]));
        e.record(1, &Kind::Math, &s(&["x^2", "x^3", "x^4"]));
        assert_eq!(e.list.len(), 1, "a block edited again keeps one record");
        let back = Edits::from_json(&e.to_json());
        assert_eq!(back, e);
        let mut answers = vec!["$$x^2$$".to_owned(), "a\n$$\nx^2\n$$".to_owned()];
        back.apply(&mut answers);
        assert_eq!(answers, s(&["$$x^2$$", "a\n$$\nx^4\n$$"]));
        assert_eq!(back.history(1, "x^4"), s(&["x^2", "x^3", "x^4"]));
        assert!(back.history(0, "x^4").is_empty());
        let f = Focus::with_history(Block::math("x^4"), back.history(1, "x^4"));
        assert_eq!((f.versions.count(), f.versions.original()), (3, "x^2"));
        assert_eq!(Edits::from_json("not json"), Edits::default());
    }
}
