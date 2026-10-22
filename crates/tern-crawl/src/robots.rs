//! robots.txt (RFC 9309). Tern reads pages for the user, but an agent run
//! can read many in a row, so its automated reads keep to what a site's
//! robots.txt allows for the `Tern` user agent (or `*`). A page the user
//! opens themselves in the built-in browser is not a crawl and skips this.

/// One robots.txt group: its agents, rules, and Crawl-delay.
type Group = (Vec<String>, Vec<(bool, String)>, Option<f32>);

/// The rules of the group that applies to Tern.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Robots {
    /// (allow, pattern), in file order.
    rules: Vec<(bool, String)>,
    /// Seconds between requests the site asks for, if any.
    pub crawl_delay: Option<f32>,
    /// Everything is off limits (the file could not be read because the
    /// site failed, which RFC 9309 treats as a full disallow).
    pub disallow_all: bool,
}

impl Robots {
    /// A site with no robots.txt (or a 4xx): everything is allowed.
    pub fn allow_all() -> Self {
        Self::default()
    }

    /// A site whose robots.txt failed with a server error.
    pub fn unreachable() -> Self {
        Self {
            disallow_all: true,
            ..Self::default()
        }
    }

    /// Parses a robots.txt body for the product token `agent`.
    pub fn parse(body: &str, agent: &str) -> Self {
        let agent = agent.to_ascii_lowercase();
        // Groups: (agents, rules, delay).
        let mut groups: Vec<Group> = Vec::new();
        let mut open_agents = false;
        for raw in body.lines() {
            let line = raw.split('#').next().unwrap_or_default().trim();
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let key = key.trim().to_ascii_lowercase();
            let value = value.trim();
            match key.as_str() {
                "user-agent" => {
                    if !open_agents {
                        groups.push((Vec::new(), Vec::new(), None));
                        open_agents = true;
                    }
                    if let Some(g) = groups.last_mut() {
                        g.0.push(value.to_ascii_lowercase());
                    }
                }
                "allow" | "disallow" => {
                    open_agents = false;
                    if let Some(g) = groups.last_mut() {
                        // An empty Disallow allows everything: no rule.
                        if !value.is_empty() {
                            g.1.push((key == "allow", value.to_owned()));
                        }
                    }
                }
                "crawl-delay" => {
                    open_agents = false;
                    if let Some(g) = groups.last_mut() {
                        g.2 = value
                            .parse::<f32>()
                            .ok()
                            .filter(|d| d.is_finite() && *d >= 0.0);
                    }
                }
                _ => {}
            }
        }
        let mine = |g: &&Group| {
            g.0.iter()
                .any(|a| a != "*" && !a.is_empty() && agent.starts_with(a.as_str()))
        };
        let star = |g: &&Group| g.0.iter().any(|a| a == "*");
        // RFC 9309: all groups matching the agent are merged; `*` only when
        // none name it.
        let chosen: Vec<_> = if groups.iter().any(|g| mine(&g)) {
            groups.iter().filter(mine).collect()
        } else {
            groups.iter().filter(star).collect()
        };
        Self {
            rules: chosen.iter().flat_map(|g| g.1.iter().cloned()).collect(),
            crawl_delay: chosen.iter().find_map(|g| g.2),
            disallow_all: false,
        }
    }

    /// Whether `path` (path plus `?query`) may be read. The longest
    /// matching rule wins; on a tie, Allow wins.
    pub fn allows(&self, path: &str) -> bool {
        if path == "/robots.txt" {
            return true;
        }
        if self.disallow_all {
            return false;
        }
        let mut best: Option<(usize, bool)> = None;
        for (allow, pattern) in &self.rules {
            if matches(pattern, path) {
                let len = pattern.len();
                best = match best {
                    Some((l, a)) if l > len || (l == len && a) => Some((l, a)),
                    _ => Some((len, *allow)),
                };
            }
        }
        best.is_none_or(|(_, allow)| allow)
    }
}

/// Matches a robots pattern: `*` is any run of characters, a trailing `$`
/// anchors the end, otherwise it is a prefix match.
fn matches(pattern: &str, path: &str) -> bool {
    let (pattern, anchored) = match pattern.strip_suffix('$') {
        Some(p) => (p, true),
        None => (pattern, false),
    };
    let parts: Vec<&str> = pattern.split('*').collect();
    let mut at = 0usize;
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            if !path.starts_with(part) {
                return false;
            }
            at = part.len();
            continue;
        }
        let last = i == parts.len() - 1;
        if last && anchored {
            return path.len() >= at + part.len() && path.ends_with(part);
        }
        match path[at..].find(part) {
            Some(pos) => at += pos + part.len(),
            None => return false,
        }
    }
    !anchored || at == path.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "\
# A typical file
User-agent: *
Disallow: /private/
Disallow: /*.pdf$
Allow: /private/open
Crawl-delay: 2

User-agent: BadBot
User-agent: OtherBot
Disallow: /
";

    #[test]
    fn the_star_group_applies_to_tern() {
        let r = Robots::parse(FILE, "Tern");
        assert!(r.allows("/"));
        assert!(r.allows("/papers/rag.html"));
        assert!(!r.allows("/private/notes"));
        assert!(r.allows("/private/open/page"), "longer Allow wins");
        assert!(!r.allows("/files/thesis.pdf"));
        assert!(r.allows("/files/thesis.pdf?x=1"), "$ anchors the end");
        assert_eq!(r.crawl_delay, Some(2.0));
        assert!(r.allows("/robots.txt"));
    }

    #[test]
    fn a_group_naming_tern_replaces_the_star_group() {
        let file = "User-agent: *\nDisallow: /\n\nUser-agent: tern\nDisallow: /drafts\n";
        let r = Robots::parse(file, "Tern");
        assert!(r.allows("/papers"));
        assert!(!r.allows("/drafts/1"));
        assert!(!Robots::parse(file, "OtherBot").allows("/papers"));
    }

    #[test]
    fn empty_disallow_and_failures() {
        assert!(Robots::parse("User-agent: *\nDisallow:\n", "Tern").allows("/x"));
        assert!(Robots::allow_all().allows("/anything"));
        assert!(!Robots::unreachable().allows("/anything"));
        assert!(Robots::unreachable().allows("/robots.txt"));
        // Equal length: Allow wins.
        assert!(Robots::parse("User-agent: *\nDisallow: /a\nAllow: /a\n", "Tern").allows("/a"));
        assert!(matches("/a*b*c", "/a-x-b-y-c-z"));
        assert!(!matches("/a*b$", "/a-b-c"));
    }
}
