//! The page cache: a page read once in a session is not fetched again for
//! half an hour, and with a cache folder it survives restarts for a day.
//! Pages a server marks `Cache-Control: no-store` are never kept.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use url::Url;

use crate::extract::Anchor;
use crate::fetch::Page;
use crate::icon::SourceIcon;

pub const MEMORY_TTL: Duration = Duration::from_secs(30 * 60);
pub const DISK_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const MEMORY_CAP: usize = 256;

/// The cache key: the URL without its fragment.
pub fn key(url: &str) -> String {
    match Url::parse(url.trim()) {
        Ok(mut u) => {
            u.set_fragment(None);
            u.to_string()
        }
        Err(_) => url.trim().to_owned(),
    }
}

#[derive(Debug, Default)]
pub struct Memory {
    pages: HashMap<String, (Instant, Page)>,
}

impl Memory {
    pub fn get(&mut self, key: &str, now: Instant) -> Option<Page> {
        match self.pages.get(key) {
            Some((at, p)) if now.duration_since(*at) < MEMORY_TTL => Some(p.clone()),
            Some(_) => {
                self.pages.remove(key);
                None
            }
            None => None,
        }
    }

    pub fn put(&mut self, key: String, page: Page, now: Instant) {
        if self.pages.len() >= MEMORY_CAP && !self.pages.contains_key(&key) {
            if let Some(oldest) = self
                .pages
                .iter()
                .min_by_key(|(_, (at, _))| *at)
                .map(|(k, _)| k.clone())
            {
                self.pages.remove(&oldest);
            }
        }
        self.pages.insert(key, (now, page));
    }

    pub fn len(&self) -> usize {
        self.pages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }
}

/// A page as stored on disk.
#[derive(Serialize, Deserialize)]
struct Stored {
    fetched_at: u64,
    url: String,
    final_url: String,
    site: String,
    title: String,
    description: Option<String>,
    favicon: Option<String>,
    content_type: String,
    text: String,
    bytes: usize,
    truncated: bool,
    markdown: String,
    links: Vec<Anchor>,
}

fn file_for(dir: &Path, key: &str) -> PathBuf {
    // FNV-1a: stable across runs and platforms, unlike the std hasher.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in key.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    dir.join(format!("{h:016x}.json"))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Reads a page from the disk cache when it is younger than a day.
pub fn disk_get(dir: &Path, key: &str) -> Option<Page> {
    let bytes = std::fs::read(file_for(dir, key)).ok()?;
    let s: Stored = serde_json::from_slice(&bytes).ok()?;
    if now_secs().saturating_sub(s.fetched_at) >= DISK_TTL.as_secs() {
        return None;
    }
    let mut icon = Url::parse(&s.final_url)
        .map(|u| SourceIcon::for_url(&u))
        .unwrap_or(SourceIcon {
            builtin: None,
            favicon: None,
        });
    if s.favicon.is_some() {
        icon.favicon = s.favicon;
    }
    Some(Page {
        url: s.url,
        final_url: s.final_url,
        site: s.site,
        title: s.title,
        description: s.description,
        icon,
        content_type: s.content_type,
        text: s.text,
        bytes: s.bytes,
        truncated: s.truncated,
        markdown: s.markdown,
        links: s.links,
    })
}

/// Writes a page to the disk cache. Failures are ignored: the cache is a
/// convenience, never a reason a read fails.
pub fn disk_put(dir: &Path, key: &str, p: &Page) {
    let s = Stored {
        fetched_at: now_secs(),
        url: p.url.clone(),
        final_url: p.final_url.clone(),
        site: p.site.clone(),
        title: p.title.clone(),
        description: p.description.clone(),
        favicon: p.icon.favicon.clone(),
        content_type: p.content_type.clone(),
        text: p.text.clone(),
        bytes: p.bytes,
        truncated: p.truncated,
        markdown: p.markdown.clone(),
        links: p.links.clone(),
    };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    if let Ok(json) = serde_json::to_vec(&s) {
        let path = file_for(dir, key);
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(url: &str) -> Page {
        Page {
            url: url.into(),
            final_url: url.into(),
            site: "example.org".into(),
            title: "T".into(),
            description: None,
            icon: SourceIcon::for_url(&Url::parse(url).unwrap()),
            content_type: "text/html".into(),
            text: "Body".into(),
            bytes: 4,
            truncated: false,
            markdown: "Body".into(),
            links: vec![Anchor {
                text: "a".into(),
                url: "https://example.org/a".into(),
            }],
        }
    }

    #[test]
    fn keys_drop_the_fragment() {
        assert_eq!(key("https://example.org/a#part"), "https://example.org/a");
        assert_eq!(key(" https://example.org/a "), "https://example.org/a");
    }

    #[test]
    fn memory_expires_and_stays_bounded() {
        let mut m = Memory::default();
        let t0 = Instant::now();
        m.put("k".into(), page("https://example.org/"), t0);
        assert!(m.get("k", t0 + Duration::from_secs(60)).is_some());
        assert!(
            m.get("k", t0 + MEMORY_TTL + Duration::from_secs(1))
                .is_none()
        );
        assert!(m.is_empty());
        for i in 0..(MEMORY_CAP + 10) {
            m.put(
                format!("k{i}"),
                page("https://example.org/"),
                t0 + Duration::from_millis(i as u64),
            );
        }
        assert_eq!(m.len(), MEMORY_CAP);
        assert!(m.get("k0", t0).is_none(), "the oldest went first");
    }

    #[test]
    fn disk_round_trip() {
        let dir = std::env::temp_dir().join(format!("tern-cache-test-{}", std::process::id()));
        let p = page("https://example.org/x");
        disk_put(&dir, "https://example.org/x", &p);
        assert_eq!(disk_get(&dir, "https://example.org/x"), Some(p));
        assert_eq!(disk_get(&dir, "https://example.org/other"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
