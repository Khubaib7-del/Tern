# Security Policy

Tern is a private, proprietary, pre-alpha project. There is no released build, so there is nothing in users' hands to patch yet. This policy covers how the team, invited collaborators and invited testers report problems now, and the security model the code is built against.

## Supported versions

| Version | Supported |
| --- | --- |
| None released | No version is released. Reports against `main`, open branches, private test builds, or the design docs are welcome. |

When the first build is released (the private beta, then v1.0), this table will list which versions receive fixes.

## Reporting a vulnerability

Report privately by email to **[khubaibnazeer8@gmail.com](mailto:khubaibnazeer8@gmail.com)**, with "Tern security" in the subject.

- Never report a suspected vulnerability in an issue, discussion, or pull request, even in this private repository.
- Include what you found, where (file, crate, commit or doc section), how to reproduce it, and what an attacker could do with it.
- Never include real API keys or anyone's private research data in a report. Redact first.

What to expect:

| Step | Target |
| --- | --- |
| Acknowledgement | Within 5 days |
| First assessment (severity, scope) | Within 14 days |
| Fix or design change | Depends on severity; tracked privately by the team until resolved |

Tern is a self-funded, pre-release project, so there is no paid bounty. Reporters are credited in the changelog with their consent. Reporting a problem does not grant any licence to Tern's code or docs.

## Security model highlights

The full model is in [docs/05-security-model.md](docs/05-security-model.md). These are the commitments every crate is built against. The security model records which are built and which are still planned.

| Area | Commitment |
| --- | --- |
| BYOK keys | Provider keys live in the OS keychain (Windows Credential Manager first, then Secret Service on Linux and Keychain on macOS). They are never written to config files, never logged, and never persisted in plain text. |
| Event log | Secrets are stripped from events, tool output, and errors *before* anything is persisted. The redactor runs in the core, not in the UI. |
| Untrusted content | Text from web pages, PDFs, OCR, transcripts, and MCP results is data, never instructions. It is kept in an isolated channel, cannot change the plan or grant permissions, and sits at the bottom of the trust order. |
| Crawler | `tern-crawl` blocks loopback, link-local, private, and cloud-metadata addresses, re-checks every redirect hop, and pins resolved IPs to defeat DNS rebinding. |
| Permissions | The model proposes; the policy engine authorises exact arguments; an executor acts. Approvals are scoped to a tool class, target, run, and expiry. A new target asks again. An approval is never described as a sandbox. |
| Local-first | Vaults, libraries, indexes, and run logs stay on the user's machine. Local models (Ollama, LM Studio) are first-class. |
| Data-flow notice | Before files or text go to a remote provider, Tern shows which provider receives what. Sensitive material can be routed to a local model instead. |

## Scope

In scope: the design documents, the crates as they land, the crawler's network guard, secret handling, permission checks, prompt-injection handling, and anything that could leak keys or research data.

Out of scope for now: vulnerabilities in third-party providers' own services, social engineering of the team, and attacks that need physical access to an unlocked machine.
