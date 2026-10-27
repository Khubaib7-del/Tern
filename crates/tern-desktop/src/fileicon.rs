//! File-type logos (#72): the real logo of a file's kind (Word, Excel,
//! PDF, Python, …) beside its name on attachment cards, sent messages and
//! the @ menu, in place of the coloured text badges ("DOC", "PY").
//!
//! The logos are 64 px PNGs in `assets/file-icons/` and the table that
//! maps names to them is generated from `assets/file-icons/catalog.csv` by
//! `scripts/gen_file_icons.py`. Logos are trademarks of their owners and
//! are used only to say what kind a file is (see NOTICE).

use std::sync::{Arc, OnceLock};

use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Hsla, Image, ImageFormat, ObjectFit, div, img, px};

use crate::fileicon_table::{BY_EXT, BY_NAME, ICONS, UNKNOWN};

/// One of the logos.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileIcon(usize);

impl FileIcon {
    /// The logo for a file named `name` (a path is fine): a whole-name
    /// match first ("Dockerfile", ".gitignore"), then the longest suffix
    /// after a dot ("app.component.ts" before "ts", "x.tar.gz" → archive),
    /// without case; anything else is the blank page.
    pub fn of(name: &str) -> Self {
        let base = name
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(name)
            .trim()
            .to_lowercase();
        if let Ok(k) = BY_NAME.binary_search_by(|(n, _)| (*n).cmp(base.as_str())) {
            return Self(BY_NAME[k].1);
        }
        for (at, _) in base.match_indices('.') {
            let suffix = &base[at + 1..];
            if suffix.is_empty() {
                continue;
            }
            if let Ok(k) = BY_EXT.binary_search_by(|(e, _)| (*e).cmp(suffix)) {
                return Self(BY_EXT[k].1);
            }
        }
        Self(UNKNOWN)
    }

    /// The logo whose file is `assets/file-icons/<stem>.png`.
    pub fn named(stem: &str) -> Option<Self> {
        ICONS.iter().position(|(s, ..)| *s == stem).map(Self)
    }

    /// Whether this is the blank page (no known kind).
    pub fn is_unknown(self) -> bool {
        self.0 == UNKNOWN
    }

    /// Whether the logo is drawn in black or dark grey, so on a dark page
    /// it sits on a light tile.
    pub fn dark_ink(self) -> bool {
        ICONS[self.0].3
    }

    /// What kind of file the logo stands for ("Word document").
    #[cfg(test)]
    fn kind(self) -> &'static str {
        ICONS[self.0].1
    }

    fn image(self) -> Arc<Image> {
        static IMAGES: OnceLock<Vec<Arc<Image>>> = OnceLock::new();
        IMAGES.get_or_init(|| {
            ICONS
                .iter()
                .map(|(_, _, png, _)| Arc::new(Image::from_bytes(ImageFormat::Png, png.to_vec())))
                .collect()
        })[self.0]
            .clone()
    }

    #[cfg(test)]
    fn stem(self) -> &'static str {
        ICONS[self.0].0
    }
}

/// The light tile a dark logo sits on in the dark theme.
const TILE: u32 = 0xE6E5E0;

/// `icon` drawn `size` px square. On a dark page (`dark`) a logo drawn in
/// black gets a subtle light rounded tile behind it, so it stays legible.
pub fn logo(icon: FileIcon, size: f32, dark: bool) -> AnyElement {
    if dark && icon.dark_ink() {
        let pad = (size * 0.14).round().max(1.);
        div()
            .flex_none()
            .p(px(pad))
            .rounded(px(size * 0.24))
            .bg(Hsla::from(gpui_kit::rgb(TILE)))
            .child(
                img(icon.image())
                    .size(px(size - 2. * pad))
                    .object_fit(ObjectFit::Contain),
            )
            .into_any_element()
    } else {
        img(icon.image())
            .size(px(size))
            .object_fit(ObjectFit::Contain)
            .flex_none()
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stem(name: &str) -> &'static str {
        FileIcon::of(name).stem()
    }

    #[test]
    fn names_find_their_logo() {
        assert_eq!(stem("CA-3 Report.DOCX"), "word-document");
        assert_eq!(stem("marks.xlsx"), "excel-workbook");
        assert_eq!(stem("paper.pdf"), "pdf");
        assert_eq!(stem("train.py"), "python");
        assert_eq!(stem("main.CPP"), "cpp");
        assert_eq!(stem("photo.png"), "image-raster");
        assert_eq!(stem("data.zip"), "zip-archive");
        assert_eq!(stem("Pasted text"), "unknown");
        assert_eq!(stem("weird.qqqq"), "unknown");
        assert_eq!(stem("trailing."), "unknown");
        assert!(FileIcon::of("x.qqqq").is_unknown());
        // Paths and whole names.
        assert_eq!(stem("C:\\Users\\k\\proj\\Dockerfile"), "dockerfile");
        assert_eq!(stem("/home/k/proj/MAKEFILE"), "makefile");
        assert_eq!(stem("cmakelists.txt"), "cmake");
        assert_eq!(stem(".gitignore"), "git-files");
        assert_eq!(stem(".env"), "env-file");
        // Longest suffix first.
        assert_eq!(stem("backup.tar.gz"), "zip-archive");
        assert_eq!(stem("app.component.ts"), "angular-component");
        assert_eq!(stem("app.service.ts"), "typescript");
        assert_eq!(stem("notes.v2.md"), "markdown");
    }

    #[test]
    fn shared_extensions_take_the_chosen_kind() {
        assert_eq!(FileIcon::of("fit.m").kind(), "MATLAB");
        assert_eq!(FileIcon::of("alu.v").kind(), "Verilog");
        assert_eq!(FileIcon::of("old.bas").kind(), "Visual Basic");
        assert_eq!(FileIcon::of("air.nc").kind(), "HDF5 / NetCDF");
        assert_eq!(FileIcon::of("talk.key").kind(), "Apple Keynote");
        assert_eq!(FileIcon::of("family.pro").kind(), "Prolog");
        assert_eq!(FileIcon::of("thesis.cls").kind(), "LaTeX");
        assert_eq!(FileIcon::of("app.d").kind(), "D");
    }

    #[test]
    fn the_tables_are_sorted_and_every_logo_is_a_png() {
        assert!(BY_EXT.windows(2).all(|w| w[0].0 < w[1].0));
        assert!(BY_NAME.windows(2).all(|w| w[0].0 < w[1].0));
        for (s, _, png, _) in ICONS {
            assert!(png.starts_with(b"\x89PNG"), "{s}");
        }
        for s in [
            "rust", "yaml", "cobol", "solidity", "torrent", "markdown", "json",
        ] {
            assert!(FileIcon::named(s).unwrap().dark_ink(), "{s}");
        }
        assert!(!FileIcon::named("word-document").unwrap().dark_ink());
    }
}
