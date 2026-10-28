//! Rich answers (#72): math, Mermaid diagrams and citation chips inside
//! the Markdown text view.
//!
//! The text view (gpui-base `TextView`) already draws headings, lists,
//! tables, block quotes, links and code blocks, and while an answer
//! streams it re-parses only the last block. This module adds three
//! Markdown plugins to it:
//!
//! - **Math.** `$…$` inline and `$$…$$` display formulas are typeset by
//!   RaTeX (MIT, a Rust port of KaTeX's layout, with the KaTeX fonts,
//!   OFL-1.1) into SVG. A formula RaTeX cannot read stays as its source in
//!   the code font. Dollar amounts ("$5 and $10") are left as text.
//! - **Mermaid.** A ```` ```mermaid ```` block is laid out and drawn by
//!   merman (MIT OR Apache-2.0; built without its EPL-2.0 ELK layout) into
//!   SVG. While the block is still streaming, or when merman cannot read
//!   it, it stays a code block.
//! - **Citation chips.** `[n]` in a finished answer becomes a small chip
//!   that opens the page it cites (`chips`).
//!
//! Parsing and drawing happen in the parser, which the text view runs on
//! a background task for long text, and every result is kept by its
//! source, so a block re-parsed while the answer streams is drawn once.
//! No code is taken from Zed's Markdown or Mermaid crates (GPL).

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock};

use gpui_kit::base::text::CodeBlock;
use gpui_kit::component::text::{
    InlineElement, InlineRenderContext, MarkdownExtensions, MarkdownNode, MarkdownParseContext,
    MarkdownPlugin, TableData, TextView, markdown_ast as mdast,
};
use gpui_kit::prelude::*;
use gpui_kit::{App, Image, ImageFormat, ObjectFit, SharedString, Window, div, img, px};

use crate::theme::Palette;

/// Bumped when the plugins change what they parse, so a text view drops
/// documents parsed by an older set.
const PARSER_REVISION: u64 = 72_002;

/// The plugins, built once and shared by every answer.
pub fn extensions() -> MarkdownExtensions {
    static EXT: OnceLock<MarkdownExtensions> = OnceLock::new();
    EXT.get_or_init(|| {
        MarkdownExtensions::default()
            .parser_revision(PARSER_REVISION)
            .plugin(InlineMath)
            .plugin(DisplayMath)
            .plugin(Mermaid)
            .plugin(Cite)
    })
    .clone()
}

/// A text view set up for answers: the plugins, and Copy · Export ▾ on
/// every code block and table (`crate::blockexport`).
pub fn rich(t: TextView) -> TextView {
    t.markdown_extensions(extensions())
        .code_block_actions(code_tools)
        .table_actions(table_tools)
}

/// An answer's Markdown as the text view should read it: `\( \)`,
/// `\[ \]` and bare `\begin{align}` formulas rewritten to dollar math
/// (`crate::mathsrc`).
pub fn prepare(markdown: &str) -> std::borrow::Cow<'_, str> {
    crate::mathsrc::delimiters(markdown)
}

/// A code block's corner: its language, Copy and Export ▾ (a source file
/// named by the language).
fn code_tools(code: &CodeBlock, window: &mut Window, cx: &mut App) -> gpui_kit::AnyElement {
    let p = Palette::get(is_dark(cx));
    let lang = code.lang().map(|l| l.to_string()).unwrap_or_default();
    let text = code.code();
    let id = format!("code-{}", key(&lang, &text));
    let (stem, formats) = crate::blockexport::code_formats(&lang, &text);
    div()
        .flex()
        .items_start()
        .gap_1()
        .when(!lang.is_empty(), |d| {
            d.child(
                div()
                    .pt(px(3.))
                    .pr_1()
                    .text_xs()
                    .text_color(p.faint)
                    .child(SharedString::from(lang.clone())),
            )
        })
        .child(crate::blockexport::toolbar(
            &id,
            Arc::<str>::from(&*text),
            stem,
            formats,
            Some(crate::focus::Block::code(&lang, &text)),
            window,
            cx,
        ))
        .into_any_element()
}

/// Under a table: Copy (as Markdown) and Export ▾ (CSV, Markdown, Excel).
fn table_tools(table: &TableData, window: &mut Window, cx: &mut App) -> gpui_kit::AnyElement {
    let id = format!("table-{}", key("t", &table.markdown));
    let (stem, formats) =
        crate::blockexport::table_formats(&table.headers, &table.rows, &table.markdown);
    div()
        .w_full()
        .flex()
        .justify_end()
        .child(crate::blockexport::toolbar(
            &id,
            Arc::from(table.markdown.as_str()),
            stem,
            formats,
            Some(crate::focus::Block::table(
                &table.headers,
                &table.rows,
                &table.markdown,
            )),
            window,
            cx,
        ))
        .into_any_element()
}

// ---------------------------------------------------------------- caches

/// A drawing in both themes: SVG for a dark and a light page, and its
/// size in the SVG's own pixels.
#[derive(Clone, Debug)]
pub struct Art {
    pub dark: Arc<Image>,
    pub light: Arc<Image>,
    pub width: f32,
    pub height: f32,
    /// For math: the baseline's distance from the top, in the same units.
    pub baseline: f32,
}

impl Art {
    fn image(&self, dark: bool) -> Arc<Image> {
        if dark {
            self.dark.clone()
        } else {
            self.light.clone()
        }
    }
}

/// Results by source, so the same formula or diagram is drawn once.
/// Failures are kept too (`None`). Capped; the oldest half goes first.
struct Cache {
    map: HashMap<u64, (u64, Option<Art>)>,
    tick: u64,
}

const CACHE_MAX: usize = 512;

