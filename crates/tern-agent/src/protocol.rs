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

/// The system prompt for one turn.
pub fn system_prompt(base: Option<&str>, tools: &ToolRegistry) -> String {
    let mut s = String::from(
        "You are Tern, a careful research and study assistant. Answer plainly and say when you are unsure.",
    );
    if let Some(b) = base {
        s.push_str("\n\n");
        s.push_str(b);
    }
    if !tools.is_empty() {
        s.push_str("\n\nYou can use these tools:\n");
        s.push_str(&tools.describe());
        s.push_str(
            "\n\nTo use one, reply with only this block and nothing else:\n```tool\n{\"tool\": \"<name>\", \"args\": {...}}\n```\nYou will get the result in the next message. Ask for one tool at a time. When you have enough, reply with the final answer and no tool block.",
        );
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
