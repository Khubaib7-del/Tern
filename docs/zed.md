# Using Tern in Zed

Tern speaks the [Agent Client Protocol](https://agentclientprotocol.com) (ACP v1), so it can run as an agent inside Zed and other editors that support ACP. The editor starts `tern acp` and talks to it over stdin and stdout.

## Before you start

| Step | Command |
| --- | --- |
| Put `tern` on your `PATH` | Build it (`cargo build -p tern-tui --release`) or take `tern.exe` from the CI artifact, and add its folder to `PATH` |
| Add a model provider | `tern provider add groq` (or any provider; keys stay in the OS keychain) |
| Check it works | `tern doctor` |

## Add Tern to Zed

Open Zed's settings (`zed: open settings`) and add Tern under `agent_servers`:

```json
{
  "agent_servers": {
    "Tern": {
      "type": "custom",
      "command": "tern",
      "args": ["acp"]
    }
  }
}
```

If `tern` is not on Zed's `PATH`, give the full path in `command` (for example `C:\\Tools\\tern.exe`).

Open the Agent panel, start a new thread, and pick **Tern**.

## What carries over

| In Zed | In Tern |
| --- | --- |
| The open folder | The project root: its `TERN.md` and `.tern/skills/` apply |
| A new thread | A Tern chat, saved like any other (`tern project`, the desktop sidebar) |
| Loading a past thread | The Tern chat, replayed into the panel |
| Tool steps | Shown as tool calls with their progress |
| A tool that needs approval | Zed's permission prompt: allow once, allow for this session, or reject |
| Stop | Cancels the turn at the next safe point |

Your provider, model, MCP servers, and skills are the same ones the desktop app and `tern` use. `tern instructions` shows exactly what the model is told for a folder.

## Troubleshooting

| Problem | Check |
| --- | --- |
| Tern does not appear | The JSON above is inside the top-level object, and Zed was reloaded |
| "No provider" | Run `tern provider add …` once in a terminal |
| Nothing happens on send | Run `tern acp` in a terminal: it should wait silently for input; anything it prints first is the problem |
