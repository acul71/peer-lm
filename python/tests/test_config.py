from peer_lm.config import FileConfig
from peer_lm.connect import ConnectMode


def test_parses_auto_example(tmp_path):
    p = tmp_path / "c.yaml"
    p.write_text(
        """
gateway: "127.0.0.1:3000"
backend: "http://127.0.0.1:8080"
prefer_local: false
identity: "./data/client.key"
allowlist:
  - "12D3KooWExamplePeer"
connect:
  mode: auto
  mdns_timeout_secs: 3
  hole_punch_grace_secs: 20
  relays:
    - "/ip4/127.0.0.1/tcp/4444/p2p/12D3KooWRelay"
  dial_peers:
    - "12D3KooWExamplePeer"
"""
    )
    cfg = FileConfig.load(p)
    assert cfg.connect_mode() is ConnectMode.AUTO
    assert cfg.auto_timers().mdns_timeout_secs == 3
    assert len(cfg.connect.relays or []) == 1
