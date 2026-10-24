//! Making skills from the Settings screen: a new skill from the editor
//! (name, description, Markdown body → `SKILL.md`), a skill imported from a
//! GitHub link (`owner/repo`, `owner/repo/path`, or a `github.com/…/tree/…`
//! URL; downloaded through `tern_crawl::github`, no git needed), and a few
//! templates to start from.

use std::path::Path;

use crate::skills::{self, FILE};

/// Where a GitHub import points.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GithubRef {
    pub owner: String,
    pub repo: String,
    /// A branch, tag, or commit; `None` is the default branch.
    pub reference: Option<String>,
    /// A folder (or `SKILL.md`) in the repository; empty is the root.
    pub path: String,
}

fn ok_part(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && s != "."
        && s != ".."
}

impl GithubRef {
    /// Reads what the user typed. Accepts `owner/repo[/path]`,
    /// `https://github.com/owner/repo[/tree|blob/<ref>/path]`, and
    /// `raw.githubusercontent.com/owner/repo/<ref>/path`. A path ending in
    /// `SKILL.md` means its folder.
    pub fn parse(input: &str) -> Result<Self, String> {
        let t = input.trim().trim_end_matches('/');
        let t = t.strip_suffix(".git").unwrap_or(t);
        let bad = || {
            format!(
                "“{}” is not a GitHub link. Try owner/repo, owner/repo/path/to/skill, or a github.com URL.",
                input.trim()
            )
        };
        let (rest, raw) = if let Some(r) = t
            .strip_prefix("https://github.com/")
            .or_else(|| t.strip_prefix("http://github.com/"))
            .or_else(|| t.strip_prefix("github.com/"))
        {
            (r, false)
        } else if let Some(r) = t
            .strip_prefix("https://raw.githubusercontent.com/")
            .or_else(|| t.strip_prefix("raw.githubusercontent.com/"))
        {
            (r, true)
        } else if t.contains("://") || t.starts_with("git@") {
            return Err(bad());
        } else {
            (t, false)
        };
        let parts: Vec<&str> = rest.split('/').filter(|p| !p.is_empty()).collect();
        if parts.len() < 2 || !ok_part(parts[0]) || !ok_part(parts[1]) {
            return Err(bad());
        }
        let (reference, path_parts): (Option<String>, &[&str]) = if raw {
            match parts.get(2) {
                Some(r) => (Some((*r).to_owned()), &parts[3..]),
                None => return Err(bad()),
            }
        } else if matches!(parts.get(2), Some(&"tree") | Some(&"blob")) {
            match parts.get(3) {
                Some(r) => (Some((*r).to_owned()), &parts[4..]),
                None => return Err(bad()),
            }
        } else {
            (None, &parts[2..])
        };
        if path_parts.iter().any(|p| *p == ".." || *p == ".") {
            return Err(bad());
        }
        let mut path = path_parts.join("/");
        if path == FILE || path.ends_with(&format!("/{FILE}")) {
            path = path.trim_end_matches(FILE).trim_end_matches('/').to_owned();
        }
        Ok(Self {
            owner: parts[0].to_owned(),
            repo: parts[1].to_owned(),
            reference,
            path,
        })
    }

    /// `owner/repo/path` for messages.
    pub fn label(&self) -> String {
        let mut s = format!("{}/{}", self.owner, self.repo);
        if !self.path.is_empty() {
            s.push('/');
            s.push_str(&self.path);
        }
        s
    }
}

