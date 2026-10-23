//! `<data>/mcp.json`: the MCP servers the user added.
//!
//! The shape is Claude Desktop's, so a config can be copied in as is:
//!
//! ```json
//! {"mcpServers": {
//!   "notes": {"command": "npx", "args": ["-y", "notes-mcp"], "env": {"NOTES_DIR": "~/notes"}},
//!   "docs":  {"type": "http", "url": "https://example.com/mcp", "headers": {"Authorization": "Bearer …"}}
//! }}
//! ```
//!
//! Tern keeps its own state for a server under `"tern"` (other apps ignore
//! unknown keys): whether it is enabled, what the user trusted (a
//! fingerprint of the tools they reviewed), and each tool's switch and
//! approval mode. A server is off until the user has seen its trust
//! preview and enabled it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::policy::Mode;

const FILE: &str = "mcp.json";

/// The whole file.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct McpConfig {
    #[serde(rename = "mcpServers", default)]
    pub servers: BTreeMap<String, ServerConfig>,
}

/// How to reach one server.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// `stdio` or `http` (`streamable-http` and `sse` read as `http`).
    /// Missing means stdio when there is a command, http when a url.
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Working folder for a stdio server.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    /// Some apps mark a server off this way; Tern honours it.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub disabled: bool,
    pub tern: TernState,
}

/// Tern's own state for one server.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TernState {
    pub enabled: bool,
    /// The fingerprint of the tool list the user reviewed and trusted.
    /// When the server's tools change, the server stops until the user
    /// reviews them again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trusted: Option<String>,
    /// Per tool: on or off, and whether it asks, runs, or is refused.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub tools: BTreeMap<String, ToolSetting>,
    /// Seconds a call may take. `None` means 60.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

/// One MCP tool's switch and approval mode.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolSetting {
    pub enabled: bool,
    pub approval: Mode,
}

impl Default for ToolSetting {
    fn default() -> Self {
        Self {
            enabled: true,
            approval: Mode::Ask,
        }
    }
}

/// How a server is reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    Stdio,
    Http,
}

impl ServerConfig {
    pub fn transport(&self) -> Result<Transport, String> {
        match self.kind.as_deref().map(str::to_ascii_lowercase).as_deref() {
            Some("stdio") => Ok(Transport::Stdio),
            Some("http" | "streamable-http" | "streamable_http" | "sse") => Ok(Transport::Http),
            Some(other) => Err(format!("Unknown MCP transport “{other}”.")),
            None if self.command.is_some() => Ok(Transport::Stdio),
            None if self.url.is_some() => Ok(Transport::Http),
            None => Err("An MCP server needs a command (stdio) or a url (http).".into()),
        }
    }

    /// The command line or address, for lists and previews.
    pub fn target(&self) -> String {
        match self.transport() {
            Ok(Transport::Stdio) => {
                let mut s = self.command.clone().unwrap_or_default();
                for a in &self.args {
                    s.push(' ');
                    if a.contains(' ') {
                        s.push_str(&format!("\"{a}\""));
                    } else {
                        s.push_str(a);
                    }
                }
                s
            }
            Ok(Transport::Http) => self.url.clone().unwrap_or_default(),
            Err(e) => e,
        }
    }

    pub fn timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.tern.timeout_secs.unwrap_or(60).clamp(1, 600))
    }

    /// Whether the server's tools reach chats: switched on, trusted, and
    /// not marked disabled by another app.
    pub fn active(&self) -> bool {
        self.tern.enabled && self.tern.trusted.is_some() && !self.disabled
    }

    /// The tool's setting, or the default (on, asks).
    pub fn tool(&self, name: &str) -> ToolSetting {
        self.tern.tools.get(name).cloned().unwrap_or_default()
    }
}

impl McpConfig {
    pub fn path_in(dir: &Path) -> PathBuf {
        dir.join(FILE)
    }

