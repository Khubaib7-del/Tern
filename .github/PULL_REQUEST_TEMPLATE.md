## Summary

What does this PR change and why? One to three sentences.

## Type

- [ ] feat
- [ ] fix
- [ ] docs
- [ ] refactor
- [ ] test
- [ ] chore / ci / build

## Roadmap phase

Which phase in `docs/04-roadmap.md` this belongs to, or "docs-only".

## Verification

Commands run and their actual results. If something was not run, say so plainly; do not imply checks that did not happen.

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Checklist

- [ ] Branch is not `main`; commits follow Conventional Commits
- [ ] Docs and `CHANGELOG.md` updated in this PR where behaviour or plans changed
- [ ] Nothing planned is described as shipped
- [ ] Crate boundaries in `docs/03-architecture.md` respected
- [ ] No secrets, keys, personal research data, or build output committed
- [ ] New dependencies have permissive licences (no copyleft code copied in)
