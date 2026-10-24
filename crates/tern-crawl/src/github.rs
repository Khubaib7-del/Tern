//! Downloads one folder of a public GitHub repository through GitHub's
//! contents API, for importing a skill (a folder with a `SKILL.md`) without
//! git. Small by design: at most [`MAX_FILES`] files and [`MAX_BYTES`] in
//! all, folders [`MAX_DEPTH`] deep, text-sized files only.

use std::time::Duration;

use serde_json::Value;

/// The most files one import downloads.
pub const MAX_FILES: usize = 60;
/// The most bytes one import downloads.
pub const MAX_BYTES: usize = 2 * 1024 * 1024;
/// How deep below the folder it goes.
pub const MAX_DEPTH: usize = 3;

const API: &str = "https://api.github.com";

/// What went wrong, in words for the import dialog.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GithubError {
    #[error("GitHub has no {0} (is the repository public and the path right?)")]
    NotFound(String),
    #[error("GitHub refused: HTTP {0}{1}")]
    Status(u16, &'static str),
    #[error("could not reach GitHub: {0}")]
    Network(String),
    #[error("the folder is larger than Tern imports ({0})")]
    TooBig(&'static str),
    #[error("GitHub sent an answer Tern could not read")]
    Bad,
}

/// One downloaded file: its path inside the folder and its bytes.
pub type File = (String, Vec<u8>);

/// Downloads `path` (a folder or one file) of `owner/repo` at `reference`
/// (a branch, tag, or commit; `None` is the default branch).
pub async fn fetch_folder(
    owner: &str,
    repo: &str,
    reference: Option<&str>,
    path: &str,
) -> Result<Vec<File>, GithubError> {
    fetch_folder_at(API, owner, repo, reference, path).await
}

/// [`fetch_folder`] against another API address (tests).
pub async fn fetch_folder_at(
    api: &str,
    owner: &str,
    repo: &str,
    reference: Option<&str>,
    path: &str,
) -> Result<Vec<File>, GithubError> {
    let local = api.contains("127.0.0.1") || api.contains("localhost");
    let mut b = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent(concat!("Tern/", env!("CARGO_PKG_VERSION")));
    if local {
        b = b.no_proxy();
    }
    let http = b.build().map_err(|e| GithubError::Network(e.to_string()))?;
    let mut out = Vec::new();
    let mut total = 0usize;
    let mut stack = vec![(path.trim_matches('/').to_owned(), 0usize)];
    let root = path.trim_matches('/').to_owned();
    while let Some((dir, depth)) = stack.pop() {
        let mut url = format!(
            "{}/repos/{owner}/{repo}/contents/{dir}",
            api.trim_end_matches('/')
        );
        if let Some(r) = reference {
            url.push_str(&format!("?ref={r}"));
        }
        let v = get_json(&http, &url, &dir).await?;
        let entries: Vec<Value> = match v {
            Value::Array(a) => a,
            one @ Value::Object(_) => vec![one],
            _ => return Err(GithubError::Bad),
        };
        for e in entries {
            let kind = e["type"].as_str().unwrap_or_default();
            let full = e["path"].as_str().unwrap_or_default().to_owned();
            let rel = full
                .strip_prefix(&root)
                .unwrap_or(&full)
                .trim_start_matches('/')
                .to_owned();
            let rel = if rel.is_empty() {
                e["name"].as_str().unwrap_or("SKILL.md").to_owned()
            } else {
                rel
            };
            match kind {
                "dir" if depth < MAX_DEPTH => stack.push((full, depth + 1)),
                "file" => {
                    let size = e["size"].as_u64().unwrap_or(0) as usize;
                    total += size;
                    if out.len() >= MAX_FILES {
                        return Err(GithubError::TooBig("too many files"));
                    }
                    if total > MAX_BYTES {
                        return Err(GithubError::TooBig("over 2 MB"));
                    }
                    let Some(dl) = e["download_url"].as_str() else {
                        continue;
                    };
                    let bytes = get_bytes(&http, dl, &rel).await?;
                    out.push((rel, bytes));
                }
                _ => {}
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

async fn get_json(http: &reqwest::Client, url: &str, what: &str) -> Result<Value, GithubError> {
    let resp = http
        .get(url)
        .header("accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|_| GithubError::Network("the request did not reach it".into()))?;
    check(resp.status().as_u16(), what)?;
    resp.json().await.map_err(|_| GithubError::Bad)
}

async fn get_bytes(http: &reqwest::Client, url: &str, what: &str) -> Result<Vec<u8>, GithubError> {
    let resp = http
        .get(url)
        .send()
        .await
        .map_err(|_| GithubError::Network("the request did not reach it".into()))?;
    check(resp.status().as_u16(), what)?;
    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|_| GithubError::Bad)
}

fn check(status: u16, what: &str) -> Result<(), GithubError> {
    match status {
        200..=299 => Ok(()),
        404 => Err(GithubError::NotFound(if what.is_empty() {
            "such repository".into()
        } else {
            format!("“{what}”")
        })),
        403 | 429 => Err(GithubError::Status(
            status,
            " (the hourly limit for unsigned requests; try again later)",
        )),
        s => Err(GithubError::Status(s, "")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test(flavor = "multi_thread")]
    async fn downloads_a_skill_folder() {
        let s = MockServer::start().await;
        let u = s.uri();
        Mock::given(path("/repos/o/r/contents/skills/cite"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"type": "file", "path": "skills/cite/SKILL.md", "name": "SKILL.md", "size": 20,
                 "download_url": format!("{u}/raw/SKILL.md")},
                {"type": "dir", "path": "skills/cite/ref", "name": "ref"}
            ])))
            .mount(&s)
            .await;
        Mock::given(path("/repos/o/r/contents/skills/cite/ref"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"type": "file", "path": "skills/cite/ref/a.md", "name": "a.md", "size": 3,
                 "download_url": format!("{u}/raw/a.md")}
            ])))
            .mount(&s)
            .await;
        Mock::given(path("/raw/SKILL.md"))
            .respond_with(ResponseTemplate::new(200).set_body_string("---\nname: cite\n---\n"))
            .mount(&s)
            .await;
        Mock::given(path("/raw/a.md"))
            .respond_with(ResponseTemplate::new(200).set_body_string("abc"))
            .mount(&s)
            .await;
        let files = fetch_folder_at(&u, "o", "r", None, "/skills/cite/")
            .await
            .unwrap();
        let names: Vec<&str> = files.iter().map(|f| f.0.as_str()).collect();
        assert_eq!(names, vec!["SKILL.md", "ref/a.md"]);
        assert_eq!(files[1].1, b"abc");
        let e = fetch_folder_at(&u, "o", "missing", None, "x").await.unwrap_err();
        assert!(matches!(e, GithubError::NotFound(_)), "{e}");
    }
}
