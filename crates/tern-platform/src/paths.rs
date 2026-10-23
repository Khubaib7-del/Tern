//! App data locations.
//!
//! | OS | Data | Settings | Cache |
//! | --- | --- | --- | --- |
//! | Windows | `%LOCALAPPDATA%\Tern\data` | the data directory | `<data>\cache` |
//! | Linux | `$XDG_DATA_HOME/tern` (`~/.local/share/tern`) | `$XDG_CONFIG_HOME/tern` (`~/.config/tern`) | `$XDG_CACHE_HOME/tern` (`~/.cache/tern`) |
//! | macOS | `~/Library/Application Support/Tern` | the data directory | `~/Library/Caches/Tern` |
//!
//! `TERN_HOME` overrides all three (used by tests and portable setups):
//! data and settings live in it and the cache in `<TERN_HOME>/cache`.
//! Windows keeps the single-folder layout it shipped with, so nothing moves
//! for existing users.

use std::path::{Path, PathBuf};

use crate::{PlatformError, PlatformResult};

/// Where Tern keeps its own data. The vault is user-chosen and lives elsewhere.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppPaths {
    /// Chats, runs, the database, study material.
    pub data_dir: PathBuf,
    /// Settings: preferences, MCP servers, global instructions.
    pub config_dir: PathBuf,
    /// Things Tern can fetch again: the crawl cache.
    pub cache_dir: PathBuf,
}

impl AppPaths {
    /// Resolves the directories from `TERN_HOME` or the OS convention.
    pub fn discover() -> PlatformResult<Self> {
        if let Some(home) = std::env::var_os("TERN_HOME").filter(|v| !v.is_empty()) {
            return Ok(Self::at(home));
        }
        let dirs = directories::ProjectDirs::from("", "", "Tern").ok_or(PlatformError::NoHome)?;
        Ok(Self::for_os(
            std::env::consts::OS,
            dirs.data_local_dir(),
            dirs.config_dir(),
            dirs.cache_dir(),
        ))
    }

    /// The layout for `os`, given that OS's conventional directories.
    pub fn for_os(os: &str, data: &Path, config: &Path, cache: &Path) -> Self {
        match os {
            "linux" | "freebsd" | "openbsd" | "netbsd" | "dragonfly" => Self {
                data_dir: data.to_path_buf(),
                config_dir: config.to_path_buf(),
                cache_dir: cache.to_path_buf(),
            },
            "macos" => Self {
                data_dir: data.to_path_buf(),
                config_dir: data.to_path_buf(),
                cache_dir: cache.to_path_buf(),
            },
            _ => Self::at(data),
        }
    }

    /// Everything under one folder: data and settings in it, the cache in
    /// `cache/`.
    pub fn at(dir: impl AsRef<Path>) -> Self {
        let dir = dir.as_ref().to_path_buf();
        Self {
            config_dir: dir.clone(),
            cache_dir: dir.join("cache"),
            data_dir: dir,
        }
    }

    pub fn database(&self) -> PathBuf {
        self.data_dir.join("tern.sqlite3")
    }

    pub fn artifacts(&self) -> PathBuf {
        self.data_dir.join("artifacts")
    }

    pub fn crawl_cache(&self) -> PathBuf {
        self.cache_dir.join("crawl")
    }

    pub fn checkpoints(&self) -> PathBuf {
        self.data_dir.join("checkpoints")
    }

    /// Creates the directory tree if it is missing.
    pub fn ensure(&self) -> PlatformResult<()> {
        for dir in [
            self.data_dir.clone(),
            self.config_dir.clone(),
            self.artifacts(),
            self.crawl_cache(),
            self.checkpoints(),
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }

    /// Writes and removes a probe file to prove the directories are writable.
    pub fn check_writable(&self) -> PlatformResult<()> {
        self.ensure()?;
        for dir in [&self.data_dir, &self.config_dir, &self.cache_dir] {
            let probe = dir.join(".tern-write-probe");
            std::fs::write(&probe, b"ok")?;
            std::fs::remove_file(&probe)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_is_under_the_data_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let p = AppPaths::at(tmp.path());
        p.ensure().unwrap();
        p.check_writable().unwrap();
        for sub in [
            p.database(),
            p.artifacts(),
            p.crawl_cache(),
            p.checkpoints(),
            p.config_dir.clone(),
        ] {
            assert!(sub.starts_with(tmp.path()));
        }
        assert!(p.artifacts().is_dir());
        assert_eq!(p.crawl_cache(), tmp.path().join("cache").join("crawl"));
        assert!(!tmp.path().join(".tern-write-probe").exists());
    }

    #[test]
    fn each_os_has_its_convention() {
        let (d, c, k) = (
            Path::new("/d/tern"),
            Path::new("/c/tern"),
            Path::new("/k/tern"),
        );
        let linux = AppPaths::for_os("linux", d, c, k);
        assert_eq!(linux.config_dir, c);
        assert_eq!(linux.crawl_cache(), k.join("crawl"));
        let mac = AppPaths::for_os("macos", d, c, k);
        assert_eq!(mac.config_dir, d);
        assert_eq!(mac.cache_dir, k);
        // Windows keeps one folder, as it shipped.
        let win = AppPaths::for_os("windows", d, c, k);
        assert_eq!(win, AppPaths::at(d));
        assert_eq!(win.crawl_cache(), d.join("cache").join("crawl"));
    }

    /// On Linux the real lookup follows the XDG variables.
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_follows_xdg() {
        if std::env::var_os("TERN_HOME").is_some() {
            return;
        }
        let p = AppPaths::discover().unwrap();
        assert!(p.data_dir.ends_with("tern"), "{p:?}");
        assert!(p.config_dir.ends_with("tern"), "{p:?}");
        assert_ne!(p.data_dir, p.config_dir);
    }
}