fn cache() -> &'static Mutex<Cache> {
    static C: OnceLock<Mutex<Cache>> = OnceLock::new();
    C.get_or_init(|| {
        Mutex::new(Cache {
            map: HashMap::new(),
            tick: 0,
        })
    })
}

fn key(kind: &str, source: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    kind.hash(&mut h);
    source.hash(&mut h);
    h.finish()
}

fn cached(kind: &str, source: &str, make: impl FnOnce() -> Option<Art>) -> Option<Art> {
    let k = key(kind, source);
    if let Ok(mut c) = cache().lock() {
        c.tick += 1;
        let tick = c.tick;
        if let Some(hit) = c.map.get_mut(&k) {
            hit.0 = tick;
            return hit.1.clone();
        }
    }
    let art = make();
    if let Ok(mut c) = cache().lock() {
        if c.map.len() >= CACHE_MAX {
            let mut ticks: Vec<u64> = c.map.values().map(|v| v.0).collect();
            ticks.sort_unstable();
            let cut = ticks[ticks.len() / 2];
            c.map.retain(|_, v| v.0 > cut);
        }
        let tick = c.tick;
        c.map.insert(k, (tick, art.clone()));
    }
    art
}

fn svg_image(svg: String) -> Arc<Image> {
    Arc::new(Image::from_bytes(ImageFormat::Svg, svg.into_bytes()))
}

/// The theme's text colours, as the palette has them (`#EDEDED` on dark,
/// `#141414` on light).
fn ink(dark: bool) -> (f32, f32, f32) {
    let c: gpui_kit::Rgba = Palette::get(dark).text.into();
    (c.r, c.g, c.b)
}

fn is_dark(cx: &App) -> bool {
    gpui_kit::component::Theme::global(cx).is_dark()
}

// ------------------------------------------------------------------ math

/// SVG units per em that formulas are drawn at, close to the size they
/// are shown (#73): GPUI rasterises an SVG image at twice its own size, and
/// the GPU's downscale of a much larger raster dropped thin strokes (one
/// bar of an `=` vanished at 40). 16 keeps the downscale near 2x, which
/// averages instead of skipping pixels. Exports draw larger
/// (`blockexport::MATH_PNG_SCALE`).
const MATH_EM: f64 = 16.0;
const MATH_PAD: f64 = 2.0;

/// Whether `$…$` holds a formula rather than prose between two dollar
/// amounts: no space just inside either dollar sign (the Pandoc rule), not
/// empty, and not a bare number ("$5$" is a price written oddly, but "$x$"
/// is math).
pub fn looks_like_math(inner: &str) -> bool {
    let t = inner.trim();
    !t.is_empty()
        && !inner.starts_with(char::is_whitespace)
        && !inner.ends_with(char::is_whitespace)
        && !t
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.' || c == ',')
}

/// Typesets `latex` (display or inline style) for both themes, or `None`
/// when RaTeX cannot read it.
pub fn typeset(latex: &str, display: bool) -> Option<Art> {
    let kind = if display { "math-d" } else { "math-i" };
    cached(kind, latex, || {
        use ratex_layout::{LayoutOptions, layout, to_display_list, validate_layout};
        use ratex_types::color::Color;
        use ratex_types::math_style::MathStyle;
        // `align` and friends as RaTeX lays them out (`crate::mathsrc`).
        // Unknown commands (`\\softmax`) are read as operators.
        let ast = crate::mathsrc::ratex_candidates(latex)
            .iter()
            .find_map(|c| {
                crate::mathsrc::lenient(c, |s| {
                    ratex_parser::parser::parse(s).map_err(|e| e.to_string())
                })
            })?;
        let style = if display {
            MathStyle::Display
        } else {
            MathStyle::Text
        };
        let draw = |dark: bool| -> Option<(String, f64, f64, f64)> {
            let (r, g, b) = ink(dark);
            let opts = LayoutOptions::default()
                .with_style(style)
                .with_color(Color::rgb(r, g, b));
            let lbox = layout(&ast, &opts);
            validate_layout(&lbox).ok()?;
            let list = to_display_list(&lbox);
            let svg = ratex_svg::render_to_svg(
                &list,
                &ratex_svg::SvgOptions {
                    font_size: MATH_EM,
                    padding: MATH_PAD,
                    stroke_width: 1.0,
                    embed_glyphs: true,
                    font_dir: String::new(),
                },
            );
            Some((svg, list.width, list.height, list.depth))
        };
        let (dark_svg, w, h, d) = draw(true)?;
        let (light_svg, ..) = draw(false)?;
        if w <= 0.0 || h + d <= 0.0 {
            return None;
        }
        Some(Art {
            dark: svg_image(px_size(&dark_svg)),
            light: svg_image(px_size(&light_svg)),
            width: (w * MATH_EM + 2.0 * MATH_PAD) as f32,
            height: ((h + d) * MATH_EM + 2.0 * MATH_PAD) as f32,
            baseline: (h * MATH_EM + MATH_PAD) as f32,
        })
    })
}

/// RaTeX sizes its SVG in points; GPUI shows an SVG at its own size, so
/// the size is restated in pixels (the view box is unchanged).
fn px_size(svg: &str) -> String {
    svg.replacen("pt\" height=\"", "\" height=\"", 1)
        .replacen("pt\">", "\">", 1)
}

/// What a formula node carries: its source and, when it could be read,
/// its drawing.
#[derive(Clone, Debug)]
struct Formula {
    latex: String,
    art: Option<Art>,
}

/// `$…$` inside a paragraph.
#[derive(Debug)]
struct InlineMath;

impl MarkdownPlugin for InlineMath {
    fn name(&self) -> &str {
        "tern-math-inline"
    }

