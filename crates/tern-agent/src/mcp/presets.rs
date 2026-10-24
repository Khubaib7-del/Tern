//! Adding MCP servers without editing `mcp.json` by hand: a gallery of
//! common servers with their commands filled in ([`TEMPLATES`]), a parser
//! for the JSON other apps hand out ([`parse_pasted`]: Claude Desktop and
//! Cursor's `mcpServers`, VS Code's `servers`, or a bare map), and a
//! command-line splitter for the Add form ([`split_command`]).
//!
//! Every server added these ways starts switched off and untrusted
//! ([`super::McpConfig::add`]); the user reads its trust preview before it
//! reaches a chat.

use std::collections::BTreeMap;

use super::config::{ServerConfig, valid_name};

/// A value the user fills in before a template can run (a key, a folder).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Needs {
    /// An environment variable (stdio) or header (http) name.
    pub key: &'static str,
    pub hint: &'static str,
    /// A secret: shown masked.
    pub secret: bool,
}

/// One server in the gallery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Template {
    /// The server's name in `mcp.json`.
    pub id: &'static str,
    pub name: &'static str,
    /// What it gives Tern, in one line.
    pub blurb: &'static str,
    /// The logo id (`crate::brands` in the desktop app).
    pub logo: &'static str,
    pub command: Option<&'static str>,
    pub args: &'static [&'static str],
    pub url: Option<&'static str>,
    pub needs: &'static [Needs],
    /// Where it is documented.
    pub docs: &'static str,
}

impl Template {
    /// The server config with the user's values (`values` by
    /// [`Needs::key`]); a missing value is an error naming it.
    pub fn config(&self, values: &BTreeMap<String, String>) -> Result<ServerConfig, String> {
        let mut s = ServerConfig {
            command: self.command.map(str::to_owned),
            args: self.args.iter().map(|a| (*a).to_owned()).collect(),
            url: self.url.map(str::to_owned),
            kind: self.url.map(|_| "http".to_owned()),
            ..Default::default()
        };
        for n in self.needs {
            let v = values
                .get(n.key)
                .map(|v| v.trim())
                .filter(|v| !v.is_empty())
                .ok_or_else(|| format!("{} needs {} ({}).", self.name, n.key, n.hint))?;
            if n.key == "PATH_ARG" {
                s.args.push(v.to_owned());
            } else if self.url.is_some() {
                let value = if n.key == "Authorization" && !v.starts_with("Bearer ") {
                    format!("Bearer {v}")
                } else {
                    v.to_owned()
                };
                s.headers.insert(n.key.to_owned(), value);
            } else {
                s.env.insert(n.key.to_owned(), v.to_owned());
            }
        }
        s.transport()?;
        Ok(s)
    }
}

const NONE: &[Needs] = &[];

