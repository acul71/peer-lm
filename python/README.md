# peer-lm (Python)

Full-featured Python port of peer-lm on [py-libp2p](https://github.com/libp2p/py-libp2p).

**Default implementation for this repo is Rust** — see [`../README.md`](../README.md) and [`../rust/`](../rust/).

## Setup

Installs `libp2p>=0.8.1` (py-libp2p) from PyPI. For py-libp2p features not yet released, use a local checkout with `uv pip install -e ../py-libp2p`.

```bash
cd python
uv sync   # or: pip install -e ".[dev]"
```

## Commands

Same surface as Rust:

```bash
peer-lm keygen --out ./data/node.key
peer-lm mock --listen 127.0.0.1:8080
peer-lm relay --listen /ip4/0.0.0.0/tcp/4001 --identity ./data/relay.key
peer-lm rendezvous --listen /ip4/0.0.0.0/tcp/4002
peer-lm serve -c ../examples/config.mock.yaml
peer-lm serve --connect-mode auto --relay <multiaddr> --dial-peer <id>
```

YAML examples are shared at [`../examples/`](../examples/).

## Tests / smoke

```bash
cd python
pytest
./scripts/smoke-local.sh
./scripts/smoke-relay-local.sh          # local 3-process circuit infer
./scripts/smoke-relay-online.sh         # A+B via public IONOS relay (override with PEER_LM_RELAY_ADDR)
```

## Layout

| Module | Role |
| --- | --- |
| `peer_lm.cli` | Typer CLI |
| `peer_lm.gateway` | OpenAI HTTP facade |
| `peer_lm.network` | Swarm, auto ladder, caps/infer |
| `peer_lm.relay_server` / `rendezvous_server` | Infra roles |
| `peer_lm.backend` / `mock_backend` | Engine client + mock |
| `peer_lm.connect` / `config` | Modes + YAML |

## Notes

- Async runtime is **Trio** (required by py-libp2p).
- Caps/infer use CBOR maps on `/peer-lm/caps/1.0.0` (Python↔Python). Mixed Rust/Python meshes need matching codecs.
- Identity files are protobuf-encoded Ed25519 private keys (same idea as Rust).
- Circuit relay + DCUtR are wired like py-libp2p `examples/nat`. Local relay infer is covered by `tests/test_relay_infer.py` (reserve on compute, circuit dial from gateway).