    fn parse(&self, node: &mdast::Node, cx: &MarkdownParseContext<'_>) -> Option<MarkdownNode> {
        let mdast::Node::InlineMath(m) = node else {
            return None;
        };
        // `$$…$$` written inside a line is display-sized but stays inline.
        let source = cx.node_source(node).unwrap_or_default();
        if !source.starts_with("$$") && !looks_like_math(&m.value) {
            return None;
        }
        let art = typeset(m.value.trim(), false);
        Some(
            MarkdownNode::new(
                self.name().to_owned(),
                Formula {
                    latex: m.value.clone(),
                    art,
                },
            )
            .text(source.to_owned())
            .accessibility_label(m.value.clone()),
        )
    }

    fn render_inline(
        &self,
        node: &MarkdownNode,
        context: &InlineRenderContext,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<InlineElement> {
        let f = node.data::<Formula>()?;
        let font = f32::from(context.font_size());
        match &f.art {
            Some(art) => {
                let scale = font / MATH_EM as f32;
                let (w, h) = (art.width * scale, art.height * scale);
                Some(
                    InlineElement::new(
                        img(art.image(is_dark(cx)))
                            .w(px(w))
                            .h(px(h))
                            .object_fit(ObjectFit::Fill)
                            .flex_none(),
                    )
                    .with_baseline(px(art.baseline * scale)),
                )
            }
            // Not readable as math: the source, in the code font.
            None => Some(InlineElement::new(
                div()
                    .font_family(crate::assets::MONO_FONT)
                    .text_size(px(font * 0.92))
                    .child(SharedString::from(format!("${}$", f.latex))),
            )),
        }
    }
}

/// What a display formula node carries.
#[derive(Clone, Debug)]
struct DisplayFormula {
    latex: String,
    art: Option<Art>,
    /// Its closing `$$` has come (a formula still streaming has none).
    closed: bool,
}

/// `$$…$$` (and `\[…\]`, `\begin{align}…`, rewritten by
/// `crate::mathsrc`) on lines of its own: a block with a Math header,
/// Rendered | LaTeX, Copy and Export.
#[derive(Debug)]
struct DisplayMath;

impl MarkdownPlugin for DisplayMath {
    fn is_block(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "tern-math-display"
    }

    fn parse(&self, node: &mdast::Node, cx: &MarkdownParseContext<'_>) -> Option<MarkdownNode> {
        let mdast::Node::Math(m) = node else {
            return None;
        };
        let closed = crate::mathsrc::math_closed(cx.node_source(node).unwrap_or_default());
        let art = if closed {
            typeset(m.value.trim(), true)
        } else {
            None
        };
        if closed && art.is_none() && !m.value.trim().is_empty() {
            crate::crashlog::log_line(&format!(
                "math: RaTeX could not read a display formula: {}",
                crate::crashlog::tail(m.value.trim(), 300)
            ));
        }
        Some(
            MarkdownNode::new(
                self.name().to_owned(),
                DisplayFormula {
                    latex: m.value.clone(),
                    art,
                    closed,
                },
            )
            .text(m.value.clone())
            .markdown(format!("$$\n{}\n$$", m.value))
            .accessibility_label(m.value.clone()),
        )
    }

