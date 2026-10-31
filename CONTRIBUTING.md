# Contributing to Tern

Tern is a private, proprietary, pre-alpha project owned by Khubaib Nazeer (Founder) with Hassan Saeed Hashmi (Co-founder). It is not open source.

## Who can contribute

| Rule | Detail |
| --- | --- |
| Founding team and invited people only | Contributions come from the founding team or from people the founders have invited in writing. Having read access to the repository is not an invitation to contribute. |
| No public pull requests | Tern does not accept pull requests, patches, or code from anyone outside the team and its written invitations, and does not accept work from forks. |
| Ownership | Every contribution (code, docs, designs, tests, assets, and ideas submitted in an issue or pull request) is assigned to and owned by Tern's owners, unless agreed otherwise in writing. See [LICENSE](LICENSE). |
| Confidentiality | The code, docs, and plans in this repository are confidential. Do not copy, publish, or share them outside the team without written permission. |

If you have been invited, this is how work moves through the repository.

## Before you start

1. Read [AGENTS.md](AGENTS.md). It applies to people as much as to AI coding agents.
2. Read the doc that covers your area: [docs/03-architecture.md](docs/03-architecture.md) for crate boundaries, [docs/05-security-model.md](docs/05-security-model.md) for anything touching keys, network, permissions, or untrusted content.
3. Check [docs/04-roadmap.md](docs/04-roadmap.md). Work should belong to the current phase unless the maintainer says otherwise.

## Branches and pull requests

| Rule | Detail |
| --- | --- |
| Never push to `main` | All changes land through a pull request. `main` is always releasable once releases exist. |
| One topic per branch | Name it `<type>/<short-topic>`, for example `feat/provider-keychain`, `fix/crawl-redirect-guard`, `docs/roadmap-phase-2`. |
| Small PRs | Prefer several focused PRs over one large one. |
| Fill in the template | Summary, roadmap phase, and real verification output. If a check was not run, say so. |
| Review | `@Khubaib7-del` reviews every PR (see `.github/CODEOWNERS`). Do not merge your own PR unless asked to. |
| Docs travel with code | If behaviour, a plan, or a decision changes, update the doc and `CHANGELOG.md` in the same PR. |

## Commit messages

Tern uses [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/).

```text
<type>(<optional scope>): <summary in the imperative, lower case, no full stop>
```

| Type | Use for |
| --- | --- |
| `feat` | A new capability |
| `fix` | A bug fix |
| `docs` | Documentation only |
| `refactor` | Code change that neither fixes a bug nor adds a feature |
| `perf` | Performance improvement |
| `test` | Adding or fixing tests |
| `build` | Cargo, dependencies, toolchain |
| `ci` | CI configuration |
| `chore` | Repository housekeeping |

Scopes follow crate names without the prefix where it helps: `feat(crawl): ...`, `fix(providers): ...`, `docs(roadmap): ...`.

Keep commits atomic: one coherent change each, no "wip" or "misc" commits on a PR branch that is ready for review.

## Rust expectations

Every PR that touches Rust is expected to pass:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

| Expectation | Detail |
| --- | --- |
| Toolchain | Stable Rust, pinned in `rust-toolchain.toml`. |
| Formatting | `rustfmt` defaults plus the repository `rustfmt.toml`. |
| Lints | Clippy clean with `-D warnings`. Allow a lint locally with a comment that says why. |
| Errors | `thiserror` in library crates, `anyhow` only at binary edges. No `unwrap()` or `expect()` outside tests without a comment explaining why it cannot fail. |
| Unsafe | No `unsafe` without maintainer approval and a `// SAFETY:` comment. |
| Async | `tokio`. Never block the runtime; use `spawn_blocking` for CPU or blocking I/O. |
| Windows first | Test on Windows before asking for review. Linux and macOS follow; do not hard-code path separators or shell assumptions. |
| Dependencies | Permissive licences only (MIT, Apache-2.0, BSD, ISC, Zlib, MPL-2.0 by approval). Copyleft projects can be read as design references; their code is never copied in. |
| Secrets | Never log a key, token, or full request header. Use the core redaction helpers. |

## Code of conduct

Everyone on the team, and every invited collaborator and tester, agrees to the [Code of Conduct](CODE_OF_CONDUCT.md).

## Security

Report vulnerabilities privately by email to [khubaibnazeer8@gmail.com](mailto:khubaibnazeer8@gmail.com), as described in [SECURITY.md](SECURITY.md). Never report them in an issue, discussion, or pull request.

## Licence

Tern is proprietary (see [LICENSE](LICENSE)). By contributing you agree that your contributions are assigned to and owned by Tern's owners, unless agreed otherwise in writing. No licence to use, copy, or distribute Tern is granted by contributing or by having access.
