---
name: rust-review
description: Canonical Rust best-practices checklist for writing and reviewing Rust code in this repo. Use when authoring Rust, reviewing a PR, or running the code-reviewer pass. Distilled from canonical/rust-best-practices; full text lives locally in .cache/rust-best-practices/site/src/.
---

# Rust review standard (Canonical)

Source of truth: `.cache/rust-best-practices/site/src/*.md` (local clone — read the relevant
chapter when a finding needs the full rationale; refresh with `git -C .cache/rust-best-practices pull`).
Web mirror: https://canonical.github.io/rust-best-practices — never fetch; use the local clone.

Apply this checklist on every Rust review, alongside repo rules in CLAUDE.md
(no comments, files small, thiserror per module, I/O behind traits).

## Imports
- No glob imports (`use foo::*`) outside preludes.
- Group: std / external crates / this crate, blank-line separated.
- Import types, traits, enums directly; call functions through their parent module (`fs::read`).
- `use self::...` explicit inside module trees.

## Naming
- Name by content/purpose, never by type (`workload`, not `data`, not `the_struct`).
- Variables holding an `Option`/pattern-matched value: name the inner thing, same name through the match.
- Generic parameters: meaningful names when non-obvious (`Store`, not `S`, when several exist).
- Lifetimes: descriptive (`'src`, `'conn`), not `'a` soup, when more than one.
- Builders: `XBuilder`, created by `X::builder()`, finished by `.build()`.

## Pattern matching
- Match exhaustively when variants carry meaning — avoid `_ =>` catch-alls that silently absorb new variants.
- Don't pattern-match references (`match *x` / deref instead of `&pat`).
- No `.0`/`.1` tuple indexing where destructuring reads better.
- Destructure struct params in the signature when only fields are used.

## Errors & panics
- Error messages: lowercase, no trailing punctuation, state what failed (`"failed to bind {addr}"`).
- One thiserror enum per module; variants carry context fields, not preformatted strings.
- Convert foreign errors at the boundary (`#[from]`), never leak transport errors upward raw.
- Panic only on broken invariants; panic messages calm and factual; library paths return `Result`.

## Functions
- No no-information returns (`Result<(), ()>`, bool that's always true).
- Hide generic complexity: prefer `impl Trait` in argument/return position when the concrete type is noise.
- Default trait impls: prefix intentionally-unused params with `_`.
- Builders own `self` (consume-and-return), build() returns the target.

## Ordering & structure
- File order: types → their impls (adjacent) → free functions → tests.
- Derive order: std traits first (Debug, Clone, Copy, PartialEq, Eq, Hash), then external.
- Struct fields ordered by importance/identity first, bookkeeping (timestamps) last.
- `mod.rs`: declarations + re-exports + shared error type; minimal logic.
- `Error`/`Result` aliases live at the module root.

## Code discipline
- `Self` for constructors/returns inside impls; concrete name when referring to *another* role of the type.
- Struct population: field-init shorthand, no `..Default::default()` masking required fields.
- `Vec::new()` over `vec![]` for empties; `collect()` over explicit `FromIterator` calls.
- Scope `let mut` as tightly as possible; prefer building expressions over mutate-after-declare.
- No unassigned `let` then assign-in-branches — use `let x = if/match` expressions.
- Take references at the call site, not stored ahead in a distant `let`.
- Shadow only for the same value transformed (`let x = x.trim()`), never for unrelated values.
- Type annotations only where inference genuinely fails.
- No explicit `drop(x)` for scoping — use a block.
- No method calls hanging off a closing brace of a multi-line literal/block — bind to a name first.
- Inline format args: `format!("{path}")`, not `format!("{}", path)`.
- Hex literals lowercase.

## Unsafe
- Repo forbids unsafe (`unsafe_code = "forbid"`); any proposal to relax is a design discussion, not a PR.

## Reviewer instructions
When acting as reviewer: cite the discipline (e.g. "error-and-panic-discipline: variant should
carry the addr field") so findings are checkable against `.cache/rust-best-practices/site/src/`.
