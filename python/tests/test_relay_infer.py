"""In-process relay + two nodes + mock backend infer (parity with Rust relay_infer)."""

from __future__ import annotations

import json
import socket

import pytest
import trio
from hypercorn.config import Config
from hypercorn.trio import serve
from libp2p.crypto.ed25519 import create_new_key_pair

from peer_lm.backend import BackendClient
from peer_lm.connect import ConnectMode
from peer_lm.identity import peer_id_str
from peer_lm.mock_backend import MOCK_MODEL_ID, build_mock_app
from peer_lm.network import NodeConfig, PeerLmNode
from peer_lm.protocol import CapsInfo
from peer_lm.relay_server import run_relay_server


def _free_tcp_port() -> int:
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


@pytest.mark.trio
async def test_infer_through_local_relay():
    mock_port = _free_tcp_port()
    mock_bind = f"127.0.0.1:{mock_port}"
    mock_url = f"http://{mock_bind}"
    mock_app = build_mock_app()
    mock_cfg = Config()
    mock_cfg.bind = [mock_bind]

    relay_port = _free_tcp_port()
    relay_listen = f"/ip4/127.0.0.1/tcp/{relay_port}"
    relay_kp = create_new_key_pair()
    relay_id = peer_id_str(relay_kp)

    a_kp = create_new_key_pair()
    b_kp = create_new_key_pair()
    a_id = peer_id_str(a_kp)
    b_id = peer_id_str(b_kp)

    backend = BackendClient(mock_url)
    relay_addr = f"{relay_listen}/p2p/{relay_id}"

    ok = False
    last_err: Exception | None = None

    with trio.move_on_after(45) as scope:
        async with trio.open_nursery() as nursery:
            nursery.start_soon(serve, mock_app, mock_cfg)
            nursery.start_soon(run_relay_server, relay_kp, relay_listen)
            await trio.sleep(0.5)

            a_cfg = NodeConfig(
                listen="/ip4/127.0.0.1/tcp/0",
                enable_mdns=False,
                # DCUtR is optional for circuit infer; leaving it on makes
                # graceful host shutdown hang under pytest — cancel below.
                enable_dcutr=False,
                allowlist={b_id},
                local_caps=CapsInfo([MOCK_MODEL_ID], "mock", 1, 8192, False),
                keypair=a_kp,
                relay_addrs=[relay_addr],
                reserve_relay=True,
                dial_via_relay=False,
                bootstrap_peers=[],
                connect_mode=ConnectMode.MANUAL,
            )
            b_cfg = NodeConfig(
                listen="/ip4/127.0.0.1/tcp/0",
                enable_mdns=False,
                enable_dcutr=False,
                allowlist={a_id},
                local_caps=CapsInfo([], "openai-compat", 0, 8192, False),
                keypair=b_kp,
                relay_addrs=[relay_addr],
                reserve_relay=False,
                dial_via_relay=True,
                bootstrap_peers=[a_id],
                connect_mode=ConnectMode.MANUAL,
            )

            a = PeerLmNode(a_cfg, backend)
            b = PeerLmNode(b_cfg, None)

            async with a.run():
                await trio.sleep(0.8)  # reservation settle
                async with b.run():
                    await trio.sleep(0.5)
                    body = json.dumps(
                        {
                            "model": MOCK_MODEL_ID,
                            "messages": [{"role": "user", "content": "via relay"}],
                        }
                    ).encode()
                    for _ in range(20):
                        try:
                            resp = await b.infer(a_id, body, False)
                            text = resp.body.decode(errors="replace")
                            if "peer-lm-mock" in text or "via relay" in text:
                                ok = True
                                break
                        except Exception as err:  # noqa: BLE001
                            last_err = err
                            await trio.sleep(0.4)
                    # Cancel nursery while still inside node contexts so
                    # relay/mock/services tear down via cancel, not hang on
                    # graceful CircuitV2/DCUtR stop.
                    nursery.cancel_scope.cancel()

    if scope.cancelled_caught and not ok:
        pytest.fail(f"relay infer test timed out; last_err={last_err!r}")
    assert ok, f"infer via local relay failed: {last_err!r}"
