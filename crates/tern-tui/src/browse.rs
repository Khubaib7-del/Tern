//! `tern browse`: the built-in browser from the terminal. An address prints
//! the page as Tern's reader shows it and Tern's explanation of it with
//! numbered citations; words print the search service's results. `--open`
//! also shows the real page in the browser helper's window
//! (`docs/decisions/0011-browser-helper-process.md`).

use std::io::IsTerminal as _;

use tern_agent::browse::{self, Target};

use crate::ChatArgs;

/// Where the helper is: `TERN_BROWSER_HELPER`, else next to `tern`.
fn helper() -> Option<std::path::PathBuf> {
    if let Some(p) = std::env::var_os("TERN_BROWSER_HELPER") {
        return Some(p.into());
    }
    let exe = std::env::current_exe().ok()?;
    let p = exe
        .parent()?
        .join(tern_core::browser_ipc::helper_file_name());
    p.exists().then_some(p)
}

/// Opens the real page in its own window and returns at once.
fn open_window(url: &str) -> Result<(), String> {
    let exe = helper().ok_or_else(|| {
        format!(
            "The browser helper ({}) is not next to tern, so the page cannot open in a window.",
            tern_core::browser_ipc::helper_file_name()
        )
    })?;
    let mut cmd = std::process::Command::new(&exe);
    cmd.args(["--standalone", "--url", url]);
    if let Ok(p) = tern_agent::platform::AppPaths::discover() {
        cmd.arg("--data-dir").arg(p.cache_dir.join("webview"));
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not start {}: {e}", exe.display()))
}

fn note(line: &str) {
    if std::io::stderr().is_terminal() {
        eprintln!("\x1b[2m• {line}\x1b[0m");
    } else {
        eprintln!("• {line}");
    }
}

/// Runs `tern browse`.
pub async fn run(
    input: String,
    chat: ChatArgs,
    explain: bool,
    open: bool,
    max_chars: usize,
) -> Result<(), String> {
    let target = browse::target(&input)
        .ok_or("Give a web address, like arxiv.org/abs/1706.03762, or words to search for.")?;
    match target {
        Target::Search(q) => {
            note(&format!("Searching for {q}"));
            let page = browse::search(&q).await?;
            println!("{}", page.markdown.trim());
            if open {
                open_window(&crate::browse::engine_address(&q))?;
                note("Opened the results in the browser window");
            }
            Ok(())
        }
        Target::Page(url) => {
            if open {
                open_window(&url)?;
                note("Opened the page in the browser window");
            }
            note(&format!("Reading {}", browse::display_address(&url)));
            let page = browse::open(&url).await?;
            let links = std::io::stdout().is_terminal();
            let title = if links {
                format!(
                    "\x1b]8;;{}\x1b\\{}\x1b]8;;\x1b\\",
                    page.page.url, page.page.title
                )
            } else {
                page.page.title.clone()
            };
            println!("# {title}");
            println!("{} · {}", page.page.site, page.page.url);
            println!();
            let text: String = page.markdown.chars().take(max_chars).collect();
            println!("{}", text.trim());
            if page.truncated || text.len() < page.markdown.len() {
                println!("\n[… the page goes on; read it whole with --max-chars or --open]");
            }
            if !explain {
                return Ok(());
            }
            note("Explaining the page");
            let opts = tern_agent::AskOptions {
                provider: chat.provider,
                model: chat.model,
                base_url: chat.base_url,
                ..Default::default()
            };
            match tern_agent::ask::explain_page(
                &page.page.title,
                &page.page.url,
                &page.markdown,
                &opts,
            )
            .await
            {
                Ok(x) => {
                    println!("\n## Explanation\n");
                    println!("{}", x.answer);
                    println!();
                    println!(
                        "Citations [1]–[{}] are parts of {} in order · {} · {}",
                        x.parts, page.page.url, x.provider, x.model
                    );
                    if !x.invalid_citations.is_empty() {
                        let nums: Vec<String> = x
                            .invalid_citations
                            .iter()
                            .map(|n| format!("[{n}]"))
                            .collect();
                        println!("{} match no part of the page", nums.join(" "));
                    }
                }
                Err(e) => note(&format!("No explanation: {e}")),
            }
            Ok(())
        }
    }
}

/// The address a search opens on in a real browser.
pub fn engine_address(q: &str) -> String {
    let enc: String = q
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            b' ' => "+".to_owned(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    format!("https://duckduckgo.com/?q={enc}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn searches_open_on_an_encoded_address() {
        assert_eq!(
            engine_address("rust gpui & wry"),
            "https://duckduckgo.com/?q=rust+gpui+%26+wry"
        );
    }
}
