use std::path::Path;

use crate::PlatformResult;

/// Free and total space on the volume holding a path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiskSpace {
    pub available: u64,
    pub total: u64,
}

impl DiskSpace {
    pub fn of(path: &Path) -> PlatformResult<Self> {
        Ok(Self {
            available: fs4::available_space(path)?,
            total: fs4::total_space(path)?,
        })
    }

    /// Below this, `tern doctor` warns: crawl caches and PDFs add up.
    pub const LOW_WATERMARK: u64 = 2 * 1024 * 1024 * 1024;

    pub fn is_low(&self) -> bool {
        self.available < Self::LOW_WATERMARK
    }
}

/// `1.5 GB`, `320 MB`: sizes in plain words.
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_sizes() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1536), "1.5 KB");
        assert_eq!(human_bytes(3 * 1024 * 1024 * 1024), "3.0 GB");
    }

    #[test]
    fn reads_the_temp_volume() {
        let d = DiskSpace::of(&std::env::temp_dir()).unwrap();
        assert!(d.total >= d.available);
    }
}
