//! Block actions in answers (#72 round 2): every rich block (a code
//! block, a table, a display formula, a Mermaid diagram) carries a small
//! toolbar with Copy and Export ▾. Export saves the block as a file:
//!
//! - code: a source file named by the fence's language (`rust` → `.rs`);
//! - table: CSV, Markdown or an Excel workbook (`tern_agent::docs::write_xlsx`);
//! - formula: `.tex`, SVG or PNG;
//! - diagram: `.mmd`, SVG or PNG;
//! - the whole answer (its More ▾ menu): Markdown.
//!
//! "Save as <format>" writes into the chat's working folder (the opened
//! folder; the Downloads folder when none is open) under a suggested
//! name, never over a file that is there (`name-1.rs`, `name-2.rs`), and
//! a toast names the path with "Open folder". "Save as…" opens the
//! system's save dialog; the extension picked there chooses the format.
//!
//! The text view draws blocks without the window's state, so the toolbar
//! hands a [`Job`] to the saver the window registers ([`set_saver`]).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, ClipboardItem, SharedString, Window, div, px};

use crate::theme::Palette;

/// What a format's bytes are made from.
#[derive(Clone, Debug)]
pub enum Data {
    /// Written as is (UTF-8).
    Text(Arc<str>),
    /// A table, written as an Excel workbook.
    Xlsx {
        headers: Arc<Vec<String>>,
        rows: Arc<Vec<Vec<String>>>,
    },
    /// An SVG, rasterised to PNG at twice its size.
    Png(Arc<str>),
}

/// One way to save a block.
#[derive(Clone, Debug)]
pub struct Format {
    /// "CSV", "Rust source".
    pub label: &'static str,
    /// Without the dot; empty when `stem` is the whole name (`Dockerfile`).
    pub ext: &'static str,
    pub data: Data,
}

/// A save the toolbar asks for.
#[derive(Clone, Debug)]
pub struct Job {
    /// The suggested name without its extension.
    pub stem: String,
    pub formats: Arc<Vec<Format>>,
    /// The format chosen (for Save as…, the one the dialog suggests).
    pub pick: usize,
    /// Open the save dialog instead of writing into the folder.
    pub save_as: bool,
}

impl Job {
    /// The suggested file name for format `i`.
    pub fn file_name(&self, i: usize) -> String {
        match self.formats.get(i).map(|f| f.ext) {
            Some("") | None => self.stem.clone(),
            Some(ext) => format!("{}.{ext}", self.stem),
        }
    }

    /// The format a chosen path's extension names, else the picked one.
    pub fn format_for(&self, path: &Path) -> usize {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        self.formats
            .iter()
            .position(|f| !f.ext.is_empty() && f.ext == ext)
            .unwrap_or(self.pick)
    }
}

/// What the window does with a [`Job`].
type Saver = Rc<dyn Fn(Job, &mut Window, &mut App)>;

thread_local! {
    static SAVER: RefCell<Option<Saver>> = const { RefCell::new(None) };
}

/// A control's click as the UI tour presses it (`TERN_TOUR=1` only): the
/// element id ("math-…-b", "table-…-export", "code-…-save-2") and what its
/// click does.
type TourClick = (String, Rc<dyn Fn(&mut Window, &mut App)>);

thread_local! {
    static TOUR_CLICKS: RefCell<Vec<TourClick>> = const { RefCell::new(Vec::new()) };
}

fn tour_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("TERN_TOUR").is_some())
}

/// Keeps `f` as what clicking element `id` does, for the UI tour; nothing
/// outside the tour.
pub fn tour_hook(id: String, f: impl Fn(&mut Window, &mut App) + 'static) {
    if !tour_on() {
        return;
    }
    TOUR_CLICKS.with(|c| {
        let mut c = c.borrow_mut();
        c.retain(|(k, _)| *k != id);
        c.push((id, Rc::new(f)));
        if c.len() > 400 {
            c.drain(..100);
        }
    });
}

/// The UI tour clicks the most recently drawn control whose id starts
/// with `prefix` and ends with `suffix`; returns its id.
pub fn tour_click(prefix: &str, suffix: &str, window: &mut Window, cx: &mut App) -> Option<String> {
    let hit = TOUR_CLICKS.with(|c| {
        c.borrow()
            .iter()
            .rev()
            .find(|(k, _)| k.starts_with(prefix) && k.ends_with(suffix))
            .map(|(k, f)| (k.clone(), f.clone()))
    });
    let (id, f) = hit?;
    f(window, cx);
    Some(id)
}