/// Downloads the folder at `r` and installs the skills in it (each folder
/// with a `SKILL.md`). Returns the installed names.
pub async fn import_github(data: &Path, r: &GithubRef, replace: bool) -> Result<Vec<String>, String> {
    let files = tern_crawl::github::fetch_folder(&r.owner, &r.repo, r.reference.as_deref(), &r.path)
        .await
        .map_err(|e| e.to_string())?;
    if !files.iter().any(|(p, _)| p == FILE || p.ends_with(&format!("/{FILE}"))) {
        return Err(format!("{} has no {FILE}.", r.label()));
    }
    let tmp = skills::tempfile_dir::TempDir::new()?;
    // The folder is named for the skill so a root-level SKILL.md installs
    // under its own name.
    let base = tmp.path().join(if r.path.is_empty() {
        r.repo.as_str()
    } else {
        r.path.rsplit('/').next().unwrap_or("skill")
    });
    for (rel, bytes) in &files {
        if rel.split('/').any(|p| p == ".." || p.is_empty()) {
            continue;
        }
        let dest = base.join(rel);
        if let Some(d) = dest.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        std::fs::write(&dest, bytes).map_err(|e| e.to_string())?;
    }
    skills::install_folder(data, tmp.path(), replace)
}

/// The text of a `SKILL.md` from the editor's fields.
pub fn skill_md(name: &str, description: &str, body: &str) -> String {
    let desc = description.trim().replace(['\n', '\r'], " ");
    let desc = if desc.contains(':') || desc.contains('#') || desc.starts_with(['"', '\'']) {
        format!("\"{}\"", desc.replace('"', "'"))
    } else {
        desc
    };
    format!("---\nname: {}\ndescription: {desc}\n---\n\n{}\n", name.trim(), body.trim())
}

/// Checks the editor's fields; every problem, in words.
pub fn check_new(name: &str, description: &str, body: &str) -> Vec<String> {
    let mut e = Vec::new();
    if !skills::valid_name(name.trim()) {
        e.push("A skill name is lowercase letters, digits, and hyphens (up to 64), like study-summariser.".to_owned());
    }
    if description.trim().is_empty() {
        e.push("Say in one line when Tern should use it; the model reads this to decide.".to_owned());
    } else if description.trim().len() > 1024 {
        e.push("Keep the description under 1024 characters.".to_owned());
    }
    if body.trim().is_empty() {
        e.push("Write the instructions the model follows when it loads the skill.".to_owned());
    }
    e
}

/// Saves a new skill (or replaces one of yours when `replace`) as
/// `<data>/skills/<name>/SKILL.md`. The file is read back through the
/// skill parser, so what was saved is what Tern will load.
pub fn create(
    data: &Path,
    name: &str,
    description: &str,
    body: &str,
    replace: bool,
) -> Result<(), String> {
    let problems = check_new(name, description, body);
    if !problems.is_empty() {
        return Err(problems.join(" "));
    }
    let dir = skills::user_dir(data).join(name.trim());
    if dir.join(FILE).exists() && !replace {
        return Err(format!("A skill called “{}” already exists.", name.trim()));
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let text = skill_md(name, description, body);
    skills::parse(&text)?;
    std::fs::write(dir.join(FILE), text).map_err(|e| e.to_string())
}

/// A skill to start from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Template {
    pub name: &'static str,
    pub description: &'static str,
    pub body: &'static str,
}

