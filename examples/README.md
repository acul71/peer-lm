# Examples

YAML configs for `peer-lm serve -c <file>` (Rust or Python).

| File | Mode | Purpose |
| --- | --- | --- |
| `config.mock.yaml` | lan | Embedded mock, localhost only |
| `config.lan.yaml` | lan | LAN mDNS mesh |
| `config.relay.yaml` | relay | Always use circuit relay |
| `config.auto.compute.yaml` | auto | Compute node + Strata backend |
| `config.auto.client.yaml` | auto | Agent client → compute peer |

See [docs/CONFIGURATION.md](../docs/CONFIGURATION.md) and [docs/CONNECT_AUTO.md](../docs/CONNECT_AUTO.md).

## Two-machine auto sketch (Rust default)

```bash
cd rust
cargo run -p peer-lm -- keygen --out ../data/compute.key
cargo run -p peer-lm -- serve -c ../examples/config.auto.compute.yaml
```

Python equivalent: `cd python && peer-lm serve -c ../examples/config.auto.client.yaml`  
(after editing `dial_peers` / `allowlist`).

Point agents at `http://127.0.0.1:3012/v1`.