/// Registers the window's saver (it knows the working folder and shows
/// the toast).
pub fn set_saver(f: impl Fn(Job, &mut Window, &mut App) + 'static) {
    SAVER.with(|s| *s.borrow_mut() = Some(Rc::new(f)));
}

/// Hands `job` to the window's saver.
pub fn request(job: Job, window: &mut Window, cx: &mut App) {
    let f = SAVER.with(|s| s.borrow().clone());
    if let Some(f) = f {
        f(job, window, cx);
    }
}

/// The bytes of format `f`.
pub fn bytes_of(f: &Format, cx: &App) -> Result<Vec<u8>, String> {
    match &f.data {
        Data::Text(t) => Ok(t.as_bytes().to_vec()),
        Data::Xlsx { headers, rows } => Ok(tern_agent::docs::write_xlsx("Table", headers, rows)),
        Data::Png(svg) => {
            let image = cx
                .svg_renderer()
                .render_single_frame(svg.as_bytes(), 2.0)
                .map_err(|e| format!("couldn't draw the picture: {e}"))?;
            let size = image.size(0);
            let bgra = image.as_bytes(0).ok_or("couldn't draw the picture")?;
            tern_agent::pictures::png_from_bgra(
                size.width.0.max(0) as u32,
                size.height.0.max(0) as u32,
                bgra,
            )
            .ok_or_else(|| "couldn't encode the PNG".to_owned())
        }
    }
}

/// Where "Save" writes: the opened folder (a file's own folder when a
/// file was opened as the workspace), else Downloads, else home.
pub fn save_dir(workspace: Option<&Path>, home: &Path) -> PathBuf {
    if let Some(w) = workspace {
        if w.is_dir() {
            return w.to_path_buf();
        }
        if let Some(parent) = w.parent().filter(|p| p.is_dir()) {
            return parent.to_path_buf();
        }
    }
    let downloads = home.join("Downloads");
    if downloads.is_dir() {
        downloads
    } else {
        home.to_path_buf()
    }
}

/// `dir/name`, or `stem-1.ext`, `stem-2.ext`… when that is taken: a save
/// never replaces a file.
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, Some(e)),
        _ => (name, None),
    };
    (1..10_000)
        .map(|n| match ext {
            Some(e) => dir.join(format!("{stem}-{n}.{e}")),
            None => dir.join(format!("{stem}-{n}")),
        })
        .find(|p| !p.exists())
        .unwrap_or(first)
}

