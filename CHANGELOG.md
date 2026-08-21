# Changelog

## v0.3.0 — M3 Workflows (2026-08-21)

The full prospecting loop as callable workflows, live-verified end to end: CSV import through research, strategy, outreach generation, and reply handling.

### Added
- CSV sync: companies, contacts, and notes land in Postgres and memory with per-row skip reporting, a note-import ledger, and backend-failure circuit breaking
- Discovery: Apollo people search scoped by ICP titles and seniority, match-by-id enrichment, per-account and per-run credit budgets that count spend rather than successes
- Enrichment: contact and company enrichment with convergence markers (attempt timestamps, terminal no-match status) so failed lookups stop burning credits
- Research: Tavily search plus LLM summarization into grounded company briefs and personalization angles, with prompt-injection fencing around all third-party text
- Signals: hiring, funding, leadership, and expansion detection with idempotent ingestion and strength scoring
- Account strategy: LLM assessment into typed stage and health enums (fail-closed schemas), coordination flags, and outreach preflight guards (converted accounts, carpet-bomb window, negative events)
- Outreach: grounded email generation citing memorized research, messaging-rule validation with one retry carrying the rejected draft, HTML rendering with full escaping
- Reply analysis: record-first classification into typed intents, transactional status disposition and sequence stop, opt-out handling
- Weekly report over a single aggregate query
- Migrations 0004–0007: note ledger, enrichment tracking with deterministic crm-id dedupe (losers staged to a backup table), strategy constraints, sequence stop timestamps

### Changed
- Milestone sweep: per-module error enums for discovery and reporting, shared truncation and memory helpers, canonical-domain drift kept on the queried company row, enrichment failures now recorded so the queue converges



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
