# Connect mode: `auto`

`connect.mode: auto` (or `--connect-mode auto`) runs a progressive connectivity ladder. The goal is a **direct** peer connection when possible; the public relay is only a bootstrap / fallback.

```text
1) mDNS (LAN)
      │  peer found → dial direct → done
      │  timeout (mdns_timeout_secs)
      ▼
2) Circuit relay + DCUtR
      │  dial /p2p-circuit through configured relay(s)
      │  DCUtR tries hole punch → prefer direct
      ▼
3) Stay on relay
      if hole punch fails, keep the relayed circuit
```

## Why this order

| Step | When it wins |
| --- | --- |
| **mDNS** | Same LAN / broadcast domain — lowest latency, no VPS traffic |
| **Hole punch (DCUtR)** | Different NATs but UDP/TCP punch succeeds — still direct after setup |
| **Relay** | Symmetric NAT / strict firewalls — always works if the relay is reachable |

Hole punching **requires** a relay (or similar rendezvous) to exchange candidates. In `auto`, peer-lm therefore:

1. Enables mDNS.
2. Dials configured relays and **reserves** a circuit listen address when relays are present (so others can reach you).
3. Waits `mdns_timeout_secs` for LAN discovery of `dial_peers`.
4. For any still-disconnected bootstrap peer, dials via `/p2p-circuit`.
5. Leaves DCUtR enabled so libp2p can upgrade that circuit to a direct connection.

## Requirements

- At least one `connect.relays` entry for steps 2–3 (IONOS example in [DEPLOY_IONOS.md](DEPLOY_IONOS.md)).
- Compute nodes should set `dial_peers` empty (or omit) and rely on reservation + allowlist; clients list the compute peer ID under `dial_peers`.
- Both sides should share matching `allowlist` entries in non-dev setups.

## Other modes

| Mode | Behaviour |
| --- | --- |
| `manual` | Exact CLI/YAML flags (`mdns`, `dial_via_relay`, `reserve_relay`, …) |
| `lan` | mDNS / direct only — no relay |
| `relay` | Always circuit-dial; optional DCUtR upgrade |
| `auto` | Ladder above |

## Quick try

```bash
# Terminal A — compute (Strata on :8080)
peer-lm serve -c examples/config.auto.compute.yaml

# Terminal B — client (replace dial_peers / allowlist with A's peer id)
peer-lm serve -c examples/config.auto.client.yaml

curl http://127.0.0.1:3012/v1/chat/completions \
  -H 'content-type: application/json' \
  -d '{"model":"qwen3.8-flash-next-iq3_s","messages":[{"role":"user","content":"hi"}]}'
```

Watch logs for:

- `auto: using mDNS/direct path`
- `auto: mDNS window elapsed — dialing missing peers via relay`
- `dcutr (hole punch toward direct connection)`