    /// Reads `<dir>/mcp.json`. A missing file is an empty config; a broken
    /// one is an error, so a typo never silently drops servers.
    pub fn load_from(dir: &Path) -> Result<Self, String> {
        let path = Self::path_in(dir);
        match std::fs::read_to_string(&path) {
            Ok(s) if s.trim().is_empty() => Ok(Self::default()),
            Ok(s) => serde_json::from_str(&s)
                .map_err(|e| format!("{} is not valid MCP config: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("Could not read {}: {e}", path.display())),
        }
    }

    pub fn save_to(&self, dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let path = Self::path_in(dir);
        let tmp = path.with_extension("json.tmp");
        let body = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, body).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
    }

    /// The user's config from the app data folder.
    pub fn load() -> Result<Self, String> {
        Self::load_from(&data_dir()?)
    }

    pub fn save(&self) -> Result<(), String> {
        self.save_to(&data_dir()?)
    }

    pub fn get(&self, name: &str) -> Result<&ServerConfig, String> {
        self.servers
            .get(name)
            .ok_or_else(|| format!("There is no MCP server called “{name}”. See `tern mcp list`."))
    }

    pub fn get_mut(&mut self, name: &str) -> Result<&mut ServerConfig, String> {
        self.servers
            .get_mut(name)
            .ok_or_else(|| format!("There is no MCP server called “{name}”. See `tern mcp list`."))
    }

    /// Adds a server, switched off and untrusted. Names are letters,
    /// digits, `-` and `_`, so tool names stay readable.
    pub fn add(&mut self, name: &str, server: ServerConfig) -> Result<(), String> {
        if !valid_name(name) {
            return Err(
                "A server name uses letters, digits, `-` and `_` (up to 40 characters).".into(),
            );
        }
        if self.servers.contains_key(name) {
            return Err(format!("There is already an MCP server called “{name}”."));
        }
        server.transport()?;
        let mut server = server;
        server.tern.enabled = false;
        server.tern.trusted = None;
        self.servers.insert(name.to_owned(), server);
        Ok(())
    }
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 40
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The app data folder (`TERN_HOME` or the OS convention).
pub fn data_dir() -> Result<PathBuf, String> {
    tern_platform::AppPaths::discover()
        .map(|p| p.data_dir)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_claude_desktop_configs_and_keeps_unknown_shapes_out() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("mcp.json"),
            r#"{"mcpServers": {
                "fs": {"command": "npx", "args": ["-y", "@x/fs", "/my files"], "env": {"A": "1"}},
                "web": {"type": "http", "url": "https://example.com/mcp"}
            }}"#,
        )
        .unwrap();
        let c = McpConfig::load_from(dir.path()).unwrap();
        let fs = c.get("fs").unwrap();
        assert_eq!(fs.transport().unwrap(), Transport::Stdio);
        assert_eq!(fs.target(), "npx -y @x/fs \"/my files\"");
        assert!(!fs.active(), "nothing runs until reviewed and enabled");
        assert_eq!(c.get("web").unwrap().transport().unwrap(), Transport::Http);
        c.save_to(dir.path()).unwrap();
        let again = McpConfig::load_from(dir.path()).unwrap();
        assert_eq!(again, c);
        assert!(
            McpConfig::load_from(&dir.path().join("none"))
                .unwrap()
                .servers
                .is_empty()
        );
        std::fs::write(dir.path().join("mcp.json"), "{nope").unwrap();
        assert!(McpConfig::load_from(dir.path()).is_err());
    }

    #[test]
    fn add_checks_names_and_starts_off() {
        let mut c = McpConfig::default();
        let mut s = ServerConfig {
            command: Some("x".into()),
            ..Default::default()
        };
        s.tern.enabled = true;
        s.tern.trusted = Some("abc".into());
        assert!(c.add("bad name", s.clone()).is_err());
        c.add("ok-1", s.clone()).unwrap();
        assert!(!c.get("ok-1").unwrap().tern.enabled);
        assert!(c.get("ok-1").unwrap().tern.trusted.is_none());
        assert!(c.add("ok-1", s).is_err());
        assert!(c.add("none", ServerConfig::default()).is_err());
    }
}