    fn render(&self, node: &MarkdownNode, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let dark = is_dark(cx);
        let p = Palette::get(dark);
        let f = node.data::<DisplayFormula>().cloned();
        let font = f32::from(window.text_style().font_size.to_pixels(window.rem_size()));
        let latex = f
            .as_ref()
            .map(|f| f.latex.trim().to_owned())
            .unwrap_or_default();
        let art = f.as_ref().and_then(|f| f.art.clone());
        let closed = f.as_ref().is_some_and(|f| f.closed);
        let id = format!("math-{}", key("d", &latex));
        let source_shown =
            window.use_keyed_state(SharedString::from(format!("{id}-src")), cx, |_, _| false);
        let show_source = art.is_none() || *source_shown.read(cx);
        let svg = art
            .as_ref()
            .map(|a| String::from_utf8_lossy(a.light.bytes()).into_owned());
        let (stem, formats) = crate::blockexport::math_formats(&latex, svg.as_deref());
        let head = block_head(
            "Math",
            art.as_ref().map(|_| {
                crate::blockexport::segmented(
                    &id,
                    ("Rendered", "LaTeX"),
                    show_source,
                    source_shown.clone(),
                    cx,
                )
            }),
            crate::blockexport::toolbar(
                &id,
                Arc::from(latex.as_str()),
                stem,
                formats,
                (!latex.is_empty()).then(|| crate::focus::Block::math(&latex)),
                window,
                cx,
            ),
            p,
        );
        let body = if show_source {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(source_text(&latex, p))
                .when(closed && art.is_none(), |d| {
                    d.child(
                        div()
                            .text_xs()
                            .text_color(p.faint)
                            .child("Couldn't render this formula; showing its LaTeX."),
                    )
                })
                .into_any_element()
        } else {
            let art = art.clone().expect("rendered mode has art");
            let scale = font * 1.1 / MATH_EM as f32;
            div()
                .id(SharedString::from(format!("{id}-scroll")))
                .w_full()
                .overflow_x_scroll()
                .flex()
                .justify_center()
                .child(
                    img(art.image(dark))
                        .w(px(art.width * scale))
                        .h(px(art.height * scale))
                        .object_fit(ObjectFit::Fill)
                        .flex_none(),
                )
                .into_any_element()
        };
        block_card(p)
            .child(head)
            .child(div().px_3().pb_2().child(body))
    }
}

/// A rich block's frame, like the text view's code blocks.
fn block_card(p: Palette) -> gpui_kit::Div {
    div()
        .w_full()
        .my_2()
        .flex()
        .flex_col()
        .rounded_md()
        .border_1()
        .border_color(p.border)
        .bg(p.raised)
}

/// A block's header: its label, an optional view toggle, its toolbar.
fn block_head(
    label: &'static str,
    toggle: Option<gpui_kit::AnyElement>,
    tools: gpui_kit::AnyElement,
    p: Palette,
) -> gpui_kit::Div {
    div()
        .w_full()
        .flex()
        .items_start()
        .gap_2()
        .pl_3()
        .pr_1()
        .pt_1()
        .pb_1()
        .child(div().pt(px(3.)).text_xs().text_color(p.faint).child(label))
        .children(toggle.map(|t| div().pt(px(1.)).child(t)))
        .child(div().flex_1())
        .child(tools)
}

/// Source text in the code font, kept as written (selectable by copying).
fn source_text(source: &str, p: Palette) -> gpui_kit::Div {
    div()
        .w_full()
        .font_family(crate::assets::MONO_FONT)
        .text_sm()
        .text_color(p.text)
        .whitespace_normal()
        .children(
            source
                .lines()
                .map(|l| div().min_h(px(16.)).child(SharedString::from(l.to_owned())))
                .collect::<Vec<_>>(),
        )
}

// --------------------------------------------------------------- mermaid

/// Widest a diagram is shown, in pixels; wider ones are scaled down.
const DIAGRAM_MAX_W: f32 = 720.0;

/// Whether a fenced code block's source ends with its closing fence (a
/// block still streaming has none yet).
pub fn fence_closed(source: &str) -> bool {
    let t = source.trim_end();
    let mut lines = t.lines();
    let first = lines.next().unwrap_or_default().trim_start();
    let fence: String = first
        .chars()
        .take_while(|c| *c == '`' || *c == '~')
        .collect();
    if fence.len() < 3 {
        return false;
    }
    let last = t.lines().last().unwrap_or_default().trim();
    t.lines().count() >= 2
        && last.len() >= fence.len()
        && last
            .chars()
            .all(|c| c == fence.chars().next().unwrap_or('`'))
}

/// Loads the SVG renderer's fonts in the background at start-up. The
/// first SVG with text otherwise scans the system's fonts while its image
/// loads, and on Windows the first diagram stayed blank for 3–4 s.
pub fn warm_up_svg_text(cx: &App) {
    const SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><text x="0" y="6" font-family="IBM Plex Sans" font-size="6">x</text></svg>"#;
    let renderer = cx.svg_renderer();
    cx.background_executor()
        .spawn(async move {
            let _ = renderer.render_single_frame(SVG, 1.0);
        })
        .detach();
}

/// Draws Mermaid `source` for both themes, or `None` when merman cannot
/// read it (even after [`crate::mermaidsrc::tolerant`]'s rewrite).
#[cfg(test)]
pub fn draw_mermaid(source: &str) -> Option<Art> {
    draw_mermaid_or_error(source).ok()
}

/// What merman said about the diagrams it could not read, by source.
fn mermaid_errors() -> &'static Mutex<HashMap<u64, String>> {
    static E: OnceLock<Mutex<HashMap<u64, String>>> = OnceLock::new();
    E.get_or_init(|| Mutex::new(HashMap::new()))
}

/// [`draw_mermaid`], or merman's one-line reason. As written first; when
/// merman refuses it, as [`crate::mermaidsrc::tolerant`] rewrites it
/// (labels quoted, comments dropped). Both errors go to `tern.log`.
pub fn draw_mermaid_or_error(source: &str) -> Result<Art, String> {
    let art = cached("mermaid", source, || {
        let first = match draw_both(source) {
            Ok(art) => return Some(art),
            Err(e) => e,
        };
        crate::crashlog::log_line(&format!(
            "mermaid: merman could not read a diagram: {first}"
        ));
        let error = match crate::mermaidsrc::tolerant(source) {
            Some(fixed) => match draw_both(&fixed) {
                Ok(art) => {
                    crate::crashlog::log_line("mermaid: drawn after quoting its labels");
                    return Some(art);
                }
                Err(e) => {
                    crate::crashlog::log_line(&format!(
                        "mermaid: still unreadable after quoting labels: {e}"
                    ));
                    e
                }
            },
            None => first,
        };
        if let Ok(mut m) = mermaid_errors().lock() {
            if m.len() > CACHE_MAX {
                m.clear();
            }
            m.insert(key("mermaid", source), error);
        }
        None
    });
    art.ok_or_else(|| {
        mermaid_errors()
            .lock()
            .ok()
            .and_then(|m| m.get(&key("mermaid", source)).cloned())
            .unwrap_or_else(|| "Mermaid could not read this diagram".to_owned())
    })
}

fn draw_both(source: &str) -> Result<Art, String> {
    let dark = mermaid_svg(source, true)?;
    let light = mermaid_svg(source, false)?;
    // Drawn at the size it is shown: the image is rasterised at its own
    // size and the GPU's downscale of a wider raster drops thin lines.
    let (w, h) = shown_size(svg_size(&dark).ok_or("the drawing has no size")?);
    Ok(Art {
        dark: svg_image(dark),
        light: svg_image(light),
        width: w,
        height: h,
        baseline: h,
    })
}

