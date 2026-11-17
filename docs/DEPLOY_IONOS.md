# Deploy peer-lm relay on IONOS

**Host:** `<RELAY_HOST>` (a plain VPS works; IONOS used here)  
**Remote path:** `~/Libp2p/peer-lm/`

## Current deployment

| Item | Value |
| --- | --- |
| Binary | `~/Libp2p/peer-lm/bin/peer-lm` |
| Identity | `~/Libp2p/peer-lm/data/relay.key` |
| Peer ID | `12D3KooWSWH7hzZMAuGyF5QfGayTF7asVrhoUGXFXBgu3LskFvi3` |
| Listen port | **4444** (VPS firewall allows TCP 4444) |
| Public multiaddr | `/ip4/<RELAY_HOST>/tcp/4444/p2p/<RELAY_PEER_ID>` |
| Start | `~/Libp2p/peer-lm/start-relay.sh` |
| Logs | `~/Libp2p/peer-lm/logs/relay.log` |

Verified reachable: `nc -vz <RELAY_HOST> 4444`.

## Client usage

```bash
cargo run -p peer-lm -- serve \
  --embedded-mock \
  --gateway 127.0.0.1:3000 \
  --no-mdns \
  --relay /ip4/<RELAY_HOST>/tcp/4444/p2p/<RELAY_PEER_ID> \
  --reserve-relay
```

Python end-to-end (two local peers via this relay + mock):

```bash
./python/scripts/smoke-relay-online.sh
# optional: PEER_LM_RELAY_ADDR=/ip4/…/tcp/4444/p2p/…
```

## Redeploy binary

```bash
cargo build -p peer-lm --bin peer-lm --release
scp target/release/peer-lm <user>@<RELAY_HOST>:~/Libp2p/peer-lm/bin/peer-lm
ssh <user>@<RELAY_HOST> 'pkill -f "peer-lm relay" || true; ~/Libp2p/peer-lm/start-relay.sh'
```
