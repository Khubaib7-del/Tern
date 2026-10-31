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
    let (name, args) = unwrap_call(
        name.to_owned(),
        json.get("args")
            .cloned()
            .unwrap_or(Value::Object(Default::default())),
    );
    Reply::Tool { name, args }
}

/// The real call inside a wrapped one (#79). Some models put the whole
/// call where its name goes: a tool named `tool` whose arguments are
/// `{"tool": "file_read", "args": {...}}` (codestral), or a name that is
/// itself that JSON object, sometimes with a stray `}` after it
/// (ministral). Anything else is returned as it came.
pub fn unwrap_call(name: String, args: Value) -> (String, Value) {
    let mut name = name;
    let mut args = args;
    // Twice at most: a wrapper inside a wrapper.
    for _ in 0..2 {
        let trimmed = name.trim();
        if trimmed.starts_with('{') {
            let mut it = serde_json::Deserializer::from_str(trimmed).into_iter::<Value>();
            let Some(Ok(inner)) = it.next() else {
                break;
            };
            let Some(real) = call_name(&inner) else {
                break;
            };
            let inner_args = call_args(&inner);
            let empty = args.as_object().is_none_or(|o| o.is_empty());
            if let Some(a) = inner_args.filter(|_| empty) {
                args = a;
            }
            name = real;
            continue;
        }
        if matches!(trimmed, "tool" | "tool_call" | "function" | "call")
            && let Some(real) = call_name(&args)
        {
            let inner = call_args(&args).unwrap_or(Value::Object(Default::default()));
            name = real;
            args = inner;
            continue;
        }
        break;
    }
    let args = normalize_args(&name, args);
    (name, args)
}

/// A call's arguments as the tool's schema expects them (#85, A6): a JSON
/// string holding an object is read as that object, and an envelope
/// around them (`{"tool": "<this tool>", "args": {...}}`, or a lone
/// `args` / `arguments` / `parameters` key) is opened. Anything else is
/// returned as it came, for the schema check to judge.
pub fn normalize_args(name: &str, args: Value) -> Value {
    let mut args = args;
    for _ in 0..3 {
        args = match args {
            Value::String(s) => match serde_json::from_str::<Value>(s.trim()) {
                Ok(v) if v.is_object() || v.is_string() => v,
                _ => return Value::String(s),
            },
            Value::Null => return Value::Object(Default::default()),
            Value::Object(o) => {
                let named = call_name(&Value::Object(o.clone()));
                let wrapper = ["args", "arguments", "parameters"]
                    .iter()
                    .filter(|k| o.contains_key(**k))
                    .count()
                    == 1;
                let only_envelope_keys = o.keys().all(|k| {
                    matches!(
                        k.as_str(),
                        "tool" | "name" | "args" | "arguments" | "parameters"
                    )
                });
                let same_tool = named.as_deref().is_none_or(|n| n == name);
                if wrapper && only_envelope_keys && same_tool {
                    match call_args(&Value::Object(o.clone())) {
                        Some(inner) => inner,
                        None => return Value::Object(o),
                    }
                } else {
                    return Value::Object(o);
                }
            }
            other => return other,
        };
    }
    args
}

/// The tool a wrapped call names (`tool` or `name`).
fn call_name(v: &Value) -> Option<String> {
    ["tool", "name"]
        .iter()
        .find_map(|k| v.get(*k).and_then(Value::as_str))
        .map(|n| n.trim().to_owned())
        .filter(|n| !n.is_empty())
}

