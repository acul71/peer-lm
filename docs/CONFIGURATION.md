# Configuration

`peer-lm serve` accepts a YAML file via `--config` / `-c`. Explicit CLI flags override the file.

```bash
# Rust (from rust/)
cargo run -p peer-lm -- serve --config ../examples/config.auto.client.yaml

# Python (from python/)
peer-lm serve -c ../examples/config.auto.compute.yaml --gateway 127.0.0.1:3001
```

## File schema

| Key | Type | Meaning |
| --- | --- | --- |
| `gateway` | string | OpenAI HTTP bind (`host:port`) |
| `listen` | string | libp2p listen multiaddr |
| `backend` | string | OpenAI-compatible engine URL (e.g. Strata `http://127.0.0.1:8080`) |
| `embedded_mock` | bool | In-process mock backend (no real LLM) |
| `prefer_local` | bool | Use local backend when healthy before peers |
| `identity` | path | Persistent ed25519 key (created if missing) |
| `allowlist` | string[] | Allowed remote peer IDs (empty = allow all, **dev only**) |
| `connect` | object | Connectivity strategy (see below) |

### `connect`

| Key | Type | Meaning |
| --- | --- | --- |
| `mode` | `manual` \| `lan` \| `relay` \| `auto` | See [CONNECT_AUTO.md](CONNECT_AUTO.md) |
| `mdns` | bool | Enable mDNS (used by `manual`; forced on for `lan`/`auto`) |
| `dcutr` | bool | Enable DCUtR hole punching |
| `reserve_relay` | bool | Listen on `/p2p-circuit` via a relay |
| `dial_via_relay` | bool | Force bootstrap dials through the relay |
| `mdns_timeout_secs` | u64 | `auto`: wait this long for LAN before relay |
| `hole_punch_grace_secs` | u64 | `auto`: grace for DCUtR after relayed dial |
| `relays` | string[] | Relay multiaddrs ending in `/p2p/<RELAY_ID>` |
| `dial_peers` | string[] | Peer IDs to dial at startup |
| `rendezvous` | string | Optional rendezvous multiaddr |
| `mesh_namespace` | string | Rendezvous namespace (default `peer-lm`) |

## Examples

| File | Role |
| --- | --- |
| [`examples/config.auto.compute.yaml`](../examples/config.auto.compute.yaml) | GPU box with Strata; auto connect + reserve relay |
| [`examples/config.auto.client.yaml`](../examples/config.auto.client.yaml) | Laptop agent gateway; auto toward a known compute peer |
| [`examples/config.lan.yaml`](../examples/config.lan.yaml) | Same LAN only (mDNS) |
| [`examples/config.relay.yaml`](../examples/config.relay.yaml) | Force relay path (NAT lab / CI) |
| [`examples/config.mock.yaml`](../examples/config.mock.yaml) | Local mock demo |

## Environment

- `PEER_LM_BACKEND_URL` — same as `backend` / `--backend`
- `RUST_LOG` — tracing filter (default directive includes `info`)
