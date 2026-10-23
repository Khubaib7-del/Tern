//! The composer's `/` menu, kept apart from GPUI so it can be tested
//! without a display: Tern's own commands, the user's skills, and the
//! prompts MCP servers offer, each row with its logo (the tern mark for
//! Tern's own, the server's logo for a server's), name, description, and
//! source. A prompt with arguments opens a small form first; a prompt's
//! filled-in text goes into the composer under a banner naming its
//! server, and is never sent until the user sends it.

use std::collections::BTreeMap;

use tern_agent::mcp::{Inserted, ItemKind, MenuItem};

use crate::extensions::{ServerGlyph, server_glyph};

/// Tern's own commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TernCommand {
    New,
    Deep,
    Library,
    Editor,
    Git,
    Browser,
    Customize,
    Instructions,
    Providers,
    Appearance,
    Decision,
}

impl TernCommand {
    pub const ALL: [Self; 11] = [
        Self::New,
        Self::Deep,
        Self::Library,
        Self::Editor,
        Self::Git,
        Self::Browser,
        Self::Customize,
        Self::Instructions,
        Self::Providers,
        Self::Appearance,
        Self::Decision,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Deep => "deep",
            Self::Library => "library",
            Self::Editor => "editor",
            Self::Git => "git",
            Self::Browser => "browser",
            Self::Customize => "customize",
            Self::Instructions => "instructions",
            Self::Providers => "providers",
            Self::Appearance => "appearance",
            Self::Decision => "decision",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::New => "Start a new chat",
            Self::Deep => "Turn deep research on or off for the next message",
            Self::Library => "Open the Library",
            Self::Editor => "Open the vault editor",
            Self::Git => "Open Git for the vault",
            Self::Browser => "Show or hide the built-in browser",
            Self::Customize => "Skills, MCP servers, providers, instructions",
            Self::Instructions => "Exactly what the model is told",
            Self::Providers => "Providers and keys",
            Self::Appearance => "Palette, type, and glyphs",
            Self::Decision => "The decision model",
        }
    }
}

/// The logo at the start of a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowLogo {
    /// The tern mark: Tern's own commands and skills.
    Tern,
    /// A server's logo, or its transport glyph.
    Server(ServerGlyph),
}

/// The logo for a server row, by the same rule as the Customize page's
/// server rows.
pub fn server_logo(item: &MenuItem) -> RowLogo {
    RowLogo::Server(server_glyph(&item.server, &item.target, item.http))
}

/// What a `/` row picks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SlashPick {
    Command(TernCommand),
    /// A skill by name: asks the model to use it.
    Skill(String),
    Prompt(Box<MenuItem>),
}

/// One row of the `/` menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlashRow {
    pub pick: SlashPick,
    pub logo: RowLogo,
    /// `/new`, `/review`.
    pub label: String,
    pub description: String,
    /// `Tern`, `Skill`, or the server's name.
    pub source: String,
}

/// The `/` menu: what is typed after `/` and which row is highlighted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SlashMenu {
    pub query: String,
    pub selected: usize,
}

const MENU_ROWS: usize = 9;

/// The `/` being typed: the composer holds exactly `/word`.
pub fn active(text: &str) -> Option<&str> {
    let q = text.strip_prefix('/')?;
    (!q.chars().any(char::is_whitespace)).then_some(q)
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Every row matching `query`, best first: Tern's commands, then skills,
/// then MCP prompts, by how well the name matches.
pub fn rows(query: &str, skills: &[(String, String)], offers: &[MenuItem]) -> Vec<SlashRow> {
    use tern_agent::mention::{matches, rank};
    let mut out: Vec<(u8, u8, SlashRow)> = Vec::new();
    for c in TernCommand::ALL {
        if matches(query, c.name()) {
            out.push((
                rank(query, c.name()),
                0,
                SlashRow {
                    pick: SlashPick::Command(c),
                    logo: RowLogo::Tern,
                    label: format!("/{}", c.name()),
                    description: c.description().into(),
                    source: "Tern".into(),
                },
            ));
        }
    }
    for (name, description) in skills {
        if matches(query, name) {
            out.push((
                rank(query, name),
                1,
                SlashRow {
                    pick: SlashPick::Skill(name.clone()),
                    logo: RowLogo::Tern,
                    label: format!("/{name}"),
                    description: one_line(description),
                    source: "Skill".into(),
                },
            ));
        }
    }
    for item in offers.iter().filter(|i| i.kind == ItemKind::Prompt) {
        if matches(query, &item.name) || matches(query, item.label()) {
            let mut description = one_line(&item.description);
            if !item.args.is_empty() {
                let args: Vec<String> = item
                    .args
                    .iter()
                    .map(|a| {
                        if a.required {
                            a.name.clone()
                        } else {
                            format!("[{}]", a.name)
                        }
                    })
                    .collect();
                description = if description.is_empty() {
                    args.join(" ")
                } else {
                    format!("{} · {description}", args.join(" "))
                };
            }
            out.push((
                rank(query, &item.name).min(rank(query, item.label())),
                2,
                SlashRow {
                    logo: server_logo(item),
                    label: format!("/{}", item.name),
                    description,
                    source: item.server.clone(),
                    pick: SlashPick::Prompt(Box::new(item.clone())),
                },
            ));
        }
    }
    out.sort_by_key(|(r, k, _)| (*r, *k));
    out.into_iter().map(|(_, _, r)| r).take(MENU_ROWS).collect()
}

/// What the form is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormFor {
    /// Fill in a prompt, then put its text in the composer.
    Prompt,
    /// Fill in a resource template, then attach what it reads.
    Resource,
}

