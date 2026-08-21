# Changelog

## v0.2.0 — M2 Clients & Connectors (2026-08-21)

Data providers and the email channel, all live-verified against real services.

### Added
- Apollo client: people search, person match, org enrichment, with mappers into domain types; wire shapes verified against the live API (filter key drift found and fixed)
- Tavily client: search with depth/topic/recency knobs
- Gmail connector: installed-app OAuth with PKCE and state verification, MIME builder with thread headers and injection guard, capacity-aware sender rotation, reply-polling primitives
- Notifier trait with log implementation (Slack lands M6 behind the same trait)
- Health checks: db, LLM, memory, gmail token, send capacity; CLI subcommands gmail-auth, health
- Local LLM router service splitting chat and embedding upstreams; memory stack reproducible via scripts/configure-memory.sh — live inference and semantic recall verified on the DGX
- 180 tests: unit, wiremock, live-Postgres integration, gated live smokes (real Gmail send + poll)


## v0.1.0 — M1 Foundation (2026-08-21)

First milestone. The binary boots, migrations run, and the LLM and memory services are reachable.

### Added
- Domain model: contacts, companies, engagements, signals, sequence states, tasks
- Typed configuration from environment: agent modes, targeting (ICP + score weights), cadence with send-window search, messaging rules, secrets that never print
- Postgres persistence via sqlx: migrations, repositories with conflict-safe upserts, audit history, atomic daily send-capacity reservation
- LLM client for any OpenAI-compatible endpoint: retries, structured output via JSON schema, robust JSON extraction from model output, governance policy injection, multi-step composition
- Memory client for self-hosted Mem0/OpenMemory: entity-tagged memorize and recall with paging, token-budgeted digest
- Seed data: 20 Austin TX startups with placeholder role contacts
- Docker compose: Postgres (pgvector), Qdrant, OpenMemory; fresh-clone bootstrap script
- 125 tests: 108 unit, 16 live-Postgres integration, 1 live-memory smoke

### Fixed (milestone review sweep)
- Contact upsert survives an email arriving after the first insert
- Signal and engagement writes are idempotent under re-runs and double-sends
- Company domains normalized at every storage and lookup boundary
- Empty banned phrase no longer hangs the messaging check