/// A code fence's language as a file extension (or, for `Dockerfile`
/// and `Makefile`, the whole name) and the format's label.
pub fn lang_file(lang: &str) -> (&'static str, &'static str) {
    let l = lang.trim().to_ascii_lowercase();
    let l = l.split([' ', ',', '{']).next().unwrap_or("");
    match l {
        "rust" | "rs" => ("rs", "Rust source"),
        "python" | "py" | "python3" => ("py", "Python source"),
        "javascript" | "js" | "node" => ("js", "JavaScript"),
        "typescript" | "ts" => ("ts", "TypeScript"),
        "tsx" => ("tsx", "TSX"),
        "jsx" => ("jsx", "JSX"),
        "json" | "jsonc" => ("json", "JSON"),
        "yaml" | "yml" => ("yaml", "YAML"),
        "toml" => ("toml", "TOML"),
        "bash" | "sh" | "shell" | "zsh" | "console" => ("sh", "Shell script"),
        "powershell" | "ps1" | "pwsh" => ("ps1", "PowerShell script"),
        "bat" | "cmd" | "batch" => ("bat", "Batch file"),
        "c" | "h" => ("c", "C source"),
        "cpp" | "c++" | "cc" | "cxx" | "hpp" => ("cpp", "C++ source"),
        "csharp" | "cs" | "c#" => ("cs", "C# source"),
        "java" => ("java", "Java source"),
        "kotlin" | "kt" => ("kt", "Kotlin source"),
        "go" | "golang" => ("go", "Go source"),
        "ruby" | "rb" => ("rb", "Ruby source"),
        "php" => ("php", "PHP source"),
        "swift" => ("swift", "Swift source"),
        "sql" | "postgres" | "postgresql" | "mysql" | "sqlite" => ("sql", "SQL"),
        "html" | "htm" => ("html", "HTML"),
        "css" => ("css", "CSS"),
        "scss" => ("scss", "SCSS"),
        "markdown" | "md" => ("md", "Markdown"),
        "xml" | "svg" => ("xml", "XML"),
        "dockerfile" | "docker" => ("", "Dockerfile"),
        "makefile" | "make" => ("", "Makefile"),
        "lua" => ("lua", "Lua source"),
        "r" => ("r", "R source"),
        "dart" => ("dart", "Dart source"),
        "scala" => ("scala", "Scala source"),
        "haskell" | "hs" => ("hs", "Haskell source"),
        "elixir" | "ex" => ("ex", "Elixir source"),
        "erlang" | "erl" => ("erl", "Erlang source"),
        "julia" | "jl" => ("jl", "Julia source"),
        "perl" | "pl" => ("pl", "Perl source"),
        "matlab" | "octave" => ("m", "MATLAB source"),
        "latex" | "tex" => ("tex", "LaTeX"),
        "mermaid" | "mmd" => ("mmd", "Mermaid"),
        "diff" | "patch" => ("diff", "Diff"),
        "ini" | "cfg" => ("ini", "INI"),
        "csv" => ("csv", "CSV"),
        "graphql" | "gql" => ("graphql", "GraphQL"),
        "proto" | "protobuf" => ("proto", "Protocol Buffers"),
        "vue" => ("vue", "Vue component"),
        "svelte" => ("svelte", "Svelte component"),
        "zig" => ("zig", "Zig source"),
        "ocaml" | "ml" => ("ml", "OCaml source"),
        "fsharp" | "fs" => ("fs", "F# source"),
        "clojure" | "clj" => ("clj", "Clojure source"),
        "groovy" | "gradle" => ("groovy", "Groovy source"),
        "asm" | "assembly" | "nasm" => ("asm", "Assembly"),
        "cmake" => ("cmake", "CMake"),
        "nix" => ("nix", "Nix"),
        "solidity" | "sol" => ("sol", "Solidity source"),
        "verilog" | "v" => ("v", "Verilog source"),
        "vhdl" => ("vhd", "VHDL source"),
        _ => ("txt", "Text file"),
    }
}

/// The suggested name for a code block: `Dockerfile` and `Makefile` by
/// kind, else `snippet`.
fn code_stem(lang: &str) -> &'static str {
    match lang_file(lang) {
        ("", label) => label,
        _ => "snippet",
    }
}

/// A table as CSV (RFC 4180: fields with commas, quotes or line breaks
/// are quoted, quotes doubled).
pub fn csv(headers: &[String], rows: &[Vec<String>]) -> String {
    fn field(s: &str) -> String {
        if s.contains([',', '"', '\n', '\r']) || s.starts_with(' ') || s.ends_with(' ') {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.to_owned()
        }
    }
    let mut out = String::new();
    for r in std::iter::once(headers).chain(rows.iter().map(Vec::as_slice)) {
        let line: Vec<String> = r.iter().map(|c| field(c)).collect();
        out.push_str(&line.join(","));
        out.push_str("\r\n");
    }
    out
}

/// A file-name-safe stem from a title: letters, digits and dashes, at
/// most 48 characters; `fallback` when nothing is left.
pub fn slug(title: &str, fallback: &str) -> String {
    let mut s = String::new();
    for c in title.chars() {
        if c.is_alphanumeric() {
            s.extend(c.to_lowercase());
        } else if !s.ends_with('-') && !s.is_empty() {
            s.push('-');
        }
        if s.chars().count() >= 48 {
            break;
        }
    }
    let s = s.trim_matches('-').to_owned();
    if s.is_empty() { fallback.to_owned() } else { s }
}

/// An answer's suggested name: its first heading, else `answer`.
pub fn answer_stem(markdown: &str) -> String {
    let title = markdown
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with('#'))
        .map(|l| l.trim_start_matches('#').trim());
    slug(title.unwrap_or(""), "answer")
}

pub fn code_formats(lang: &str, code: &str) -> (String, Vec<Format>) {
    let (ext, label) = lang_file(lang);
    (
        code_stem(lang).to_owned(),
        vec![Format {
            label,
            ext,
            data: Data::Text(Arc::from(code)),
        }],
    )
}

