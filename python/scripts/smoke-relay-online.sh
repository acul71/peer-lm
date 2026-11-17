#!/usr/bin/env bash
# Two local Python peers + public Circuit Relay (default: IONOS lab).
# Compute A reserves on the relay; client B dials A via /p2p-circuit and infers.
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

RELAY_ADDR="${PEER_LM_RELAY_ADDR:-/ip4/<RELAY_HOST>/tcp/4444/p2p/<RELAY_PEER_ID>}"

# Prefer free ports (avoid colliding with leftover Rust smokes on 3011/3012).
free_port() {
  "$PY" -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()'
}
if [[ -n "${PEER_LM_GW_A:-}" ]]; then
  GW_A="$PEER_LM_GW_A"
else
  GW_A="127.0.0.1:$(free_port)"
fi
if [[ -n "${PEER_LM_GW_B:-}" ]]; then
  GW_B="$PEER_LM_GW_B"
else
  GW_B="127.0.0.1:$(free_port)"
fi

# Quick TCP reachability (host/port from multiaddr when possible)
RELAY_HOST="$(echo "$RELAY_ADDR" | sed -n 's|.*/ip4/\([^/]*\)/.*|\1|p')"
RELAY_PORT="$(echo "$RELAY_ADDR" | sed -n 's|.*/tcp/\([^/]*\)/.*|\1|p')"
if [[ -n "$RELAY_HOST" && -n "$RELAY_PORT" ]]; then
  if ! nc -vz -w 5 "$RELAY_HOST" "$RELAY_PORT" 2>&1; then
    echo "relay not reachable at ${RELAY_HOST}:${RELAY_PORT}"
    exit 1
  fi
fi

IDIR="$(mktemp -d)"
trap 'kill $(jobs -p) 2>/dev/null || true; rm -rf "$IDIR"' EXIT

A_ID="$(run keygen --out "$IDIR/a.key")"
B_ID="$(run keygen --out "$IDIR/b.key")"

echo "A=$A_ID"
echo "B=$B_ID"
echo "relay_addr=$RELAY_ADDR"
echo "gw_a=http://${GW_A} gw_b=http://${GW_B}"

run serve \
  --embedded-mock \
  --gateway "$GW_A" \
  --identity "$IDIR/a.key" \
  --listen /ip4/0.0.0.0/tcp/0 \
  --no-mdns \
  --dcutr \
  --relay "$RELAY_ADDR" \
  --reserve-relay \
  --allowlist "$B_ID" \
  >"$IDIR/a.log" 2>&1 &

for i in $(seq 1 60); do
  if grep -q 'relay reservation accepted' "$IDIR/a.log" 2>/dev/null; then
    break
  fi
  if grep -qi 'relay reservation failed' "$IDIR/a.log" 2>/dev/null; then
    echo "A relay reservation failed"
    tail -n 80 "$IDIR/a.log" || true
    exit 1
  fi
  sleep 0.5
done
if ! grep -q 'relay reservation accepted' "$IDIR/a.log" 2>/dev/null; then
  echo "A never got relay reservation"
  tail -n 120 "$IDIR/a.log" || true
  exit 1
fi
echo "A reserved on relay"

run serve \
  --gateway "$GW_B" \
  --identity "$IDIR/b.key" \
  --listen /ip4/0.0.0.0/tcp/0 \
  --no-mdns \
  --dcutr \
  --relay "$RELAY_ADDR" \
  --dial-via-relay \
  --dial-peer "$A_ID" \
  --no-prefer-local \
  --allowlist "$A_ID" \
  >"$IDIR/b.log" 2>&1 &

for i in $(seq 1 60); do
  if RESP="$(curl -sf "http://${GW_B}/v1/chat/completions" \
    -H 'content-type: application/json' \
    -d '{"model":"peer-lm-mock","messages":[{"role":"user","content":"via ionos relay"}]}' 2>/dev/null)"; then
    if echo "$RESP" | grep -q 'peer-lm-mock'; then
      echo "smoke-relay-online OK"
      echo "$RESP"
      exit 0
    fi
  fi
  sleep 1
done

echo "smoke-relay-online FAILED"
echo "--- a.log ---"; tail -n 120 "$IDIR/a.log" || true
echo "--- b.log ---"; tail -n 120 "$IDIR/b.log" || true
exit 1
