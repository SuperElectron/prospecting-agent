# Architecture

One Rust binary, three run modes, Postgres as the only stateful dependency
beyond the memory stack.

```
              +-----------+     +-----------+
   CSV  --->  |  workflows| --> | Postgres  | <---> apalis queue (own schemas)
   Apollo --> |  (pure    |     +-----------+
   Tavily --> |   logic)  | --> | OpenMemory| --> Qdrant
   LLM    --> |           |     +-----------+
   Gmail <--> +-----------+
                    ^
        +-----------+-----------+
        |     jobs (dispatch)   |  <- cron schedules, credit budgets, timeouts
        +-----------+-----------+
                    ^
     +------+  +--------+  +---------+
     | CLI  |  | worker |  | control |
     +------+  +--------+  |   API   |
                           +---------+
```

## Layers

- `domain` — plain types, no I/O. Normalization (domains, emails) lives here.
- `db` — sqlx repositories; every write path normalizes; conflict-safe upserts;
  partial-upsert discipline (import/enrichment writes never clobber
  status/score/assignment).
- `clients` — Apollo, Tavily; retry with backoff on 429/5xx only.
- `connectors` — Gmail (OAuth/PKCE, MIME, capacity-aware rotation, polling),
  Notifier trait (log implementation now, Slack at M6), health checks.
- `llm` / `memory` — OpenAI-compatible chat with structured output via JSON
  schema; Mem0 memorize/recall/digest with entity tags.
- `workflows` — the business logic: sync, discovery, enrichment, research,
  signals, account strategy, outreach generation and engine, reply analysis,
  reporting. I/O comes in through parameters; each module owns a thiserror enum.
- `jobs` — the `JobKind` registry (one enum drives names, dispatch, and the cron
  table; drift is a compile error), Apollo credit ledger, per-job timeout,
  apalis runtime in isolated schemas.
- `http` — loopback control API and the named-webhook registry.

## Invariants worth knowing

- Sends: state advances before transport; a failure burns a cadence slot,
  never double-sends. Final-step failures end as Manual, not Completed.
- Follow-ups obey the send window; nothing fires outside Tue-Thu 08:00-16:00
  (standard cadence) no matter how overdue.
- Every paid Apollo path draws from one daily ledger (`APOLLO_DAILY_CREDIT_CAP`).
- Claims (tasks, inbound messages) are leases: exclusive, reaped after an hour,
  bounded attempts, terminal state.
- Prompt-injection containment: third-party text is fenced (`<web_result>`,
  `<reply>`) and policies live in the system message, pinned by tests.
- Dedupe migrations merge (re-point history, propagate terminal statuses);
  copy 0011, not 0005.

## Queue isolation

The apalis queue lives in the `apalis`/`apalis_meta` schemas with a pinned
`search_path`, so its timestamped migrations never touch
`public._sqlx_migrations`. A regression test asserts the placement of both
migration tables and that no queue relation leaks into `public`.