fn mermaid_svg(source: &str, dark: bool) -> Result<String, String> {
    use merman::{
        Engine, MermaidConfig, OperationControl, RenderOutput, RenderRequest, Renderer, SvgRequest,
    };
    let config = MermaidConfig::from_value(serde_json::json!({
        "theme": if dark { "dark" } else { "neutral" },
        // The classic look: merman's default "neo" look strokes nodes with
        // a gradient that fades to the page, so their borders vanish.
        "look": "classic",
        // The app's own font, so the SVG renderer finds it (a missing
        // family fell back to an italic face).
        "fontFamily": format!("{}, sans-serif", crate::assets::UI_FONT),
    }));
    let renderer = Renderer::new().with_engine(Engine::new().with_site_config(config));
    let request = SvgRequest {
        pipeline: Some(merman::svg::SvgPipeline::resvg_safe()),
        ..Default::default()
    };
    let out = renderer
        .render(RenderRequest::svg(source, OperationControl::new(), request))
        .map_err(|e| one_line(&e.to_string()))?;
    let RenderOutput::Svg(Some(svg)) = out else {
        return Err("Mermaid drew nothing for this diagram".to_owned());
    };
    // The UI tour keeps each theme's drawing beside its scene log, to be
    // looked at outside the app.
    if crate::view::tour::enabled()
        && let Some(log) = std::env::var_os("TERN_TOUR_LOG")
        && let Some(dir) = std::path::Path::new(&log).parent()
    {
        let theme = if dark { "dark" } else { "light" };
        let _ = std::fs::write(dir.join(format!("mermaid-{theme}.svg")), svg.svg());
    }
    Ok(sized(&solid_edges(svg.svg()), DIAGRAM_MAX_W))
}

/// The first line of an error, at most 160 characters.
fn one_line(e: &str) -> String {
    let l = e
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("unknown error");
    match l.char_indices().nth(160) {
        Some((i, _)) => format!("{}…", &l[..i]),
        None => l.to_owned(),
    }
}

/// The size a diagram of view-box size `(w, h)` is shown at: as drawn,
/// or scaled down to [`DIAGRAM_MAX_W`].
fn shown_size((w, h): (f32, f32)) -> (f32, f32) {
    let shown = w.min(DIAGRAM_MAX_W);
    (shown, h * shown / w)
}

/// Drops the `stroke-dasharray:0 0 L G` Mermaid puts on edges for its
/// draw-in animation: the SVG renderer draws such an edge as nothing,
/// leaving only its arrowhead.
pub fn solid_edges(svg: &str) -> String {
    const DASH: &str = "stroke-dasharray:0 0 ";
    let mut out = String::with_capacity(svg.len());
    let mut rest = svg;
    while let Some(at) = rest.find(DASH) {
        out.push_str(&rest[..at]);
        let after = &rest[at..];
        let end = after.find([';', '"']).unwrap_or(after.len());
        // The `;` goes with the declaration; a closing quote stays.
        let end = if after[end..].starts_with(';') {
            end + 1
        } else {
            end
        };
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

/// Gives the root `<svg>` a pixel width and height from its view box
/// (Mermaid writes `width="100%"`, which an image cannot use), at most
/// `max_w` wide. A diagram scaled down gets its edge and node strokes
/// thickened by the same factor, so a 1 px line stays about 1 px on
/// screen instead of fading out.
pub fn sized(svg: &str, max_w: f32) -> String {
    let Some(start) = svg.find("<svg") else {
        return svg.to_owned();
    };
    let Some(end) = svg[start..].find('>').map(|e| start + e) else {
        return svg.to_owned();
    };
    let tag = &svg[start..end];
    let Some((vw, vh)) = view_box(tag) else {
        return svg.to_owned();
    };
    let w = vw.min(max_w);
    let h = vh * w / vw;
    let mut new_tag = String::with_capacity(tag.len() + 32);
    let mut rest = tag;
    // Drop the root's own width, height and style (max-width), keep the rest.
    while let Some(i) = find_attr(rest) {
        new_tag.push_str(&rest[..i.0]);
        rest = &rest[i.1..];
    }
    new_tag.push_str(rest);
    let new_tag = format!("{new_tag} width=\"{w}\" height=\"{h}\"");
    let out = format!("{}{}{}", &svg[..start], new_tag, &svg[end..]);
    if w >= vw {
        return out;
    }
    thicken(&out, (vw / w).min(3.0))
}

/// Appends stroke widths `k` times Mermaid's 1 px to the diagram's style
/// sheet (same selectors, later, so they win).
fn thicken(svg: &str, k: f32) -> String {
    let Some(at) = svg.find("</style>") else {
        return svg.to_owned();
    };
    let id = svg_id(svg).unwrap_or_default();
    let scope = if id.is_empty() {
        String::new()
    } else {
        format!("#{id} ")
    };
    let rule = format!(
        "{scope}.edge-thickness-normal{{stroke-width:{k:.2}px;}}\
         {scope}.node rect,{scope}.node circle,{scope}.node ellipse,\
         {scope}.node polygon,{scope}.node path{{stroke-width:{k:.2}px;}}"
    );
    format!("{}{}{}", &svg[..at], rule, &svg[at..])
}

/// The root `<svg>`'s `id`, which Mermaid's style sheet scopes rules to.
fn svg_id(svg: &str) -> Option<&str> {
    let start = svg.find("<svg")?;
    let end = svg[start..].find('>')? + start;
    let tag = &svg[start..end];
    let i = tag.find(" id=\"")? + " id=\"".len();
    let len = tag[i..].find('"')?;
    Some(&tag[i..i + len])
}

/// The next ` width="…"`, ` height="…"` or ` style="…"` in a tag, as the
/// byte range to drop.
fn find_attr(tag: &str) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize)> = None;
    for name in [" width=\"", " height=\"", " style=\""] {
        if let Some(i) = tag.find(name) {
            let vstart = i + name.len();
            if let Some(len) = tag[vstart..].find('"') {
                let r = (i, vstart + len + 1);
                if best.is_none_or(|b| r.0 < b.0) {
                    best = Some(r);
                }
            }
        }
    }
    best
}

fn view_box(tag: &str) -> Option<(f32, f32)> {
    let i = tag.find("viewBox=\"")? + "viewBox=\"".len();
    let len = tag[i..].find('"')?;
    let nums: Vec<f32> = tag[i..i + len]
        .split([' ', ','])
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect();
    match nums.as_slice() {
        [_, _, w, h] if *w > 0.0 && *h > 0.0 => Some((*w, *h)),
        _ => None,
    }
}