/// The gallery: common servers, prefilled.
pub const TEMPLATES: &[Template] = &[
    Template {
        id: "filesystem",
        name: "Filesystem",
        blurb: "Read and search files in one folder you choose (and write, with approval).",
        logo: "filesystem",
        command: Some("npx"),
        args: &["-y", "@modelcontextprotocol/server-filesystem"],
        url: None,
        needs: &[Needs {
            key: "PATH_ARG",
            hint: "the folder it may use",
            secret: false,
        }],
        docs: "https://github.com/modelcontextprotocol/servers/tree/main/src/filesystem",
    },
    Template {
        id: "fetch",
        name: "Fetch",
        blurb: "Fetch a web page and turn it into Markdown.",
        logo: "fetch",
        command: Some("uvx"),
        args: &["mcp-server-fetch"],
        url: None,
        needs: NONE,
        docs: "https://github.com/modelcontextprotocol/servers/tree/main/src/fetch",
    },
    Template {
        id: "memory",
        name: "Memory",
        blurb: "A small knowledge graph the model can keep notes in across chats.",
        logo: "memory",
        command: Some("npx"),
        args: &["-y", "@modelcontextprotocol/server-memory"],
        url: None,
        needs: NONE,
        docs: "https://github.com/modelcontextprotocol/servers/tree/main/src/memory",
    },
    Template {
        id: "sequential-thinking",
        name: "Sequential Thinking",
        blurb: "A scratchpad tool for step-by-step reasoning that can branch and revise.",
        logo: "sequential-thinking",
        command: Some("npx"),
        args: &["-y", "@modelcontextprotocol/server-sequential-thinking"],
        url: None,
        needs: NONE,
        docs: "https://github.com/modelcontextprotocol/servers/tree/main/src/sequentialthinking",
    },
    Template {
        id: "github",
        name: "GitHub",
        blurb: "Search code, read issues and pull requests on GitHub (GitHub's hosted server).",
        logo: "github",
        command: None,
        args: &[],
        url: Some("https://api.githubcopilot.com/mcp/"),
        needs: &[Needs {
            key: "Authorization",
            hint: "a GitHub personal access token",
            secret: true,
        }],
        docs: "https://github.com/github/github-mcp-server",
    },
    Template {
        id: "brave-search",
        name: "Brave Search",
        blurb: "Web and news search through Brave's API.",
        logo: "brave",
        command: Some("npx"),
        args: &["-y", "@brave/brave-search-mcp-server"],
        url: None,
        needs: &[Needs {
            key: "BRAVE_API_KEY",
            hint: "a Brave Search API key",
            secret: true,
        }],
        docs: "https://github.com/brave/brave-search-mcp-server",
    },
    Template {
        id: "context7",
        name: "Context7",
        blurb: "Up-to-date library and framework docs, by version.",
        logo: "context7",
        command: None,
        args: &[],
        url: Some("https://mcp.context7.com/mcp"),
        needs: NONE,
        docs: "https://github.com/upstash/context7",
    },
    Template {
        id: "firecrawl",
        name: "Firecrawl",
        blurb: "Scrape, crawl, and search the web with Firecrawl's tools.",
        logo: "firecrawl",
        command: Some("npx"),
        args: &["-y", "firecrawl-mcp"],
        url: None,
        needs: &[Needs {
            key: "FIRECRAWL_API_KEY",
            hint: "a Firecrawl API key",
            secret: true,
        }],
        docs: "https://github.com/firecrawl/firecrawl-mcp-server",
    },
    Template {
        id: "notion",
        name: "Notion",
        blurb: "Search and read your Notion pages (Notion's hosted server).",
        logo: "notion",
        command: None,
        args: &[],
        url: Some("https://mcp.notion.com/mcp"),
        needs: NONE,
        docs: "https://developers.notion.com/docs/mcp",
    },
    Template {
        id: "time",
        name: "Time",
        blurb: "The current time and time-zone conversions.",
        logo: "time",
        command: Some("uvx"),
        args: &["mcp-server-time"],
        url: None,
        needs: NONE,
        docs: "https://github.com/modelcontextprotocol/servers/tree/main/src/time",
    },
];

/// A template by id.
pub fn template(id: &str) -> Option<&'static Template> {
    TEMPLATES.iter().find(|t| t.id == id)
}

/// Splits a command line the way a shell would for simple cases: spaces
/// separate words, double or single quotes keep spaces, a backslash
/// escapes the next character outside single quotes.
pub fn split_command(line: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    let mut chars = line.trim().chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('\''), c) => cur.push(c),
            (_, '\\') => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
                started = true;
            }
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            (None, c) => {
                cur.push(c);
                started = true;
            }
        }
    }
    if quote.is_some() {
        return Err("A quote in the command is not closed.".into());
    }
    if started || !cur.is_empty() {
        out.push(cur);
    }
    if out.is_empty() {
        return Err("The command is empty.".into());
    }
    Ok(out)
}

/// Reads `KEY=value` lines (env vars) or `Name: value` lines (headers);
/// blank lines and `#` comments are skipped.
pub fn parse_pairs(text: &str, sep: char) -> Result<BTreeMap<String, String>, String> {
    let mut m = BTreeMap::new();
    for (i, line) in text.lines().enumerate() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let (k, v) = l
            .split_once(sep)
            .ok_or_else(|| format!("Line {}: expected `name{sep}value`.", i + 1))?;
        let k = k.trim();
        if k.is_empty() || k.contains(char::is_whitespace) {
            return Err(format!("Line {}: “{k}” is not a name.", i + 1));
        }
        m.insert(k.to_owned(), v.trim().to_owned());
    }
    Ok(m)
}

