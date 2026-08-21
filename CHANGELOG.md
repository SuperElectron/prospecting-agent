# Changelog

## v1.0.0 — MVP (2026-08-21)

The complete local-first prospecting loop, reviewed and live-verified at every milestone: CSV or Apollo in, researched and personalized email out, replies analyzed and acted on, all unattended.

### Added
- End-to-end suite driving the full funnel through real job dispatch against mocked externals, hermetic across repeated and concurrent runs (cross-process advisory lock, per-run capacity keys)
- Configuration for everything that drives unattended spend or sending: targeting titles and seniority, discovery budgets, preflight knobs, cadence with send-window overrides, one shared daily Apollo credit cap
- Atomic write-ahead credit reservation: concurrent processes serialize on the ledger row; failures over-count instead of blowing the cap; midnight-spanning runs settle on the day they drew from
- Operator and contributor docs: setup, architecture (prose + diagram), operations runbook, CONTRIBUTING, and an agent skill for driving the control API
- Credential files written owner-only (0600)

### Post-1.0 (tracked)
- Slack notifier (M6), HubSpot sync (M6), HeyReach/LinkedIn (M6), jenticOne (M7), managed Mem0 (M8)
- Full-body reply fetch before trusting opt-out classification (#54); injectable job clock + send-pass scoping (#62); preflight Modify reachability (#51); enrichment cooldown asymmetry (#49)


## v0.4.0 — M4 Jobs & Runtime (2026-08-21)

The prospecting loop runs itself: a Postgres-backed job queue, twelve scheduled jobs, a local control API, and every workflow wired into unattended operation.

### Added
- Job runtime: apalis queue isolated in its own schemas, worker with per-job timeout and failure notify, cron scheduler over a compile-checked JobKind table (no name drift possible)
- Control API on loopback: health, job list/run/enqueue with in-flight guards and timeouts, weekly report, contact lookup, named webhook dispatch with the first production handler (inbound_reply)
- Discovery job: ICP-ranked sourcing for contact-less companies with a 30-day attempt cooldown that failed searches never burn
- Outreach engine: hourly send pass under every guard — cadence windows (follow-ups included), min-day spacing, account preflight with warm-intro capping, opt-out stops, capacity-aware transport; state advances before send so failures burn a slot instead of double-sending; final-step failures end as Manual, never fake a completion
- Sequence enrollment for enriched contacts; personalization angles memorized on send for future dedupe
- Agent tasks in Postgres: SKIP LOCKED claiming with a one-hour reaper lease, per-run credit budget, bounded retries to a terminal state
- Reply monitor: per-sender Gmail polling with pagination, case-insensitive contact matching enforced by the schema, an at-least-once claim lease with bounded attempts, replies through the M3 analysis pipeline
- Daily digest at the close of each send day plus the Monday weekly report, one parameterized activity-report path
- CI workflow (fresh Postgres service) enforcing fmt, clippy -D warnings, and the full integration suite — with vacuity guards so silently-skipped tests fail loudly
- Migrations 0008–0012: discovery tracking, agent tasks, processed-message ledger, email normalization (merge-style dedupe that re-points history and preserves terminal statuses), inbound attempt tracking

### Changed
- Contact emails normalize at every boundary; uniqueness moved to lower(email)
- BREAKING for existing .env files: MEM0_URL alias removed; the memory endpoint must be set as MEMORY_URL


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