pub fn table_formats(
    headers: &[String],
    rows: &[Vec<String>],
    markdown: &str,
) -> (String, Vec<Format>) {
    (
        "table".to_owned(),
        vec![
            Format {
                label: "CSV",
                ext: "csv",
                data: Data::Text(Arc::from(csv(headers, rows))),
            },
            Format {
                label: "Markdown",
                ext: "md",
                data: Data::Text(Arc::from(format!("{}\n", markdown.trim_end()))),
            },
            Format {
                label: "Excel workbook",
                ext: "xlsx",
                data: Data::Xlsx {
                    headers: Arc::new(headers.to_vec()),
                    rows: Arc::new(rows.to_vec()),
                },
            },
        ],
    )
}

pub fn math_formats(latex: &str, svg: Option<&str>) -> (String, Vec<Format>) {
    let mut f = vec![Format {
        label: "LaTeX",
        ext: "tex",
        data: Data::Text(Arc::from(format!("\\[\n{}\n\\]\n", latex.trim()))),
    }];
    if let Some(svg) = svg {
        f.push(Format {
            label: "SVG",
            ext: "svg",
            data: Data::Text(Arc::from(svg)),
        });
        f.push(Format {
            label: "PNG",
            ext: "png",
            data: Data::Png(Arc::from(svg)),
        });
    }
    ("formula".to_owned(), f)
}

pub fn mermaid_formats(source: &str, svg: Option<&str>) -> (String, Vec<Format>) {
    let mut f = vec![Format {
        label: "Mermaid",
        ext: "mmd",
        data: Data::Text(Arc::from(format!("{}\n", source.trim_end()))),
    }];
    if let Some(svg) = svg {
        f.push(Format {
            label: "SVG",
            ext: "svg",
            data: Data::Text(Arc::from(svg)),
        });
        f.push(Format {
            label: "PNG",
            ext: "png",
            data: Data::Png(Arc::from(svg)),
        });
    }
    ("diagram".to_owned(), f)
}

pub fn answer_formats(markdown: &str) -> (String, Vec<Format>) {
    (
        answer_stem(markdown),
        vec![Format {
            label: "Markdown",
            ext: "md",
            data: Data::Text(Arc::from(format!("{}\n", markdown.trim_end()))),
        }],
    )
}

// ------------------------------------------------------------------- UI

fn dark(cx: &App) -> bool {
    gpui_kit::component::Theme::global(cx).is_dark()
}

/// A small icon button in the block's ink.
fn tool(
    id: SharedString,
    icon: AssetIcon,
    tip: &'static str,
    p: Palette,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(3.))
        .px(px(4.))
        .py(px(2.))
        .rounded_md()
        .cursor_pointer()
        .text_xs()
        .text_color(p.faint)
        .hover(move |d| d.text_color(p.text).bg(p.hover))
        .tooltip(move |window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx)
        })
        .child(gpui_kit::component::Icon::new(icon).size(px(12.)))
}

/// Two labels, one lit: Diagram | Code, Rendered | LaTeX. `alt` lights
/// the second; a click flips `state`.
pub fn segmented(
    id: &str,
    labels: (&'static str, &'static str),
    alt: bool,
    state: gpui_kit::Entity<bool>,
    cx: &App,
) -> AnyElement {
    let p = Palette::get(dark(cx));
    let seg = |k: &'static str, label: &'static str, lit: bool| {
        div()
            .id(SharedString::from(format!("{id}-{k}")))
            .px(px(6.))
            .py(px(1.))
            .rounded(px(4.))
            .text_xs()
            .cursor_pointer()
            .text_color(if lit { p.text } else { p.faint })
            .when(lit, |d| d.bg(p.hover))
            .when(!lit, |d| d.hover(move |d| d.text_color(p.muted)))
            .child(label)
    };
    for (k, v) in [("a", false), ("b", true)] {
        let st = state.clone();
        tour_hook(format!("{id}-{k}"), move |_, cx| {
            st.update(cx, |s, cx| {
                *s = v;
                cx.notify();
            })
        });
    }
    let (s1, s2) = (state.clone(), state);
    div()
        .flex()
        .items_center()
        .gap(px(1.))
        .p(px(1.))
        .rounded(px(5.))
        .border_1()
        .border_color(p.border)
        .child(seg("a", labels.0, !alt).on_click(move |_, _, cx| {
            s1.update(cx, |v, cx| {
                *v = false;
                cx.notify();
            })
        }))
        .child(seg("b", labels.1, alt).on_click(move |_, _, cx| {
            s2.update(cx, |v, cx| {
                *v = true;
                cx.notify();
            })
        }))
        .into_any_element()
}

