#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

set -a; . ./.env; set +a
MEM0_URL="${MEM0_URL:-http://localhost:8765}"
LLM_MODEL="${LLM_MODEL:-gpt-oss-120b}"
EMBED_MODEL="${LLM_EMBEDDING_MODEL:-qwen3-embedding-0.6b}"
EMBED_DIMS="${LLM_EMBEDDING_DIMS:-1024}"

echo "-- waiting for memory service"
for _ in $(seq 1 30); do
  curl -sf -m 2 "$MEM0_URL/api/v1/config/" > /dev/null 2>&1 && break
  sleep 1
done

echo "-- writing model config (embedding_dims via sqlite: the config API drops unknown fields)"
docker compose exec -T memory python3 - <<PYEOF
import json, sqlite3
db = sqlite3.connect('/usr/src/openmemory/data/openmemory.db')
row = db.execute("SELECT value FROM configs WHERE key='main'").fetchone()
config = json.loads(row[0]) if row else {"openmemory": {"custom_instructions": None}, "mem0": {}}
config.setdefault("mem0", {})
config["mem0"]["llm"] = {"provider": "openai", "config": {
    "model": "$LLM_MODEL", "temperature": 0.1, "max_tokens": 2000, "api_key": "env:OPENAI_API_KEY"}}
config["mem0"]["embedder"] = {"provider": "openai", "config": {
    "model": "$EMBED_MODEL", "api_key": "env:OPENAI_API_KEY"}}
if row:
    db.execute("UPDATE configs SET value=? WHERE key='main'", (json.dumps(config),))
else:
    db.execute("INSERT INTO configs (key, value) VALUES ('main', ?)", (json.dumps(config),))
db.commit()
print("config written")
PYEOF

echo "-- ensuring qdrant collection matches the embedder dimension ($EMBED_DIMS)"
docker compose exec -T memory python3 - <<PYEOF
import json, urllib.request, urllib.error
def req(method, path, body=None):
    r = urllib.request.Request(f"http://mem0_store:6333{path}", method=method,
        data=json.dumps(body).encode() if body else None,
        headers={"Content-Type": "application/json"})
    try:
        return json.load(urllib.request.urlopen(r, timeout=10))
    except urllib.error.HTTPError as e:
        return {"error": e.read().decode()[:200]}
info = req("GET", "/collections/openmemory")
size = (info.get("result") or {}).get("config", {}).get("params", {}).get("vectors", {}).get("size")
if size != $EMBED_DIMS:
    req("DELETE", "/collections/openmemory")
    print("recreate:", req("PUT", "/collections/openmemory",
        {"vectors": {"size": $EMBED_DIMS, "distance": "Cosine"}}))
else:
    print("collection already at", size)
PYEOF

docker compose restart memory
echo "== memory configured: llm=$LLM_MODEL embedder=$EMBED_MODEL dims=$EMBED_DIMS =="
