//! App data locations.
//!
//! | OS | Data directory |
//! | --- | --- |
//! | Windows | `%LOCALAPPDATA%\\Tern\\data` |
//! | Linux | `$XDG_DATA_HOME/tern` |
//! | macOS | `~/Library/Application Support/Tern` |
//!
//! `TERN_HOME` overrides the location (used by tests and portable setups).

use std::path::{Path, PathBuf};

use crate::{PlatformError, PlatformResult};

/// Where Tern keeps its own data. The vault is user-chosen and lives elsewhere.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppPaths {
    pub data_dir: PathBuf,
}

impl AppPaths {
    /// Resolves the data directory from `TERN_HOME` or the OS convention.
    pub fn discover() -> PlatformResult<Self> {
        if let Some(home) = std::env::var_os("TERN_HOME").filter(|v| !v.is_empty()) {
            return Ok(Self::at(home));
        }
        let dirs = directories::ProjectDirs::from("", "", "Tern").ok_or(PlatformError::NoHome)?;
        Ok(Self::at(dirs.data_local_dir()))
    }

    pub fn at(dir: impl AsRef<Path>) -> Self {
        Self {
            data_dir: dir.as_ref().to_path_buf(),
        }
    }

    pub fn database(&self) -> PathBuf {
        self.data_dir.join("tern.sqlite3")
    }

    pub fn artifacts(&self) -> PathBuf {
        self.data_dir.join("artifacts")
    }

    pub fn crawl_cache(&self) -> PathBuf {
        self.data_dir.join("cache").join("crawl")
    }

    pub fn checkpoints(&self) -> PathBuf {
        self.data_dir.join("checkpoints")
    }

    /// Creates the data directory tree if it is missing.
    pub fn ensure(&self) -> PlatformResult<()> {
        for dir in [
            self.data_dir.clone(),
            self.artifacts(),
            self.crawl_cache(),
            self.checkpoints(),
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }

    /// Writes and removes a probe file to prove the directory is writable.
    pub fn check_writable(&self) -> PlatformResult<()> {
        self.ensure()?;
        let probe = self.data_dir.join(".tern-write-probe");
        std::fs::write(&probe, b"ok")?;
        std::fs::remove_file(&probe)?;
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
        ] {
            assert!(sub.starts_with(tmp.path()));
        }
        assert!(p.artifacts().is_dir());
        assert!(!tmp.path().join(".tern-write-probe").exists());
    }
}
