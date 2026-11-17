# peer-lm

OpenAI-compatible **local gateway over libp2p** — reach a language model on another machine as if it were on localhost (LM Link–style), without Tailscale.

- **peer** = peer-to-peer (libp2p)
- **lm** = language model

## Languages

| Tree | Status | Default |
| --- | --- | --- |
| [`rust/`](rust/) | Reference implementation (rust-libp2p) | **yes** |
| [`python/`](python/) | Full port (py-libp2p) | no |

Shared docs and YAML examples live at the repo root (`docs/`, `examples/`).

## Quick start (Rust — default)

```bash
cd rust
cargo run -p peer-lm -- serve -c ../examples/config.mock.yaml
```

```bash
curl http://127.0.0.1:3000/health
curl http://127.0.0.1:3000/v1/chat/completions \
  -H 'content-type: application/json' \
  -d '{"model":"peer-lm-mock","messages":[{"role":"user","content":"hi"}]}'
```

Point coding agents at `http://127.0.0.1:3000/v1`.

## Python port

See [`python/README.md`](python/README.md). Requires `libp2p>=0.8.1` (py-libp2p, installed from PyPI).

```bash
cd python && uv sync && peer-lm serve -c ../examples/config.mock.yaml
```

## Features

- Mock OpenAI backend + localhost gateway
- mDNS LAN discovery + `/peer-lm/caps|infer` request/response
- Circuit Relay v2 + DCUtR client
- Rendezvous point
- **`connect.mode: auto`** — mDNS → hole-punch (direct) → relay fallback
- YAML config (`--config` / `-c`)

## Tests

```bash
cd rust && cargo test --workspace
cd python && pytest
```

Smokes: `rust/scripts/smoke-*.sh`, `python/scripts/smoke-local.sh`.

## Docs

- [Architecture](docs/ARCHITECTURE.md)
- [YAML configuration](docs/CONFIGURATION.md)
- [Auto connect ladder](docs/CONNECT_AUTO.md)
- [Examples](examples/README.md)
- [IONOS relay deploy](docs/DEPLOY_IONOS.md)
- [Python port](python/README.md)