pub(crate) fn svg_size(svg: &str) -> Option<(f32, f32)> {
    let start = svg.find("<svg")?;
    let end = svg[start..].find('>')? + start;
    view_box(&svg[start..end])
}

/// A finished ```` ```mermaid ```` block: its source and its drawing, or
/// why it could not be drawn.
#[derive(Clone, Debug)]
struct Diagram {
    source: String,
    art: Result<Art, String>,
}

/// ```` ```mermaid ```` blocks: a block with a Mermaid header,
/// Diagram | Code, Copy and Export. While the block streams it stays the
/// text view's code block.
#[derive(Debug)]
struct Mermaid;

impl MarkdownPlugin for Mermaid {
    fn is_block(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "tern-mermaid"
    }

    fn parse(&self, node: &mdast::Node, cx: &MarkdownParseContext<'_>) -> Option<MarkdownNode> {
        let mdast::Node::Code(code) = node else {
            return None;
        };
        if !code
            .lang
            .as_deref()
            .is_some_and(|l| l.eq_ignore_ascii_case("mermaid"))
        {
            return None;
        }
        // Still streaming: a code block until its fence closes.
        if !fence_closed(cx.node_source(node).unwrap_or_default()) {
            return None;
        }
        let art = draw_mermaid_or_error(&code.value);
        Some(
            MarkdownNode::new(
                self.name().to_owned(),
                Diagram {
                    source: code.value.clone(),
                    art,
                },
            )
            .text(code.value.clone())
            .markdown(format!("```mermaid\n{}\n```", code.value))
            .accessibility_label("Mermaid diagram"),
        )
    }

    fn render(&self, node: &MarkdownNode, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let dark = is_dark(cx);
        let p = Palette::get(dark);
        let Some(d) = node.data::<Diagram>().cloned() else {
            return div().into_any_element();
        };
        let id = format!("mermaid-{}", key("m", &d.source));
        let code_shown =
            window.use_keyed_state(SharedString::from(format!("{id}-code")), cx, |_, _| false);
        let art = d.art.as_ref().ok().cloned();
        let show_code = art.is_none() || *code_shown.read(cx);
        let svg = art
            .as_ref()
            .map(|a| String::from_utf8_lossy(a.light.bytes()).into_owned());
        let (stem, formats) = crate::blockexport::mermaid_formats(&d.source, svg.as_deref());
        let head = block_head(
            "Mermaid",
            art.as_ref().map(|_| {
                crate::blockexport::segmented(
                    &id,
                    ("Diagram", "Code"),
                    show_code,
                    code_shown.clone(),
                    cx,
                )
            }),
            crate::blockexport::toolbar(
                &id,
                Arc::from(d.source.as_str()),
                stem,
                formats,
                Some(crate::focus::Block::mermaid(&d.source)),
                window,
                cx,
            ),
            p,
        );
        let body = if show_code {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(source_text(&d.source, p))
                .children(d.art.as_ref().err().map(|e| {
                    div()
                        .text_xs()
                        .text_color(p.faint)
                        .child(SharedString::from(format!(
                            "Couldn't draw this diagram: {e}"
                        )))
                }))
                .into_any_element()
        } else {
            let art = art.clone().expect("diagram mode has art");
            let w = art.width.min(DIAGRAM_MAX_W);
            let h = art.height * (w / art.width);
            div()
                .id(SharedString::from(format!("{id}-scroll")))
                .w_full()
                .overflow_x_scroll()
                .flex()
                .justify_center()
                .child(
                    // Never wider than the answer column (a narrow window
                    // shrinks it; Contain keeps its shape).
                    img(art.image(dark))
                        .w(px(w))
                        .max_w_full()
                        .h(px(h))
                        .object_fit(ObjectFit::Contain)
                        .flex_none(),
                )
                .into_any_element()
        };
        block_card(p)
            .child(head)
            .child(div().px_3().pb_2().child(body))
            .into_any_element()
    }
}

// ------------------------------------------------------------- citations

/// The title a citation link carries, so the chip plugin knows it.
pub const CITE_TITLE: &str = "tern-cite";

/// What a chip's click calls with its page's URL.
type CiteOpener = Rc<dyn Fn(&str, &mut Window, &mut App)>;

thread_local! {
    /// Opens a chip's page: the main window sets it to its browser.
    static OPEN: RefCell<Option<CiteOpener>> =
        const { RefCell::new(None) };
}

/// Sets what a chip's click does (open the page in Tern's browser).
pub fn set_cite_opener(f: impl Fn(&str, &mut Window, &mut App) + 'static) {
    OPEN.with(|o| *o.borrow_mut() = Some(Rc::new(f)));
}

fn open_cite(url: &str, window: &mut Window, cx: &mut App) {
    let f = OPEN.with(|o| o.borrow().clone());
    match f {
        Some(f) => f(url, window, cx),
        None => cx.open_url(url),
    }
}

/// `[n]` markers in `text` that cite a page in `pages` become links the
/// chip plugin draws: `[2](https://page "tern-cite")`. Code spans, fenced
/// blocks, links (`[2](…)`), reference definitions (`[2]: …`) and
/// footnotes are left alone. `pages` maps a citation number to its URL.
pub fn chips(text: &str, pages: &HashMap<usize, String>) -> String {
    if pages.is_empty() || !text.contains('[') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len() + 64);
    let mut fence: Option<String> = None;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let marker: String = trimmed
            .chars()
            .take_while(|c| *c == '`' || *c == '~')
            .collect();
        if marker.len() >= 3 {
            match &fence {
                Some(f) if marker.starts_with(f.as_str()) => fence = None,
                None => fence = Some(marker),
                _ => {}
            }
            out.push_str(line);
            continue;
        }
        if fence.is_some() {
            out.push_str(line);
            continue;
        }
        out.push_str(&chips_in_line(line, pages));
    }
    out
}

