# ADR 0004 · Bounded turn loop, policy gate, and a text tool protocol

| Field | Value |
| --- | --- |
| Status | Accepted |
| Date | 2026-10-21 |
| Decider | Khubaib Nazeer |

## Context

Phase 3 needs a turn loop that every surface (TUI, desktop, `tern ask`) can share. It has to stop on its own, let the user approve risky steps, and leave a log a UI can replay. Native tool calling differs across the three wire formats and is missing on many local models in Ollama and LM Studio.

## Decision

| Part | Decision |
| --- | --- |
| Loop | `tern-agent::engine::TurnLoop` runs one turn: model call, then tool calls, until the model answers or a limit stops it. Every step moves the `tern-core` turn state machine and is written to the event log before the next step starts. A log write failure stops the turn |
| Limits | Model calls (default 8), tool calls (12), and tokens (200,000) per turn. The same tool call three times is treated as a loop (`loop.suspected`, then `Failed`) |
| Policy gate | `tern-agent::policy::PolicyGate`. Each effect class is Allow, Ask, or Deny; defaults ask for credentials, external side effects, and destructive steps. Grants are *once* (these exact arguments, spent when the call runs) or *session* (this tool on this target, expires after 30 minutes). A new target asks again; a deny rule beats any grant |
| Approvals | An `Approver` puts the request in front of the user. `approval.requested` and `approval.answered` are both recorded. A refusal ends the turn as `Blocked` |
| Tools | A typed registry: name, version, one-line summary, effect class, target, JSON Schema input, timeout, idempotency. Input is checked before the gate. Unknown tools and bad input go back to the model as text, not as failures |
| Tool protocol | For now the model asks for a tool with a fenced `tool` JSON block. It works on every provider and local model. Native tool calls can replace it later; the loop only sees the parsed reply |
| Errors | A provider error fails the turn with its plain message. No silent retry, no fallback provider |

## Consequences

- The TUI and desktop app only supply an `Approver`, an `EventSink`, and a `CancelFlag`; they never call providers or the store directly.
- The text protocol costs a few prompt tokens and depends on the model following it; malformed blocks are recorded as `tool.rejected` and returned to the model.
- Streaming, pause and resume mid-turn, and workers come in later slices.
