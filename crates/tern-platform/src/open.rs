//! Opening files and links with the user's default app.
//!
//! | OS | Opener |
//! | --- | --- |
//! | Windows | `explorer` |
//! | Linux | `xdg-open` (the desktop portal answers it under Flatpak and on most desktops) |
//! | macOS | `open` |
//!
//! The desktop app opens links through its window toolkit and hands it
//! [`file_url`] for local files, since a bare path is not a link on Linux
//! or macOS. The CLI uses [`open`].

use std::path::Path;
use std::process::Command;

use crate::{PlatformError, PlatformResult};

/// A `file://` link for a local path; anything that is already a link
/// (`https://`, `file://`, `mailto:`) is returned as it is.
pub fn file_url(target: &str) -> String {
    let looks_like_link = target.split_once(':').is_some_and(|(scheme, _)| {
        scheme.len() > 1
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    });
    if looks_like_link {
        return target.to_owned();
    }
    let path = Path::new(target);
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|d| d.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    url::Url::from_file_path(&abs)
        .map(String::from)
        .unwrap_or_else(|()| target.to_owned())
}

/// The command that opens `target` on this OS.
pub fn command(target: &str) -> Command {
    #[cfg(windows)]
    {
        let mut c = Command::new("explorer");
        c.arg(target);
        c
    }
    #[cfg(target_os = "macos")]
    {
        let mut c = Command::new("open");
        c.arg(target);
        c
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let mut c = Command::new("xdg-open");
        c.arg(target);
        c
    }
}

/// Opens a file or link with the default app, without waiting for it.
pub fn open(target: &str) -> PlatformResult<()> {
    command(target)
        .spawn()
        .map(|_| ())
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => PlatformError::Io(std::io::Error::new(
                e.kind(),
                format!(
                    "no opener found ({}); open {target} yourself",
                    command(target).get_program().to_string_lossy()
                ),
            )),
            _ => PlatformError::Io(e),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_stay_links_and_paths_become_file_links() {
        assert_eq!(file_url("https://tern.test/a"), "https://tern.test/a");
        assert_eq!(file_url("mailto:x@y.test"), "mailto:x@y.test");
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("turn 1.md");
        let u = file_url(&f.to_string_lossy());
        assert!(u.starts_with("file:///"), "{u}");
        assert!(u.ends_with("turn%201.md"), "{u}");
    }

    #[test]
    fn the_opener_fits_the_os() {
        let program = command("x").get_program().to_string_lossy().into_owned();
        assert!(["explorer", "open", "xdg-open"].contains(&program.as_str()));
    }
}
