# peer-lm architecture

**peer-lm** = peer-to-peer language-model access: an OpenAI-compatible localhost gateway over libp2p.

Implementations: **Rust (default)** in [`rust/`](../rust/), **Python** in [`python/`](../python/). Shared examples/docs at repo root.

## Stack

```text
Agent (pi / Claude Code / curl)
        |
        |  HTTP OpenAI API on 127.0.0.1
        v
   peer-lm gateway
        |
        +--> local backend (mock / Strata / Ollama) if healthy
        |
        +--> libp2p request/response `/peer-lm/caps|infer/1.0.0`
                |
                v
           remote peer-lm --> its local backend
```

## Trust (v0.1)

- Transport is encrypted (Noise).
- Compute peers **can read prompts**. This is a private mesh, not confidential compute.
- Allowlist peer IDs for anything beyond localhost demos (`--allowlist`). Empty allowlist = allow all (dev only).

## Connectivity

| Phase | Feature | Status |
| --- | --- | --- |
| v0.1 | mDNS LAN discovery, request/response infer, mock backend | done |
| v0.2 | Circuit Relay v2 + DCUtR; `peer-lm relay`; local 3-process smoke | done (`scripts/smoke-relay-local.sh`) |
| v0.3 | Rendezvous namespaces (`peer-lm rendezvous` + `--rendezvous`) | done (client + server) |
| v0.4 | `connect.mode: auto` (mDNS → hole-punch/direct → relay) + YAML config | done |

### Connect modes

See [CONNECT_AUTO.md](CONNECT_AUTO.md) and [CONFIGURATION.md](CONFIGURATION.md). Short version:

- **`auto`** — try mDNS first; on timeout dial via relay and let DCUtR aim for a direct path; keep relay if punch fails  
- **`lan`** — mDNS / direct only  
- **`relay`** — always circuit-dial  
- **`manual`** — raw flags (`--dial-via-relay`, `--no-mdns`, …)

### Self-host a relay (VPS)

1. `peer-lm keygen --out relay.key` → note printed peer id  
2. On a public host: `peer-lm relay --listen /ip4/0.0.0.0/tcp/4001 --identity relay.key`  
3. Open TCP (and preferably UDP/QUIC) 4001  
4. Clients: `--relay /ip4/<VPS_IP>/tcp/4001/p2p/<RELAY_ID> --reserve-relay` (compute) and `--dial-via-relay --dial-peer <id>` (client)  

Reservation happens after Identify with the relay; DCUtR upgrades to direct when possible. Localhost smoke proves **relayed circuits**; real hole-punch needs two NATs.

## Why not load Strata for development

Real engines can OOM a laptop. Default demos use `peer-lm mock` / `--embedded-mock` (megabytes, canned SSE). Point `PEER_LM_BACKEND_URL` at Strata later unchanged.
