//! The app's embedded assets: Tern's own (the mark) in front of the kit's
//! full Lucide catalog.

use std::borrow::Cow;

use gpui_kit::{AssetSource, SharedString};

/// The tern mark as one-colour SVG; the view tints and animates it.
pub const MARK: &str = "tern/mark.svg";
const MARK_SVG: &[u8] = include_bytes!("../assets/tern-mark.svg");

#[derive(Debug)]
pub struct TernAssets;

impl AssetSource for TernAssets {
    fn load(&self, path: &str) -> gpui_kit::Result<Option<Cow<'static, [u8]>>> {
        if path == MARK {
            return Ok(Some(Cow::Borrowed(MARK_SVG)));
        }
        gpui_kit::assets::AllAssets.load(path)
    }

    fn list(&self, path: &str) -> gpui_kit::Result<Vec<SharedString>> {
        let mut out = gpui_kit::assets::AllAssets.list(path)?;
        if MARK.starts_with(path) {
            out.push(MARK.into());
        }
        Ok(out)
    }
}