/// Copy and Export ▾ for one block; the open menu lists each format
/// ("Save as CSV") and "Save as…". `id` names the block (its state is
/// kept under it). `float`: the menu floats over what is below (a code
/// block's corner) instead of pushing it down.
pub fn toolbar(
    id: &str,
    copy: Arc<str>,
    stem: String,
    formats: Vec<Format>,
    float: bool,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let p = Palette::get(dark(cx));
    let open = window.use_keyed_state(SharedString::from(format!("{id}-menu")), cx, |_, _| false);
    let copied = window.use_keyed_state(SharedString::from(format!("{id}-copied")), cx, |_, _| {
        None::<Instant>
    });
    let is_open = *open.read(cx);
    let just_copied = copied
        .read(cx)
        .is_some_and(|t| t.elapsed() < Duration::from_millis(1500));
    let formats = Arc::new(formats);
    if !formats.is_empty() {
        let open = open.clone();
        tour_hook(format!("{id}-export"), move |_, cx| {
            open.update(cx, |o, cx| {
                *o = !*o;
                cx.notify();
            })
        });
        for i in 0..formats.len() {
            let job = Job {
                stem: stem.clone(),
                formats: formats.clone(),
                pick: i,
                save_as: false,
            };
            let open = open.clone();
            tour_hook(format!("{id}-save-{i}"), move |window, cx| {
                open.update(cx, |o, cx| {
                    *o = false;
                    cx.notify();
                });
                request(job.clone(), window, cx);
            });
        }
    }
    let row = div()
        .flex()
        .items_center()
        .gap(px(2.))
        .child({
            let copied = copied.clone();
            tool(
                SharedString::from(format!("{id}-copy")),
                if just_copied {
                    AssetIcon::Check
                } else {
                    AssetIcon::Copy
                },
                "Copy",
                p,
            )
            .on_click(move |_, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(copy.to_string()));
                copied.update(cx, |c, cx| {
                    *c = Some(Instant::now());
                    cx.notify();
                });
            })
        })
        .when(!formats.is_empty(), |d| {
            let open = open.clone();
            d.child(
                tool(
                    SharedString::from(format!("{id}-export")),
                    AssetIcon::Download,
                    "Export: save this block as a file",
                    p,
                )
                .when(is_open, |d| d.bg(p.hover).text_color(p.text))
                .child(gpui_kit::component::Icon::new(AssetIcon::ChevronDown).size(px(12.)))
                .on_click(move |_, _, cx| {
                    open.update(cx, |o, cx| {
                        *o = !*o;
                        cx.notify();
                    })
                }),
            )
        });
    let menu = is_open.then(|| {
        let item = |k: String, icon: AssetIcon, label: String| {
            div()
                .id(SharedString::from(format!("{id}-{k}")))
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .py(px(3.))
                .rounded_md()
                .text_xs()
                .cursor_pointer()
                .text_color(p.text)
                .hover(move |d| d.bg(p.hover))
                .child(
                    div()
                        .flex_none()
                        .text_color(p.muted)
                        .child(gpui_kit::component::Icon::new(icon).size(px(12.))),
                )
                .child(SharedString::from(label))
        };
        let mut m = div()
            .flex()
            .flex_col()
            .min_w(px(190.))
            .p_1()
            .rounded_lg()
            .border_1()
            .border_color(p.border)
            .bg(p.raised)
            .when(float, |d| d.shadow_md());
        for (i, f) in formats.iter().enumerate() {
            let job = Job {
                stem: stem.clone(),
                formats: formats.clone(),
                pick: i,
                save_as: false,
            };
            let name = job.file_name(i);
            let open = open.clone();
            m = m.child(
                item(
                    format!("save-{i}"),
                    AssetIcon::Download,
                    format!("Save {name}"),
                )
                .tooltip({
                    let label = f.label;
                    move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(format!(
                            "{label}, saved into the chat's folder"
                        ))
                        .build(window, cx)
                    }
                })
                .on_click(move |_, window, cx| {
                    open.update(cx, |o, cx| {
                        *o = false;
                        cx.notify();
                    });
                    request(job.clone(), window, cx);
                }),
            );
        }
        let job = Job {
            stem: stem.clone(),
            formats: formats.clone(),
            pick: 0,
            save_as: true,
        };
        let open = open.clone();
        m.child(div().my(px(3.)).h(px(1.)).bg(p.border)).child(
            item("save-as".into(), AssetIcon::Save, "Save as…".into()).on_click(
                move |_, window, cx| {
                    open.update(cx, |o, cx| {
                        *o = false;
                        cx.notify();
                    });
                    request(job.clone(), window, cx);
                },
            ),
        )
    });
    div()
        .flex()
        .flex_col()
        .items_end()
        .gap_1()
        .child(row)
        .children(menu)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fence_languages_name_their_files() {
        assert_eq!(lang_file("rust").0, "rs");
        assert_eq!(lang_file("Python").0, "py");
        assert_eq!(lang_file("c++").0, "cpp");
        assert_eq!(lang_file("bash").0, "sh");
        assert_eq!(lang_file("rust,ignore").0, "rs");
        assert_eq!(lang_file("").0, "txt");
        assert_eq!(lang_file("whatever").0, "txt");
        let (stem, f) = code_formats("dockerfile", "FROM rust");
        let job = Job {
            stem,
            formats: Arc::new(f),
            pick: 0,
            save_as: false,
        };
        assert_eq!(job.file_name(0), "Dockerfile");
        let (stem, f) = code_formats("python", "print(1)");
        let job = Job {
            stem,
            formats: Arc::new(f),
            pick: 0,
            save_as: false,
        };
        assert_eq!(job.file_name(0), "snippet.py");
    }

    #[test]
    fn tables_export_as_csv_markdown_and_xlsx() {
        let h = vec!["Name".to_owned(), "Note".to_owned()];
        let r = vec![vec!["Ali".to_owned(), "says \"hi\", twice".to_owned()]];
        assert_eq!(
            csv(&h, &r),
            "Name,Note\r\nAli,\"says \"\"hi\"\", twice\"\r\n"
        );
        let (stem, f) = table_formats(&h, &r, "| Name | Note |");
        assert_eq!(stem, "table");
        let exts: Vec<_> = f.iter().map(|f| f.ext).collect();
        assert_eq!(exts, ["csv", "md", "xlsx"]);
        let job = Job {
            stem,
            formats: Arc::new(f),
            pick: 0,
            save_as: true,
        };
        assert_eq!(job.format_for(Path::new("/x/marks.XLSX")), 2);
        assert_eq!(job.format_for(Path::new("/x/marks.txt")), 0);
    }

    #[test]
    fn a_save_never_replaces_a_file() {
        let dir = std::env::temp_dir().join(format!("tern-blockexport-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(unique_path(&dir, "snippet.rs"), dir.join("snippet.rs"));
        std::fs::write(dir.join("snippet.rs"), "a").unwrap();
        assert_eq!(unique_path(&dir, "snippet.rs"), dir.join("snippet-1.rs"));
        std::fs::write(dir.join("snippet-1.rs"), "b").unwrap();
        assert_eq!(unique_path(&dir, "snippet.rs"), dir.join("snippet-2.rs"));
        std::fs::write(dir.join("Dockerfile"), "c").unwrap();
        assert_eq!(unique_path(&dir, "Dockerfile"), dir.join("Dockerfile-1"));
        // The folder: the opened one, a file's own, else Downloads or home.
        assert_eq!(save_dir(Some(&dir), Path::new("/nowhere")), dir);
        assert_eq!(
            save_dir(Some(&dir.join("snippet.rs")), Path::new("/nowhere")),
            dir
        );
        assert_eq!(save_dir(None, &dir), dir);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn answers_are_named_by_their_first_heading() {
        assert_eq!(
            answer_stem("# Transformer notes\n\ntext"),
            "transformer-notes"
        );
        assert_eq!(answer_stem("no heading"), "answer");
        assert_eq!(
            slug("Self-Attention: Q, K & V!", "x"),
            "self-attention-q-k-v"
        );
    }
}
