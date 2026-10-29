//! How the model asks for a tool in phase 3.
//!
//! Every provider Tern supports, local servers included, can follow a plain
//! text convention, while native tool calling differs per wire format and is
//! missing on many local models. So for now the model asks for a tool with a
//! fenced block:
//!
//! ````text
//! ```tool
//! {"tool": "web_search", "args": {"query": "attention is all you need"}}
//! ```
//! ````
//!
//! Native tool calls can replace this later without touching the loop, which
//! only sees [`Reply`].

use serde_json::Value;

use crate::tools::ToolRegistry;

/// What one model reply asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum Reply {
    /// A final answer for the user.
    Answer(String),
    Tool {
        name: String,
        args: Value,
    },
    /// A tool block Tern could not read. The text goes back to the model.
    Malformed(String),
}

const FENCE: &str = "```tool";

pub fn parse(text: &str) -> Reply {
    let Some(start) = text.find(FENCE) else {
        return Reply::Answer(text.trim().to_owned());
    };
    let body = &text[start + FENCE.len()..];
    let Some(end) = body.find("```") else {
        return Reply::Malformed("the tool block was not closed with ```".into());
    };
    let json: Value = match serde_json::from_str(body[..end].trim()) {
        Ok(v) => v,
        Err(e) => return Reply::Malformed(format!("the tool block is not valid JSON ({e})")),
    };
    let Some(name) = json.get("tool").and_then(Value::as_str) else {
        return Reply::Malformed("the tool block needs a \"tool\" name".into());
    };
    Reply::Tool {
        name: name.to_owned(),
        args: json
            .get("args")
            .cloned()
            .unwrap_or(Value::Object(Default::default())),
    }
}

/// Every tool block in a reply, in order, as `(name, args)`: a provider's
/// parallel tool calls come back as one block each. Blocks Tern cannot
/// read are left out (the first one is reported by [`parse`]).
pub fn parse_calls(text: &str) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(FENCE) {
        let body = &rest[start + FENCE.len()..];
        let Some(end) = body.find("```") else {
            break;
        };
        if let Ok(json) = serde_json::from_str::<Value>(body[..end].trim())
            && let Some(name) = json.get("tool").and_then(Value::as_str)
        {
            out.push((
                name.to_owned(),
                json.get("args")
                    .cloned()
                    .unwrap_or(Value::Object(Default::default())),
            ));
        }
        rest = &body[end + 3..];
    }
    out
}

/// Tern's opening line in every system prompt.
pub const BASE: &str = "You are Tern, a careful research and study assistant. Answer plainly and say when you are unsure.";

/// How to ask for a tool, after the tool list.
pub const TOOL_PROTOCOL: &str = "To use one, reply with only this block and nothing else:\n```tool\n{\"tool\": \"<name>\", \"args\": {...}}\n```\nYou will get the result in the next message. To run several independent calls at once (for example two searches), put one block after another in the same reply; all the results come back together. When you have enough, reply with the final answer and no tool block.\nOnly use and mention the tools listed above. A tool that changes something (a file write, edit or rename, a command) shows the user an approval card with the diff or the old and new path, and that card is the confirmation: when the user asks for a change, call the tool directly, never ask first in prose (no \"Shall I save it?\", no \"Would you like me to proceed?\", no numbered options). To add or change a few lines use `file_edit` when it is listed, not a rewrite. When the user asks to read, show or open a file, call `file_read` even if you wrote or read it earlier (it is cheap); never answer from memory. Rename or move a file with `file_rename` when it is listed, never by writing a copy. Never say a file was saved, renamed or a command ran unless its tool result says so; when a tool failed, was declined or was stopped, say exactly what its result says. To read a file or list a folder outside the working folder, call `file_read` or `files_list` with its full path: Tern asks the user, so do not refuse it.";

/// The system prompt for one turn. The effective instructions view
/// ([`crate::rules::effective`]) is built from this same function.
pub fn system_prompt(base: Option<&str>, tools: &ToolRegistry) -> String {
    let mut s = String::from(BASE);
    if let Some(b) = base {
        s.push_str("\n\n");
        s.push_str(b);
    }
    if !tools.is_empty() {
        s.push_str("\n\nYou can use these tools:\n");
        s.push_str(&tools.describe());
        s.push_str("\n\n");
        s.push_str(TOOL_PROTOCOL);
        if tools.get("run_python").is_some() {
            let stubs = crate::python::stubs(tools);
            if !stubs.is_empty() {
                s.push_str("\n\n");
                s.push_str(CODE_MODE);
                s.push('\n');
                s.push_str(&crate::python::stub_text(&stubs));
                let example = crate::python::example(&stubs);
                if !example.is_empty() {
                    s.push_str("\nFor example:\n");
                    s.push_str(&example);
                }
            }
        }
    }
    s
}

/// Code mode (#77), after the tool protocol when `run_python` is offered;
/// the stubs follow it.
pub const CODE_MODE: &str = "Code mode: `run_python` has no file system, OS or network access (open(), pathlib, os do not work); inside it these tools are Python functions that return text, the only way to reach files and the web, so one script can call many tools and only what it prints comes back to you (print a short summary). Each call still asks the user when needed:";

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_block_in_a_reply_is_a_call() {
        let text = format!(
            "{}\n{}\n```tool\nnot json\n```",
            tern_providers::tool_block("web_search", &json!({"query": "a"})),
            tern_providers::tool_block("read_url", &json!({"url": "https://x.org"}))
        );
        assert_eq!(
            parse_calls(&text),
            vec![
                ("web_search".to_owned(), json!({"query": "a"})),
                ("read_url".to_owned(), json!({"url": "https://x.org"}))
            ]
        );
        assert!(parse_calls("no tools").is_empty());
    }

    #[test]
    fn plain_text_is_an_answer() {
        assert_eq!(parse("  Paris.  "), Reply::Answer("Paris.".into()));
    }

    #[test]
    fn reads_a_tool_block_with_text_around_it() {
        let r = parse(
            "Let me look.\n```tool\n{\"tool\": \"web_search\", \"args\": {\"query\": \"x\"}}\n```\n",
        );
        assert_eq!(
            r,
            Reply::Tool {
                name: "web_search".into(),
                args: json!({"query": "x"})
            }
        );
    }

    #[test]
    fn missing_args_default_to_an_empty_object() {
        assert_eq!(
            parse("```tool\n{\"tool\": \"now\"}\n```"),
            Reply::Tool {
                name: "now".into(),
                args: json!({})
            }
        );
    }

    #[test]
    fn broken_blocks_are_malformed_not_answers() {
        assert!(matches!(
            parse("```tool\n{\"tool\": \"x\""),
            Reply::Malformed(_)
        ));
        assert!(matches!(
            parse("```tool\nnot json\n```"),
            Reply::Malformed(_)
        ));
        assert!(matches!(
            parse("```tool\n{\"args\": {}}\n```"),
            Reply::Malformed(_)
        ));
    }
}