/// A wrapped call's arguments (`args`, `arguments` or `parameters`); a
/// JSON string holding an object is read as that object.
fn call_args(v: &Value) -> Option<Value> {
    let a = ["args", "arguments", "parameters"]
        .iter()
        .find_map(|k| v.get(*k))?;
    match a {
        Value::String(s) => serde_json::from_str::<Value>(s)
            .ok()
            .filter(Value::is_object),
        Value::Object(_) => Some(a.clone()),
        _ => None,
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
            out.push(unwrap_call(
                name.to_owned(),
                ["args", "arguments", "parameters"]
                    .iter()
                    .find_map(|k| json.get(*k).cloned())
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
    system_prompt_for(base, tools, false)
}

/// [`system_prompt`] for a request that also sends the tools natively
/// (#85): the tool list carries no input schemas, which the request
/// already holds, so they are not paid for twice.
pub fn system_prompt_native(base: Option<&str>, tools: &ToolRegistry) -> String {
    system_prompt_for(base, tools, true)
}

/// A short hash of a system prompt, for the turn fingerprint (#85, A7).
pub fn prompt_hash(text: &str) -> String {
    use sha2::{Digest, Sha256};
    let d = Sha256::digest(text.as_bytes());
    d.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// The version of Tern's base prompt, for the turn fingerprint.
pub const PROMPT_VERSION: &str = "core-0";

fn system_prompt_for(base: Option<&str>, tools: &ToolRegistry, native: bool) -> String {
    let mut s = String::from(BASE);
    s.push(' ');
    s.push_str(RENDERING);
    if let Some(b) = base {
        s.push_str("\n\n");
        s.push_str(b);
    }
    if !tools.is_empty() {
        s.push_str("\n\nYou can use these tools:\n");
        s.push_str(&if native {
            tools.describe_brief()
        } else {
            tools.describe()
        });
        s.push_str("\n\n");
        s.push_str(TOOL_PROTOCOL);
        if tools.get("scholar_search").is_some() {
            s.push_str("\n\n");
            s.push_str(SCHOLARLY);
        }
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

/// What Tern draws from an answer's Markdown (#80: asked for a
/// flowchart, a model said it had no tools to draw one).
pub const RENDERING: &str = "Tern renders Markdown: tables, math ($...$, $$...$$) and ```mermaid fenced blocks, drawn as diagrams. To draw a diagram, flowchart, sequence, mind map or chart, answer with a ```mermaid block; never say you cannot draw.";

/// Scholarly search (#82), after the tool protocol when `scholar_search`
/// is offered.
pub const SCHOLARLY: &str = "For academic, scientific or medical questions, and any question about papers, studies, research methods or a named technique (for example FLARE, LoRA, ReAct), call `scholar_search` first (web search second), read a paper's text with `paper_read`, and widen a reading list with `paper_snowball`. Cite papers by their link and quote passages with their locator, for example (p. 4, §3 Method). Say what a paper finds only from text `paper_read` returned; a paper read as \"Abstract only\" is described \"according to its abstract\", and a paper you only saw listed is described by its title alone, never as discussing or showing anything. Leave a paper's own reference numbers such as [17] out of quotes: [n] cites only Tern's sources. When the papers found do not support a claim, say \"not enough evidence\" for it; never present an answer from memory as sourced, and never expand an acronym from memory.";

/// Code mode (#77), after the tool protocol when `run_python` is offered;
/// the stubs follow it.
pub const CODE_MODE: &str = "Code mode: `run_python` has no file system, OS or network access (open() and os do not work; pathlib's read_text, write_text and iterdir go through the file tools); inside it these tools are Python functions, already defined (never define or assign them yourself), that return what each line says (text unless noted). They are the only way to reach files and the web, so one script can call many tools and only what it prints comes back to you (print a short summary). A failed call raises ToolError with the reason; never print that something was saved unless the call returned. Each call still asks the user when needed:";

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_prompt_says_mermaid_blocks_are_drawn() {
        // With tools and without: "draw a flowchart" is a ```mermaid block.
        let none = system_prompt(None, &ToolRegistry::new());
        assert!(none.starts_with(BASE));
        assert!(
            none.contains("```mermaid fenced blocks, drawn as diagrams"),
            "{none}"
        );
        assert!(none.contains("never say you cannot draw"));
    }

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
    fn string_and_envelope_arguments_are_opened() {
        // A-T6 (#85): arguments sent as a JSON string.
        let r = "```tool\n{\"tool\": \"read_url\", \"args\": \"{\\\"url\\\": \\\"https://a.org\\\"}\"}\n```";
        assert_eq!(
            parse_calls(r),
            vec![("read_url".to_owned(), json!({"url": "https://a.org"}))]
        );
        // `arguments` instead of `args`.
        let r = format!(
            "```tool\n{}\n```",
            json!({"tool": "web_search", "arguments": {"query": "x"}})
        );
        assert_eq!(
            parse_calls(&r),
            vec![("web_search".to_owned(), json!({"query": "x"}))]
        );
        // A `{tool, args}` envelope around the arguments of the same tool.
        assert_eq!(
            normalize_args(
                "web_search",
                json!({"tool": "web_search", "args": {"query": "x"}})
            ),
            json!({"query": "x"})
        );
        // A lone `parameters` key holding a JSON string.
        assert_eq!(
            normalize_args("web_search", json!({"parameters": "{\"query\": \"y\"}"})),
            json!({"query": "y"})
        );
        // Twice-encoded.
        assert_eq!(
            normalize_args("web_search", json!("\"{\\\"query\\\": \\\"z\\\"}\"")),
            json!({"query": "z"})
        );
        // Ordinary arguments, another tool's envelope and junk are left
        // for the schema check.
        assert_eq!(
            normalize_args("web_search", json!({"query": "q", "args": {"a": 1}})),
            json!({"query": "q", "args": {"a": 1}})
        );
        assert_eq!(
            normalize_args(
                "web_search",
                json!({"tool": "file_read", "args": {"path": "a"}})
            ),
            json!({"tool": "file_read", "args": {"path": "a"}})
        );
        assert_eq!(normalize_args("now", Value::Null), json!({}));
        assert_eq!(normalize_args("now", json!("not json")), json!("not json"));
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
    fn wrapped_calls_are_unwrapped() {
        // codestral: a tool named `tool` carrying the real call.
        let r = format!(
            "```tool\n{}\n```",
            json!({"tool": "tool", "args": {"tool": "file_read", "args": {"path": "sum.md"}}})
        );
        assert_eq!(
            parse_calls(&r),
            vec![("file_read".to_owned(), json!({"path": "sum.md"}))]
        );
        assert_eq!(
            parse(&r),
            Reply::Tool {
                name: "file_read".into(),
                args: json!({"path": "sum.md"})
            }
        );
        // ministral: the name is the call itself, with a stray brace.
        let (n, a) = unwrap_call(
            r#"{"tool": "file_read", "args": {"path": "counts.md"}}}"#.into(),
            json!({}),
        );
        assert_eq!((n.as_str(), a), ("file_read", json!({"path": "counts.md"})));
        // Arguments given beside such a name are kept.
        let (n, a) = unwrap_call(r#"{"name": "files_list"}"#.into(), json!({"path": "src"}));
        assert_eq!((n.as_str(), a), ("files_list", json!({"path": "src"})));
        // `arguments` as a JSON string.
        let (n, a) = unwrap_call(
            "function".into(),
            json!({"name": "web_search", "arguments": "{\"query\": \"rag\"}"}),
        );
        assert_eq!((n.as_str(), a), ("web_search", json!({"query": "rag"})));
        // Plain calls, and a `tool` with nothing inside, are left alone.
        let (n, a) = unwrap_call("file_read".into(), json!({"path": "a"}));
        assert_eq!((n.as_str(), a), ("file_read", json!({"path": "a"})));
        let (n, _) = unwrap_call("tool".into(), json!({"x": 1}));
        assert_eq!(n, "tool");
        let (n, _) = unwrap_call("{not json".into(), json!({}));
        assert_eq!(n, "{not json");
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