fn chips_in_line(line: &str, pages: &HashMap<usize, String>) -> String {
    let bytes = line.as_bytes();
    let mut out = String::with_capacity(line.len() + 32);
    let mut i = 0;
    let mut in_code = false;
    while i < line.len() {
        let c = bytes[i];
        if c == b'`' {
            in_code = !in_code;
            out.push('`');
            i += 1;
            continue;
        }
        if c == b'[' && !in_code {
            // `[12]`: not an image (`![`), an escape, a link (`[1](…)`) or a
            // reference definition (`[1]: …`). `[2][3]` is two citations.
            let prev = if i == 0 { None } else { Some(bytes[i - 1]) };
            let digits = line[i + 1..].bytes().take_while(u8::is_ascii_digit).count();
            let close = i + 1 + digits;
            let after = bytes.get(close + 1).copied();
            if digits > 0
                && digits <= 3
                && bytes.get(close) == Some(&b']')
                && !matches!(prev, Some(b'!' | b'\\'))
                && !matches!(after, Some(b'(' | b':'))
                && let Ok(n) = line[i + 1..close].parse::<usize>()
                && let Some(url) = pages.get(&n)
                && is_web(url)
            {
                out.push_str(&format!(
                    "[{n}]({} \"{CITE_TITLE}\")",
                    url.replace(' ', "%20")
                        .replace('(', "%28")
                        .replace(')', "%29")
                ));
                i = close + 1;
                continue;
            }
        }
        let ch = line[i..].chars().next().unwrap_or(' ');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn is_web(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

/// What a chip carries.
#[derive(Clone, Debug)]
struct CiteData {
    n: String,
    url: String,
}

/// Draws `[n](url "tern-cite")` links as chips.
#[derive(Debug)]
struct Cite;

impl MarkdownPlugin for Cite {
    fn name(&self) -> &str {
        "tern-cite"
    }

    fn parse(&self, node: &mdast::Node, _cx: &MarkdownParseContext<'_>) -> Option<MarkdownNode> {
        let mdast::Node::Link(link) = node else {
            return None;
        };
        if link.title.as_deref() != Some(CITE_TITLE) {
            return None;
        }
        let n: String = link
            .children
            .iter()
            .filter_map(|c| match c {
                mdast::Node::Text(t) => Some(t.value.clone()),
                _ => None,
            })
            .collect();
        Some(
            MarkdownNode::new(
                self.name().to_owned(),
                CiteData {
                    n: n.clone(),
                    url: link.url.clone(),
                },
            )
            .text(format!("[{n}]"))
            .markdown(format!("[{n}]"))
            .accessibility_label(format!("Source {n}")),
        )
    }

    fn render_inline(
        &self,
        node: &MarkdownNode,
        context: &InlineRenderContext,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<InlineElement> {
        let c = node.data::<CiteData>()?.clone();
        let p = Palette::get(is_dark(cx));
        let font = f32::from(context.font_size());
        let url = c.url.clone();
        let chip = div()
            .id(SharedString::from(format!(
                "cite-{}-{}",
                c.n,
                key("c", &c.url)
            )))
            .mx(px(1.))
            .px(px(5.))
            .rounded(px(4.))
            .bg(p.hover)
            .border_1()
            .border_color(p.border)
            .text_color(p.muted)
            .text_size(px((font * 0.72).max(10.)))
            .line_height(px(font * 1.15))
            .cursor_pointer()
            .hover(move |d| d.text_color(p.text).border_color(p.accent))
            .on_click(move |_, window, cx| open_cite(&url, window, cx))
            .child(SharedString::from(c.n.clone()));
        Some(InlineElement::new(chip))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dollar_amounts_are_not_math() {
        assert!(looks_like_math("x^2"));
        assert!(looks_like_math("\\frac{a}{b}"));
        assert!(!looks_like_math("5 and "));
        assert!(!looks_like_math(" x"));
        assert!(!looks_like_math("10"));
        assert!(!looks_like_math("1,000.50"));
        assert!(!looks_like_math(""));
    }

    #[test]
    fn formulas_are_typeset_for_both_themes_and_bad_ones_are_not() {
        let a = typeset("\\frac{a}{b} + \\sqrt{x^2}", true).expect("typeset");
        assert!(a.width > 0.0 && a.height > 0.0);
        assert!(a.baseline > 0.0 && a.baseline < a.height);
        let dark = String::from_utf8(a.dark.bytes().to_vec()).unwrap();
        let light = String::from_utf8(a.light.bytes().to_vec()).unwrap();
        assert!(dark.starts_with("<svg") && dark.contains("<path"), "{dark}");
        assert_ne!(dark, light, "each theme has its own ink");
        assert!(!dark.contains("pt\""), "sized in pixels");
        // Kept: the same formula comes back from the cache.
        let again = typeset("\\frac{a}{b} + \\sqrt{x^2}", true).unwrap();
        assert!(Arc::ptr_eq(&a.dark, &again.dark));
        assert!(typeset("\\frac{a}{", true).is_none());
    }

    #[test]
    fn mermaid_draws_closed_blocks_only() {
        assert!(fence_closed("```mermaid\ngraph TD\nA-->B\n```"));
        assert!(fence_closed("```mermaid\ngraph TD\nA-->B\n```\n"));
        assert!(!fence_closed("```mermaid\ngraph TD\nA-->B"));
        assert!(!fence_closed("```mermaid\n```x"));
        assert!(fence_closed("~~~mermaid\nA\n~~~"));
        let art = draw_mermaid("flowchart TD\n  Q[Question] --> S[Search]\n  S --> A[Answer]")
            .expect("drawn");
        assert!(art.width > 10.0 && art.height > 10.0);
        let svg = String::from_utf8(art.light.bytes().to_vec()).unwrap();
        assert!(!svg.contains("foreignObject"), "resvg-safe labels");
        assert!(draw_mermaid("this is not mermaid at all {{{").is_none());
    }

    #[test]
    fn the_models_bracket_formulas_and_align_are_typeset() {
        // The exact formulas from the answer that showed raw LaTeX.
        let text = crate::mathsrc::delimiters(crate::mathsrc::SAMPLE);
        assert!(
            typeset(
                "\\alpha = \\text{softmax}\\left(\\frac{Q K^T}{\\sqrt{d_k}}\\right) V",
                true
            )
            .is_some()
        );
        assert!(typeset("n \\times d_k", false).is_some());
        assert!(
            typeset(
                "\\begin{align*}\nQ &= X W^Q \\\\\nK &= X W^K \\\\\nV &= X W^V\n\\end{align*}",
                true
            )
            .is_some(),
            "align* lays out as aligned"
        );
        assert!(typeset("a &= b \\\\ c &= d", true).is_some(), "bare rows");
        assert!(text.contains("$$n \\times d_k$$"), "{text}");
    }

    #[test]
    fn the_models_mermaid_is_drawn_after_quoting_its_labels() {
        let art = draw_mermaid_or_error(crate::mermaidsrc::SAMPLE);
        assert!(art.is_ok(), "{:?}", art.err());
        let err = draw_mermaid_or_error("flowchart TD\n  A[[[ --> ").unwrap_err();
        assert!(!err.is_empty() && !err.contains('\n'), "{err}");
    }

    #[test]
    fn edges_lose_their_draw_in_dashes() {
        let svg = r#"<path style="stroke-dasharray:0 0 42 4;stroke-dashoffset:0" d="M0"/><path style="stroke-dasharray:0 0 7.5 4" d="M1"/><path style="stroke-dasharray:3" d="M2"/>"#;
        assert_eq!(
            solid_edges(svg),
            r#"<path style="stroke-dashoffset:0" d="M0"/><path style="" d="M1"/><path style="stroke-dasharray:3" d="M2"/>"#
        );
    }

    #[test]
    fn the_root_svg_gets_a_pixel_size() {
        let s = sized(
            r#"<svg id="m" width="100%" style="max-width: 300px;" viewBox="0 0 300 120" xmlns="http://www.w3.org/2000/svg"><style>#m .x{}</style><g/></svg>"#,
            720.0,
        );
        assert!(s.starts_with(r#"<svg id="m" viewBox="0 0 300 120""#), "{s}");
        assert!(s.contains(r#"width="300" height="120">"#), "{s}");
        assert!(!s.contains("100%") && !s.contains("max-width"));
        assert_eq!(svg_size(&s), Some((300.0, 120.0)));
        assert!(!s.contains("stroke-width"), "not scaled, not thickened");
    }

    #[test]
    fn a_wide_diagram_is_drawn_at_its_shown_size_with_thicker_lines() {
        let s = sized(
            r#"<svg id="m" width="100%" viewBox="0 0 1440 100"><style>#m .x{}</style><g/></svg>"#,
            720.0,
        );
        assert!(s.contains(r#"width="720" height="50">"#), "{s}");
        assert!(
            s.contains("#m .edge-thickness-normal{stroke-width:2.00px;}"),
            "{s}"
        );
        assert!(s.contains("#m .node rect,"), "{s}");
        assert!(s.find("#m .x{}").unwrap() < s.find("edge-thickness").unwrap());
        assert_eq!(shown_size((1440.0, 100.0)), (720.0, 50.0));
        assert_eq!(shown_size((300.0, 120.0)), (300.0, 120.0));
        // A real diagram: its art is the shown size, its strokes thickened.
        let art = draw_mermaid(
            "flowchart LR\n  A[Tokens] --> B[Embedding and position] --> C[Multi-head attention] --> D[Add and norm] --> E[Feed-forward] --> F[Add and norm]",
        )
        .expect("drawn");
        assert!(art.width <= DIAGRAM_MAX_W, "{}", art.width);
        let svg = String::from_utf8(art.dark.bytes().to_vec()).unwrap();
        assert!(
            svg.contains("edge-thickness-normal{stroke-width:"),
            "thickened"
        );
    }

    #[test]
    fn citations_become_chips_outside_code_and_links() {
        let mut pages = HashMap::new();
        pages.insert(1, "https://a.test/x".to_owned());
        pages.insert(2, "https://b.test/y (z)".to_owned());
        let text = "RAG retrieves first [1]. Agents loop [2][1].\n\n`[1]` in code, [1](https://keep.test) a link, [3] unknown.\n\n```\n[1] in a block\n```\n[1]: https://def.test\n";
        let out = chips(text, &pages);
        assert!(
            out.contains(r#"first [1](https://a.test/x "tern-cite")."#),
            "{out}"
        );
        assert!(
            out.contains(r#"loop [2](https://b.test/y%20%28z%29 "tern-cite")[1](https://a.test/x "tern-cite")."#),
            "{out}"
        );
        assert!(out.contains("`[1]` in code"), "{out}");
        assert!(out.contains("[1](https://keep.test) a link"), "{out}");
        assert!(out.contains("[3] unknown"), "{out}");
        assert!(out.contains("```\n[1] in a block\n```"), "{out}");
        assert!(out.contains("[1]: https://def.test"), "{out}");
        assert_eq!(chips("no citations", &pages), "no citations");
        assert_eq!(chips("[1]", &HashMap::new()), "[1]");
    }
}