/// A server from the Add form: a command line (stdio) or a URL (http).
pub fn from_form(
    command_or_url: &str,
    env_or_headers: &str,
) -> Result<ServerConfig, String> {
    let t = command_or_url.trim();
    if t.starts_with("http://") || t.starts_with("https://") {
        let u = url::Url::parse(t).map_err(|_| format!("“{t}” is not a URL."))?;
        let local = matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
        if u.scheme() == "http" && !local {
            return Err("Use https for a server on the internet.".into());
        }
        Ok(ServerConfig {
            kind: Some("http".into()),
            url: Some(t.to_owned()),
            headers: parse_pairs(env_or_headers, ':')?,
            ..Default::default()
        })
    } else {
        let mut words = split_command(t)?;
        let command = words.remove(0);
        Ok(ServerConfig {
            command: Some(command),
            args: words,
            env: parse_pairs(env_or_headers, '=')?,
            ..Default::default()
        })
    }
}

/// Reads pasted JSON into named servers: `{"mcpServers": {…}}` (Claude
/// Desktop, Cursor, Windsurf), `{"servers": {…}}` (VS Code),
/// `{"mcp": {"servers": {…}}}`, or a bare map of name to server. Every
/// server is checked (name, transport); the error lists every problem.
pub fn parse_pasted(text: &str) -> Result<Vec<(String, ServerConfig)>, Vec<String>> {
    let v: serde_json::Value = serde_json::from_str(text.trim()).map_err(|e| {
        vec![format!(
            "That is not valid JSON (line {}, column {}): {e}",
            e.line(),
            e.column()
        )]
    })?;
    let map = v
        .get("mcpServers")
        .or_else(|| v.get("servers"))
        .or_else(|| v.get("mcp").and_then(|m| m.get("servers")))
        .or_else(|| v.get("context_servers"))
        .unwrap_or(&v);
    let Some(obj) = map.as_object() else {
        return Err(vec!["Expected an object of servers by name.".into()]);
    };
    if obj.contains_key("command") || obj.contains_key("url") {
        return Err(vec![
            "That is one server without a name: wrap it as {\"mcpServers\": {\"name\": …}}."
                .into(),
        ]);
    }
    let mut out = Vec::new();
    let mut errs = Vec::new();
    for (name, raw) in obj {
        if !valid_name(name) {
            errs.push(format!(
                "“{name}”: a server name uses letters, digits, `-` and `_` (up to 40 characters)."
            ));
            continue;
        }
        let mut raw = raw.clone();
        // VS Code and Zed say "serverUrl" or nest the command.
        if let Some(o) = raw.as_object_mut() {
            if let Some(u) = o.remove("serverUrl") {
                o.entry("url").or_insert(u);
            }
        }
        match serde_json::from_value::<ServerConfig>(raw) {
            Ok(s) => match s.transport() {
                Ok(_) => out.push((name.clone(), s)),
                Err(e) => errs.push(format!("“{name}”: {e}")),
            },
            Err(e) => errs.push(format!("“{name}”: {e}")),
        }
    }
    if out.is_empty() && errs.is_empty() {
        errs.push("No servers in that JSON.".into());
    }
    if errs.is_empty() { Ok(out) } else { Err(errs) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::Transport;

    #[test]
    fn templates_fill_in_and_name_what_they_need() {
        let mut ids: Vec<&str> = TEMPLATES.iter().map(|t| t.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), TEMPLATES.len());
        for t in TEMPLATES {
            assert!(valid_name(t.id), "{}", t.id);
            assert!(t.docs.starts_with("https://"), "{}", t.id);
            if t.needs.is_empty() {
                assert!(t.config(&BTreeMap::new()).is_ok(), "{}", t.id);
            } else {
                let e = t.config(&BTreeMap::new()).unwrap_err();
                assert!(e.contains(t.needs[0].key), "{e}");
            }
        }
        let fs = template("filesystem").unwrap();
        let s = fs
            .config(&BTreeMap::from([("PATH_ARG".into(), "/home/me/notes".into())]))
            .unwrap();
        assert_eq!(s.args.last().unwrap(), "/home/me/notes");
        assert_eq!(s.transport().unwrap(), Transport::Stdio);
        let gh = template("github").unwrap();
        let s = gh
            .config(&BTreeMap::from([("Authorization".into(), "ghp_x".into())]))
            .unwrap();
        assert_eq!(s.headers["Authorization"], "Bearer ghp_x");
        assert_eq!(s.transport().unwrap(), Transport::Http);
        let brave = template("brave-search").unwrap();
        let s = brave
            .config(&BTreeMap::from([("BRAVE_API_KEY".into(), "k".into())]))
            .unwrap();
        assert_eq!(s.env["BRAVE_API_KEY"], "k");
    }

    #[test]
    fn commands_split_like_a_shell() {
        assert_eq!(
            split_command(r#"npx -y "@x/fs" "/my files" a\ b 'c d'"#).unwrap(),
            vec!["npx", "-y", "@x/fs", "/my files", "a b", "c d"]
        );
        assert_eq!(split_command(r#"run """#).unwrap(), vec!["run", ""]);
        assert!(split_command("  ").is_err());
        assert!(split_command("npx \"open").is_err());
    }

    #[test]
    fn the_add_form_takes_a_command_or_a_url() {
        let s = from_form("uvx mcp-server-fetch --x", "A=1\n# c\nB = two").unwrap();
        assert_eq!(s.command.as_deref(), Some("uvx"));
        assert_eq!(s.args, vec!["mcp-server-fetch", "--x"]);
        assert_eq!(s.env["B"], "two");
        let h = from_form("https://example.com/mcp", "Authorization: Bearer t").unwrap();
        assert_eq!(h.transport().unwrap(), Transport::Http);
        assert_eq!(h.headers["Authorization"], "Bearer t");
        assert!(from_form("http://example.com/mcp", "").is_err());
        assert!(from_form("http://localhost:3000/mcp", "").is_ok());
        assert!(from_form("x", "NOEQUALS").unwrap_err().contains("Line 1"));
    }

    #[test]
    fn pasted_configs_from_other_apps() {
        let claude = r#"{"mcpServers": {
            "fs": {"command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]},
            "docs": {"type": "http", "url": "https://example.com/mcp", "headers": {"X": "1"}}}}"#;
        let got = parse_pasted(claude).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].0, "docs");
        let vscode = r#"{"servers": {"gh": {"type": "http", "url": "https://api.githubcopilot.com/mcp/"}}}"#;
        assert_eq!(parse_pasted(vscode).unwrap()[0].0, "gh");
        let bare = r#"{"time": {"command": "uvx", "args": ["mcp-server-time"]}}"#;
        assert_eq!(parse_pasted(bare).unwrap()[0].1.args, vec!["mcp-server-time"]);
        let zed = r#"{"mcp": {"servers": {"w": {"serverUrl": "https://w.example/mcp"}}}}"#;
        assert_eq!(
            parse_pasted(zed).unwrap()[0].1.url.as_deref(),
            Some("https://w.example/mcp")
        );
        // Errors name each problem.
        let bad = r#"{"mcpServers": {"bad name": {"command": "x"}, "empty": {}, "ok": {"command": "y"}}}"#;
        let e = parse_pasted(bad).unwrap_err();
        assert_eq!(e.len(), 2, "{e:?}");
        assert!(e.iter().any(|m| m.contains("bad name")));
        assert!(e.iter().any(|m| m.contains("empty")));
        assert!(parse_pasted("{nope").unwrap_err()[0].contains("not valid JSON"));
        assert!(parse_pasted(r#"{"command": "npx"}"#).unwrap_err()[0].contains("without a name"));
        assert!(parse_pasted("{}").is_err());
    }
}
