//! Showing a file in the system file manager, and opening a file with its
//! default app (#74).
//!
//! The toast's "Open folder" went through GPUI's `reveal_path`, which did
//! nothing on Windows. These call the platform's own tools directly:
//!
//! - Windows: `explorer.exe /select,"C:\path\to\file"` (the argument is
//!   passed raw, so a path with spaces keeps its quotes); a folder, or a
//!   file that is gone, opens its folder instead.
//! - macOS: `open -R path`, `open path`.
//! - Linux and the rest: `xdg-open` on the folder (file managers there
//!   have no common "select this file"), `xdg-open path` to open.
//!
//! A failure to start the tool is written to `tern.log` and returned.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Shows `path` in the file manager: its folder with the file selected
/// where the platform can, or the folder itself.
pub fn reveal(path: &Path) -> Result<(), String> {
    let r = reveal_inner(path);
    if let Err(e) = &r {
        crate::crashlog::log_line(&format!("reveal {}: {e}", path.display()));
    }
    r
}

/// Opens `path` (a file or a folder) with its default app.
pub fn open(path: &Path) -> Result<(), String> {
    let r = open_inner(path);
    if let Err(e) = &r {
        crate::crashlog::log_line(&format!("open {}: {e}", path.display()));
    }
    r
}

/// The folder to open when a file cannot be selected: the path itself
/// when it is a folder, else its parent (the nearest one that exists).
pub fn folder_of(path: &Path) -> PathBuf {
    if path.is_dir() {
        return path.to_path_buf();
    }
    let mut p = path.parent();
    while let Some(d) = p {
        if d.is_dir() {
            return d.to_path_buf();
        }
        p = d.parent();
    }
    path.parent().map(Path::to_path_buf).unwrap_or_default()
}

/// Explorer's `/select,"…"` argument for `path`, with Windows separators
/// and the path in quotes (Explorer reads the raw command line, so the
/// quotes must reach it as written).
pub fn explorer_select_arg(path: &str) -> String {
    format!("/select,\"{}\"", windows_path(path))
}

/// `path` quoted for a raw Windows command line.
pub fn quoted(path: &str) -> String {
    format!("\"{}\"", windows_path(path))
}

/// `path` with `\` separators and no `\\?\` prefix (Explorer does not read
/// verbatim paths).
fn windows_path(path: &str) -> String {
    let p = path.strip_prefix(r"\\?\").unwrap_or(path);
    p.replace('/', "\\").trim_end_matches('"').to_owned()
}

#[cfg(target_os = "windows")]
fn reveal_inner(path: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let s = path.to_string_lossy();
    let mut cmd = Command::new("explorer.exe");
    if path.is_file() {
        cmd.raw_arg(explorer_select_arg(&s));
    } else {
        cmd.raw_arg(quoted(&folder_of(path).to_string_lossy()));
    }
    // Explorer's exit code is 1 even when it worked, so only a failure to
    // start counts.
    spawn(cmd)
}

#[cfg(target_os = "windows")]
fn open_inner(path: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let mut cmd = Command::new("explorer.exe");
    cmd.raw_arg(quoted(&path.to_string_lossy()));
    spawn(cmd)
}

#[cfg(target_os = "macos")]
fn reveal_inner(path: &Path) -> Result<(), String> {
    let mut cmd = Command::new("open");
    if path.exists() {
        cmd.arg("-R").arg(path);
    } else {
        cmd.arg(folder_of(path));
    }
    spawn(cmd)
}

#[cfg(target_os = "macos")]
fn open_inner(path: &Path) -> Result<(), String> {
    let mut cmd = Command::new("open");
    cmd.arg(path);
    spawn(cmd)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn reveal_inner(path: &Path) -> Result<(), String> {
    let mut cmd = Command::new("xdg-open");
    cmd.arg(folder_of(path));
    spawn(cmd)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn open_inner(path: &Path) -> Result<(), String> {
    let mut cmd = Command::new("xdg-open");
    cmd.arg(path);
    spawn(cmd)
}

/// Starts `cmd` and reaps it on a thread of its own (no zombie, no wait
/// on the UI thread).
fn spawn(mut cmd: Command) -> Result<(), String> {
    if crate::view::tour::enabled() {
        // The UI tour records the call instead of opening windows.
        crate::crashlog::log_line(&format!("tour: would run {cmd:?}"));
        return Ok(());
    }
    let mut child = cmd.spawn().map_err(|e| format!("{cmd:?}: {e}"))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explorer_arguments_keep_spaces_quoted() {
        assert_eq!(
            explorer_select_arg(r"D:\Tern testing\diagram-1.svg"),
            r#"/select,"D:\Tern testing\diagram-1.svg""#
        );
        assert_eq!(
            explorer_select_arg("C:/Users/Khubaib Nazeer/Tern testing/table 1.xlsx"),
            r#"/select,"C:\Users\Khubaib Nazeer\Tern testing\table 1.xlsx""#
        );
        assert_eq!(
            explorer_select_arg(r"\\?\D:\Tern testing\a.png"),
            r#"/select,"D:\Tern testing\a.png""#
        );
        assert_eq!(quoted(r"D:\Tern testing"), r#""D:\Tern testing""#);
    }

    #[test]
    fn a_missing_file_reveals_its_nearest_folder() {
        let dir = std::env::temp_dir().join("tern reveal test");
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(folder_of(&dir.join("gone.svg")), dir);
        assert_eq!(folder_of(&dir.join("a/b/c.txt")), dir);
        assert_eq!(folder_of(&dir), dir);
    }
}