/// The small form a prompt's arguments (or a template's parts) are typed
/// into, under the composer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArgForm {
    pub item: MenuItem,
    pub purpose: FormFor,
    /// A line for the user when submitting failed (a required argument).
    pub error: Option<String>,
}

impl ArgForm {
    pub fn new(item: MenuItem, purpose: FormFor) -> Self {
        Self {
            item,
            purpose,
            error: None,
        }
    }

    /// The values typed, by argument name, leaving out empty optional
    /// ones. Errors name the required arguments left empty.
    pub fn values(&self, typed: &[String]) -> Result<BTreeMap<String, String>, String> {
        let mut out = BTreeMap::new();
        let mut missing = Vec::new();
        for (a, v) in self.item.args.iter().zip(
            typed
                .iter()
                .map(String::as_str)
                .chain(std::iter::repeat("")),
        ) {
            let v = v.trim();
            if v.is_empty() {
                if a.required {
                    missing.push(a.name.as_str());
                }
            } else {
                out.insert(a.name.clone(), v.to_owned());
            }
        }
        if missing.is_empty() {
            Ok(out)
        } else {
            Err(format!("Fill in {}.", missing.join(", ")))
        }
    }
}

/// The banner over composer text a server wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InsertedBanner {
    pub inserted: Inserted,
    pub logo: RowLogo,
}

impl InsertedBanner {
    pub fn line(&self) -> String {
        format!(
            "From {} · /{}. This is the server's text: read it before you send it.",
            self.inserted.server, self.inserted.prompt
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tern_agent::mcp::PromptArg;

    fn prompt(server: &str, name: &str, args: &[(&str, bool)]) -> MenuItem {
        MenuItem {
            server: server.into(),
            kind: ItemKind::Prompt,
            name: name.into(),
            title: None,
            description: "Does a thing.".into(),
            uri: String::new(),
            mime: None,
            args: args
                .iter()
                .map(|(n, r)| PromptArg {
                    name: (*n).into(),
                    title: None,
                    description: String::new(),
                    required: *r,
                })
                .collect(),
            http: false,
            target: format!("npx {server}-mcp"),
        }
    }

    #[test]
    fn slash_lists_tern_skills_and_prompts_each_with_a_logo() {
        assert_eq!(active("/rev"), Some("rev"));
        assert_eq!(active("/rev x"), None);
        assert_eq!(active("hi /x"), None);
        let skills = vec![("apa-style".to_owned(), "Cite in APA.".to_owned())];
        let offers = vec![
            prompt("github", "review", &[("code", true), ("focus", false)]),
            prompt("notes", "recap", &[]),
        ];
        let all = rows("", &skills, &offers);
        assert!(
            all.iter()
                .any(|r| r.label == "/new" && r.logo == RowLogo::Tern)
        );
        assert!(
            all.iter()
                .any(|r| r.source == "Skill" && r.logo == RowLogo::Tern)
        );
        let review = rows("rev", &skills, &offers);
        assert_eq!(review[0].label, "/review");
        assert_eq!(review[0].source, "github");
        assert_eq!(
            review[0].logo,
            RowLogo::Server(ServerGlyph::Service("github"))
        );
        assert!(review[0].description.starts_with("code [focus]"));
        let recap = rows("recap", &skills, &offers);
        assert_eq!(recap[0].logo, RowLogo::Server(ServerGlyph::Terminal));
    }

    #[test]
    fn forms_need_their_required_arguments() {
        let f = ArgForm::new(
            prompt("x", "review", &[("code", true), ("focus", false)]),
            FormFor::Prompt,
        );
        assert_eq!(f.values(&["".into()]).unwrap_err(), "Fill in code.");
        let v = f.values(&["fn x()".into(), " ".into()]).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v["code"], "fn x()");
    }
}
