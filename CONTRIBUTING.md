# Contributing

## Workflow

`main` <- `staging` <- `feature/*`. One PR per feature group, into staging.
Never commit directly to staging or main. Milestones tag `v0.x.0` on main with
a CHANGELOG entry; before each staging->main merge, a milestone review sweep
runs over the full diff.

## Gate

Every PR must pass:

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Set `TEST_DATABASE_URL` (see `.env.example`) or the integration suites skip
locally; CI runs them against a fresh Postgres service and the vacuity guards
make silent skips fail there.

## Code rules

- No code comments; the code and its tests carry the intent.
- Files aim under 400 lines (~500 is a guideline, not a hard cap); split when a file grows.
- I/O behind traits; each module owns a thiserror enum with `#[from]` chains —
  never flatten errors to strings across a boundary.
- No `#[allow]` escape hatches; fix the lint properly.
- No `_ =>` catch-alls on enums the codebase owns.
- Reviews apply the Canonical Rust standard via the `rust-review` skill; the
  reference lives in `.cache/rust-best-practices`.

## Tests

- Unit tests live beside the code; integration suites in `tests/` run against
  a real Postgres and wiremock externals.
- Shared-database discipline: unique ids per test, sweep locks for tests that
  scan whole tables, assertions that cannot pass vacuously.
- Live smokes are `#[ignore]` and env-gated; they hit real services.

## Migrations

Additive migrations are plain. Dedupe or destructive migrations must merge:
re-point child rows before any delete, propagate terminal statuses onto
survivors, back up what you remove. Copy `0011_email_normalization.sql`.
