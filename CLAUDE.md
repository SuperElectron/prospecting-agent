# prospecting-agent — project rules

## First-time setup (fresh clone)
If `.cache/rust-best-practices/` or `.env` is missing, this is a fresh clone — run `./scripts/setup.sh`
before any other work. It clones the Rust review reference into `.cache/`, creates `.env` and
`.claude/settings.local.json` from their examples, starts Postgres via docker compose, and runs
build + tests. Then remind the user to fill in API keys.

## GitHub issues
Every issue created in this repo MUST use this body structure, in this order:

```markdown
## Feature
<one short paragraph: what this delivers>

## Requirements
<bullet list: constraints, dependencies on other feature groups, integrations touched>

## Objectives
<bullet list: measurable outcomes — what is true when this closes>

## Tasklist
- [ ] task 1
- [ ] task 2
```

One issue per feature group. Add each issue to the project board
(https://github.com/users/SuperElectron/projects/4/views/1). Close via its PR merge into staging
(manual close with comment — auto-close only fires on default-branch merges).
Before closing an issue, edit its body so every completed Tasklist item is checked (`- [x]`);
never close an issue with unchecked boxes for work that was done.

## Workflow
- `main` ← `staging` ← `feature/*`. One PR per feature group. Never commit directly to staging/main.
- Gate per PR: `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`.
- Review pass (code-reviewer agent) on every PR before merge. Reviewers and authors apply the
  Canonical Rust standard via the `rust-review` skill (.claude/skills/rust-review/SKILL.md);
  full reference is the local clone at .cache/rust-best-practices (never fetch the website).
- Milestones tagged v0.1.0…v1.0.0 on main, CHANGELOG entry each.
- Milestone review: before each staging → main PR, run a dedicated review sweep of the full
  staging-vs-main diff against the rust-review standard; land findings as a `refactor:` commit
  before merging and tagging.
- For large milestones (M4/M5 especially), prompt the user to run `/code-review ultra` — the
  multi-agent cloud review of the branch. It is user-triggered and billed; Claude cannot launch
  it. Suggest it at the milestone boundary, then apply its findings like any review.

## Code rules
- No code comments. Keep files small — aim under 400 lines, ~500 is a guideline not a hard cap; when a file grows, split it into smaller parts. I/O behind traits. thiserror per module.
- No code text copied from .cache/revenue-os — genuine reimplementation only.
- No vendor branding in identifiers or docs.
- Plan of record: wiki (Architecture, Development-Plan) + .cache/commit-plan.md (detail).
