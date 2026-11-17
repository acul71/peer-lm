#!/usr/bin/env bash
# Three local processes: relay + compute (A) + client gateway (B).
# Forces /p2p-circuit (no mDNS) so the infer path goes through the relay.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

if [[ -n "${PEER_LM_PYTHON:-}" ]]; then
  PY="$PEER_LM_PYTHON"
else
  PY=python3
else
  PY=python3
fi
export PYTHONPATH="${ROOT}/src${PYTHONPATH:+:$PYTHONPATH}"
run() { "$PY" -m peer_lm.cli "$@"; }

IDIR="$(mktemp -d)"
trap 'kill $(jobs -p) 2>/dev/null || true; rm -rf "$IDIR"' EXIT

RELAY_ID="$(run keygen --out "$IDIR/relay.key")"
A_ID="$(run keygen --out "$IDIR/a.key")"
B_ID="$(run keygen --out "$IDIR/b.key")"
RELAY_ADDR="/ip4/127.0.0.1/tcp/44011/p2p/${RELAY_ID}"

echo "relay=$RELAY_ID"
echo "A=$A_ID"
echo "B=$B_ID"
echo "relay_addr=$RELAY_ADDR"

run relay --listen /ip4/127.0.0.1/tcp/44011 --identity "$IDIR/relay.key" >"$IDIR/relay.log" 2>&1 &
sleep 1

run serve \
  --embedded-mock \
  --gateway 127.0.0.1:3021 \
  --identity "$IDIR/a.key" \
  --listen /ip4/127.0.0.1/tcp/0 \
  --no-mdns \
  --no-dcutr \
  --relay "$RELAY_ADDR" \
  --reserve-relay \
  --allowlist "$B_ID" \
  >"$IDIR/a.log" 2>&1 &

for i in $(seq 1 40); do
  if grep -q 'relay reservation accepted' "$IDIR/a.log" 2>/dev/null; then
    break
  fi
  sleep 0.5
done
if ! grep -q 'relay reservation accepted' "$IDIR/a.log" 2>/dev/null; then
  echo "A never got relay reservation"
  tail -n 120 "$IDIR/a.log" || true
  tail -n 40 "$IDIR/relay.log" || true
  exit 1
fi

run serve \
  --gateway 127.0.0.1:3022 \
  --identity "$IDIR/b.key" \
  --listen /ip4/127.0.0.1/tcp/0 \
  --no-mdns \
  --no-dcutr \
  --relay "$RELAY_ADDR" \
  --dial-via-relay \
  --dial-peer "$A_ID" \
  --no-prefer-local \
  --allowlist "$A_ID" \
  >"$IDIR/b.log" 2>&1 &

for i in $(seq 1 40); do
  if RESP="$(curl -sf http://127.0.0.1:3022/v1/chat/completions \
    -H 'content-type: application/json' \
    -d '{"model":"peer-lm-mock","messages":[{"role":"user","content":"via relay"}]}' 2>/dev/null)"; then
    if echo "$RESP" | grep -q 'peer-lm-mock'; then
      echo "smoke-relay-local OK"
      echo "$RESP"
      exit 0
    fi
  fi
  sleep 1
done

echo "smoke-relay-local FAILED"
echo "--- relay.log ---"; tail -n 80 "$IDIR/relay.log" || true
echo "--- a.log ---"; tail -n 120 "$IDIR/a.log" || true
echo "--- b.log ---"; tail -n 120 "$IDIR/b.log" || true
exit 1
