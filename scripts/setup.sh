#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

echo "== prospecting-agent setup =="

command -v cargo >/dev/null || { echo "missing: rust toolchain (https://rustup.rs)"; exit 1; }
command -v docker >/dev/null || { echo "missing: docker"; exit 1; }

if [ ! -d .cache/rust-best-practices ]; then
  echo "-- cloning rust-best-practices reference into .cache/"
  mkdir -p .cache
  git clone --quiet --depth 1 https://github.com/canonical/rust-best-practices .cache/rust-best-practices
else
  echo "-- rust-best-practices reference present"
fi

if [ ! -f .env ]; then
  echo "-- creating .env from .env.example (fill in your API keys)"
  cp .env.example .env
fi

if [ ! -f .claude/settings.local.json ]; then
  echo "-- creating .claude/settings.local.json from example (fill in your tokens)"
  cp .claude/settings.example.json .claude/settings.local.json
fi

echo "-- starting postgres (docker compose)"
docker compose up -d postgres

echo "-- waiting for postgres health"
for _ in $(seq 1 30); do
  if docker compose ps postgres | grep -q healthy; then break; fi
  sleep 1
done

echo "-- build + test"
cargo build --quiet
TEST_DATABASE_URL="postgres://postgres:postgres@localhost:5432/prospecting" cargo test --quiet

cat <<'EOF'
== setup complete ==
Next steps:
  1. Fill API keys in .env (Apollo, Tavily, HubSpot, Gmail, SendGrid, Slack)
  2. Point LLM_BASE_URL at your OpenAI-compatible endpoint
  3. Optional: docker compose up -d memory   (Mem0 semantic memory)
EOF
