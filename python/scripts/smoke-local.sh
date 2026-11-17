#!/usr/bin/env bash
# Local smoke: embedded mock via peer-lm serve
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
python -m peer_lm.cli serve --embedded-mock --gateway 127.0.0.1:3001 --listen /ip4/127.0.0.1/tcp/0 &
PID=$!
trap 'kill $PID 2>/dev/null || true' EXIT
for i in $(seq 1 40); do
  if curl -sf http://127.0.0.1:3001/health >/dev/null; then
    break
  fi
  sleep 0.25
done
curl -sf http://127.0.0.1:3001/health
echo
curl -sf http://127.0.0.1:3001/v1/models
echo
curl -sf http://127.0.0.1:3001/v1/chat/completions \
  -H 'content-type: application/json' \
  -d '{"model":"peer-lm-mock","messages":[{"role":"user","content":"hello peer-lm"}],"max_tokens":32}'
echo
echo "smoke-local OK"
