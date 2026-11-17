#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$ROOT/target/debug/peer-lm"
cd "$ROOT"
cargo build -q -p peer-lm

IDIR="$(mktemp -d)"
trap 'kill $(jobs -p) 2>/dev/null || true; rm -rf "$IDIR"' EXIT

"$BIN" serve --embedded-mock --gateway 127.0.0.1:3001 --identity "$IDIR/a.key" --listen /ip4/127.0.0.1/tcp/0 >"$IDIR/a.log" 2>&1 &
sleep 1
curl -sf http://127.0.0.1:3001/health | tee "$IDIR/health.json"
echo
curl -sf http://127.0.0.1:3001/v1/models | tee "$IDIR/models.json"
echo
curl -sf http://127.0.0.1:3001/v1/chat/completions \
  -H 'content-type: application/json' \
  -d '{"model":"peer-lm-mock","messages":[{"role":"user","content":"hello peer-lm"}],"max_tokens":32}' \
  | tee "$IDIR/chat.json"
echo
echo "smoke-local OK"
