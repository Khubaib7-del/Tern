//! The operating system's reduced-motion setting.
//!
//! | OS | Read from | When it says "reduce" |
//! | --- | --- | --- |
//! | Linux (GNOME and GTK desktops) | `gsettings get org.gnome.desktop.interface enable-animations` | `false` |
//! | Linux (KDE Plasma) | `kreadconfig6` or `kreadconfig5`, group `KDE`, key `AnimationDurationFactor` | `0` |
//! | macOS | `defaults read com.apple.universalaccess reduceMotion` (the Accessibility > Display > Reduce motion switch) | `1` |
//! | Windows | Not read yet: the setting is only reachable through a `SystemParametersInfo` call, and this crate forbids `unsafe` | — |
//!
//! Each lookup runs a small system tool once per process and is cached. A
//! missing tool or an unset key reads as "no preference", so motion stays
//! on unless the user, `TERN_REDUCED_MOTION`, or the OS turns it off.

use std::process::Command;
use std::sync::OnceLock;

/// Whether the OS asks apps to reduce motion. Read once, then cached.
pub fn os_reduces_motion() -> bool {
    static CACHED: OnceLock<bool> = OnceLock::new();
    *CACHED.get_or_init(|| read().unwrap_or(false))
}

/// The OS setting, or `None` when this OS has none Tern can read.
pub fn read() -> Option<bool> {
    #[cfg(target_os = "linux")]
    {
        if let Some(v) = run(
            "gsettings",
            &["get", "org.gnome.desktop.interface", "enable-animations"],
        )
        .and_then(|out| parse_gsettings(&out))
        {
            return Some(v);
        }
        for tool in ["kreadconfig6", "kreadconfig5"] {
            if let Some(v) = run(
                tool,
                &["--group", "KDE", "--key", "AnimationDurationFactor"],
            )
            .and_then(|out| parse_kde_factor(&out))
            {
                return Some(v);
            }
        }
        None
    }
    #[cfg(target_os = "macos")]
    {
        run(
            "defaults",
            &["read", "com.apple.universalaccess", "reduceMotion"],
        )
        .and_then(|out| parse_macos(&out))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

/// Runs a tool and returns its trimmed output when it succeeds.
#[cfg_attr(not(any(target_os = "linux", target_os = "macos")), allow(dead_code))]
fn run(tool: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(tool).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// `enable-animations` is `true` or `false`; false means reduce motion.
pub fn parse_gsettings(out: &str) -> Option<bool> {
    match out.trim() {
        "false" => Some(true),
        "true" => Some(false),
        _ => None,
    }
}

/// KDE's animation speed factor: `0` turns animations off.
pub fn parse_kde_factor(out: &str) -> Option<bool> {
    let f: f64 = out.trim().parse().ok()?;
    Some(f <= 0.0)
}

/// macOS writes `1` when Reduce motion is on.
pub fn parse_macos(out: &str) -> Option<bool> {
    match out.trim() {
        "1" | "true" | "YES" => Some(true),
        "0" | "false" | "NO" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_each_os_answer() {
        assert_eq!(parse_gsettings("false\n"), Some(true));
        assert_eq!(parse_gsettings("true"), Some(false));
        assert_eq!(parse_gsettings("No such key"), None);
        assert_eq!(parse_kde_factor("0"), Some(true));
        assert_eq!(parse_kde_factor("0.5"), Some(false));
        assert_eq!(parse_kde_factor(""), None);
        assert_eq!(parse_macos("1"), Some(true));
        assert_eq!(parse_macos("0"), Some(false));
        assert_eq!(parse_macos("The domain/default pair does not exist"), None);
    }

    #[test]
    fn reading_never_fails() {
        // Whatever the machine has, the answer is cached and stable.
        assert_eq!(os_reduces_motion(), os_reduces_motion());
    }
}
