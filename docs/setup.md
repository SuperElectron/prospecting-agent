# Setup

## Fresh clone

```
./scripts/setup.sh
```

The script clones the Rust review reference into `.cache/`, creates `.env` and
`.claude/settings.local.json` from their examples, starts Postgres, Qdrant, and
OpenMemory via docker compose, and runs build plus tests. Fill in API keys in
`.env` afterwards.

## Required services

| Service | Default | Started by |
|---|---|---|
| Postgres (pgvector) | localhost:5432 | docker compose |
| OpenMemory (Mem0) | localhost:8765 | docker compose |
| Qdrant | localhost:6333 | docker compose |
| LLM (OpenAI-compatible) | LLM_BASE_URL | you |
| Embeddings router | bundled llm-router service | docker compose |

`scripts/configure-memory.sh` makes the memory stack reproducible: it writes the
OpenMemory config (model, base URL, embedding dims) straight into its sqlite
volume and recreates the Qdrant collection at the right dimension. Destructive
reset is gated behind `CONFIRM_RESET_MEMORY=1`.

## Environment

Copy `.env.example` to `.env`. Required: `DATABASE_URL`, `LLM_BASE_URL`,
`LLM_MODEL`, `MEMORY_URL`, `APOLLO_API_KEY`, `TAVILY_API_KEY`. Everything else
has defaults; see the comments in `.env.example` for scope notes — in
particular `DRY_RUN=true` gates only outbound email, not API spend.

Set `TEST_DATABASE_URL` to run the integration suites; without it they skip
locally and fail loudly in CI.

## Gmail

```
prospecting-agent gmail-auth --daily-limit 25
```

Runs the installed-app OAuth flow (PKCE) against your own Google Cloud client.
The sender credentials the app writes land in `.claude/secrets/` (gitignored, owner-only 0600); the OAuth client JSON you place there yourself keeps whatever mode you give it. Scopes: send +
readonly.

## Verify

```
prospecting-agent health
```

Nonzero exit if any of database, LLM, memory, Gmail token, or send capacity is
down.