pub const TEMPLATES: &[Template] = &[
    Template {
        name: "study-summariser",
        description: "Use when the user asks to summarise study material, a chapter, or lecture notes for revision.",
        body: "# Study summariser\n\n1. Read the material the user points to (attached files, vault notes, or Study materials).\n2. Write a summary in three parts:\n   - **Key ideas**: 5 to 8 bullet points, one idea each, in plain words.\n   - **Terms**: each new term with a one-line definition.\n   - **Check yourself**: 3 short questions the user should be able to answer.\n3. Cite the passage each key idea comes from.\n4. Keep it under one page. Do not add facts the material does not contain; if something important is missing, say so at the end.",
    },
    Template {
        name: "citation-checker",
        description: "Use when the user asks whether the citations in a draft, answer, or report actually support the claims.",
        body: "# Citation checker\n\nFor each sentence that carries a citation:\n\n1. Open the cited source (read_url or vault_read).\n2. Find the passage that would support the claim.\n3. Give a verdict: Supported, Partly supported, Not supported, or Source unreadable.\n4. Quote the passage (one or two sentences) and say what differs when it is not fully supported (a number, a scope, a date).\n\nEnd with a table: claim · source · verdict. Never mark a claim Supported without a quoted passage.",
    },
    Template {
        name: "paper-reader",
        description: "Use when the user shares a research paper and wants to understand its method, results, and limits.",
        body: "# Paper reader\n\nRead the paper and answer, each in a short section:\n\n- **Question**: what problem it tackles and why it matters.\n- **Method**: the approach, data, and setup, in plain words.\n- **Results**: the main numbers, with the table or figure they come from.\n- **Limits**: what the authors admit and what they leave out.\n- **Related**: two or three papers it builds on (from its references).\n\nCite page or section numbers. Mark your own judgement as yours.",
    },
    Template {
        name: "exam-drill",
        description: "Use when the user wants to practise for an exam with questions that get harder as they answer correctly.",
        body: "# Exam drill\n\n1. Ask which topic and how long the user has.\n2. Ask one question at a time, starting easy.\n3. After each answer: say if it is right, explain briefly, and cite the material.\n4. Two right in a row: make the next one harder. A wrong one: ask an easier question on the same idea.\n5. At the end, list the ideas to review.",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_links_in_every_common_form() {
        let r = GithubRef::parse("anthropics/skills").unwrap();
        assert_eq!((r.owner.as_str(), r.repo.as_str(), r.path.as_str()), ("anthropics", "skills", ""));
        assert_eq!(r.reference, None);
        let r = GithubRef::parse("anthropics/skills/document-skills/pdf").unwrap();
        assert_eq!(r.path, "document-skills/pdf");
        let r = GithubRef::parse("https://github.com/anthropics/skills/tree/main/skills/pdf/").unwrap();
        assert_eq!(r.reference.as_deref(), Some("main"));
        assert_eq!(r.path, "skills/pdf");
        let r = GithubRef::parse("https://github.com/o/r/blob/v2/x/SKILL.md").unwrap();
        assert_eq!((r.reference.as_deref(), r.path.as_str()), (Some("v2"), "x"));
        let r = GithubRef::parse("https://raw.githubusercontent.com/o/r/main/SKILL.md").unwrap();
        assert_eq!((r.reference.as_deref(), r.path.as_str()), (Some("main"), ""));
        let r = GithubRef::parse("https://github.com/o/r.git").unwrap();
        assert_eq!(r.repo, "r");
        assert_eq!(r.label(), "o/r");
        for bad in ["", "justone", "https://gitlab.com/o/r", "o/../r", "o/r/../../etc", "git@github.com:o/r", "https://github.com/o/r/tree"] {
            assert!(GithubRef::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn new_skills_save_as_skill_md_and_load_back() {
        let dir = tempfile::tempdir().unwrap();
        assert!(create(dir.path(), "Bad Name", "", "", false).is_err());
        let e = check_new("x", " ", " ");
        assert_eq!(e.len(), 2);
        create(dir.path(), "citation-checker", "Use when: checking citations", "Do it.", false).unwrap();
        let file = skills::user_dir(dir.path()).join("citation-checker").join(FILE);
        let (keys, body) = skills::parse(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(keys["name"], "citation-checker");
        assert_eq!(keys["description"], "Use when: checking citations");
        assert_eq!(body.trim(), "Do it.");
        assert!(create(dir.path(), "citation-checker", "d", "b", false).is_err());
        create(dir.path(), "citation-checker", "d", "b2", true).unwrap();
        // Templates are valid skills.
        for t in TEMPLATES {
            assert!(check_new(t.name, t.description, t.body).is_empty(), "{}", t.name);
            skills::parse(&skill_md(t.name, t.description, t.body)).unwrap();
        }
    }
}
