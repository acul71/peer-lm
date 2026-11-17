"""peer-lm CLI (Python)."""

from __future__ import annotations

import logging
import os
from pathlib import Path
from typing import Optional

import trio
import typer
from hypercorn.config import Config as HyperConfig
from hypercorn.trio import serve as hypercorn_serve

from peer_lm.backend import BackendClient
from peer_lm.config import FileConfig
from peer_lm.connect import AutoConnectTimers, ConnectMode
from peer_lm.gateway import build_gateway_app, serve_gateway
from peer_lm.identity import load_or_create_identity, peer_id_str
from peer_lm.mock_backend import MOCK_MODEL_ID, build_mock_app
from peer_lm.network import NodeConfig, PeerLmNode
from peer_lm.protocol import CapsInfo
from peer_lm.relay_server import run_relay_server
from peer_lm.rendezvous_server import run_rendezvous_server

app = typer.Typer(name="peer-lm", help="OpenAI-compatible local gateway over libp2p")
logging.basicConfig(
    level=os.environ.get("PEER_LM_LOG", "INFO"),
    format="%(asctime)s %(levelname)s %(name)s: %(message)s",
)
logger = logging.getLogger("peer_lm")


@app.command()
def keygen(
    out: Optional[Path] = typer.Option(None, help="Write identity protobuf bytes"),
) -> None:
    """Print a new peer id; optionally persist identity."""
    kp = load_or_create_identity(None)
    print(peer_id_str(kp))
    if out is not None:
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_bytes(kp.private_key.serialize())
        typer.echo(f"wrote identity to {out}", err=True)


@app.command()
def mock(
    listen: str = typer.Option("127.0.0.1:8080"),
    model: str = typer.Option(MOCK_MODEL_ID),
) -> None:
    """Run only the tiny OpenAI-compatible mock backend."""

    async def _run() -> None:
        application = build_mock_app(model)
        cfg = HyperConfig()
        cfg.bind = [listen]
        logger.info("mock backend on http://%s", listen)
        await hypercorn_serve(application, cfg)

    trio.run(_run)


@app.command()
def relay(
    listen: str = typer.Option("/ip4/0.0.0.0/tcp/4001"),
    identity: Optional[Path] = typer.Option(None),
) -> None:
    """Run a Circuit Relay v2 server."""
    kp = load_or_create_identity(identity)
    trio.run(run_relay_server, kp, listen)


@app.command()
def rendezvous(
    listen: str = typer.Option("/ip4/0.0.0.0/tcp/4001"),
    identity: Optional[Path] = typer.Option(None),
) -> None:
    """Run a rendezvous point."""
    kp = load_or_create_identity(identity)
    trio.run(run_rendezvous_server, kp, listen)


@app.command()
def serve(
    config: Optional[Path] = typer.Option(None, "--config", "-c"),
    gateway: str = typer.Option("127.0.0.1:3000"),
    listen: str = typer.Option("/ip4/0.0.0.0/tcp/0"),
    backend: Optional[str] = typer.Option(None, envvar="PEER_LM_BACKEND_URL"),
    embedded_mock: bool = typer.Option(False, "--embedded-mock"),
    prefer_local: bool = typer.Option(True, "--prefer-local/--no-prefer-local"),
    connect_mode: Optional[str] = typer.Option(None, "--connect-mode"),
    mdns_timeout_secs: Optional[float] = typer.Option(None),
    hole_punch_grace_secs: Optional[float] = typer.Option(None),
    mdns: bool = typer.Option(True, "--mdns/--no-mdns"),
    dcutr: bool = typer.Option(True, "--dcutr/--no-dcutr"),
    relay: Optional[list[str]] = typer.Option(None, "--relay"),
    reserve_relay: bool = typer.Option(False, "--reserve-relay"),
    dial_via_relay: bool = typer.Option(False, "--dial-via-relay"),
    rendezvous_addr: Optional[str] = typer.Option(None, "--rendezvous"),
    mesh_namespace: str = typer.Option("peer-lm"),
    allowlist: str = typer.Option(""),
    dial_peer: Optional[list[str]] = typer.Option(None, "--dial-peer"),
    identity: Optional[Path] = typer.Option(None),
) -> None:
    """Run gateway + libp2p node."""

    file = FileConfig.load(config) if config else None

    gateway_b = (file.gateway if file and file.gateway else None) or gateway
    listen_b = (file.listen if file and file.listen else None) or listen
    backend_b = backend or (file.backend if file else None)
    embedded = embedded_mock or bool(file and file.embedded_mock)
    prefer = prefer_local if file is None or file.prefer_local is None else file.prefer_local
    if not prefer_local:
        prefer = False
    identity_b = identity or (Path(file.identity) if file and file.identity else None)

    if allowlist:
        allow = {x.strip() for x in allowlist.split(",") if x.strip()}
    elif file and file.allowlist:
        allow = set(file.allowlist)
    else:
        allow = set()

    if connect_mode:
        mode = ConnectMode.parse(connect_mode)
    elif file:
        mode = file.connect_mode()
    else:
        mode = ConnectMode.MANUAL

    enable_mdns = mdns if file is None or file.connect.mdns is None else file.connect.mdns
    if not mdns:
        enable_mdns = False
    enable_dcutr = dcutr if file is None or file.connect.dcutr is None else file.connect.dcutr
    if not dcutr:
        enable_dcutr = False

    relays = list(relay or [])
    if not relays and file and file.connect.relays:
        relays = list(file.connect.relays)

    reserve = reserve_relay or bool(file and file.connect.reserve_relay)
    via_relay = dial_via_relay or bool(file and file.connect.dial_via_relay)

    dials = list(dial_peer or [])
    if not dials and file and file.connect.dial_peers:
        dials = list(file.connect.dial_peers)

    rzv = rendezvous_addr or (file.connect.rendezvous if file else None)
    ns = (
        file.connect.mesh_namespace
        if file and file.connect.mesh_namespace
        else mesh_namespace
    )

    timers = file.auto_timers() if file else AutoConnectTimers()
    if mdns_timeout_secs is not None:
        timers.mdns_timeout_secs = mdns_timeout_secs
    if hole_punch_grace_secs is not None:
        timers.hole_punch_grace_secs = hole_punch_grace_secs

    async def _run() -> None:
        nonlocal backend_b
        backend_client: BackendClient | None = None
        mock_task_started = False

        if embedded:
            # Bind mock on ephemeral port via hypercorn in nursery
            import socket

            sock = socket.socket()
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
            sock.close()
            mock_bind = f"127.0.0.1:{port}"
            backend_b = f"http://{mock_bind}"
            mock_app = build_mock_app()
            mock_cfg = HyperConfig()
            mock_cfg.bind = [mock_bind]
            mock_task_started = True
            logger.info("embedded mock backend started url=%s", backend_b)

        if backend_b:
            backend_client = BackendClient(backend_b)

        models: list[str] = []
        if backend_client:
            try:
                mj = await backend_client.get_models_json()
                models = [
                    m["id"]
                    for m in (mj.get("data") or [])
                    if isinstance(m, dict) and m.get("id")
                ]
            except Exception:  # noqa: BLE001
                models = [MOCK_MODEL_ID] if embedded else []
        caps = CapsInfo(
            models=models,
            engine="mock" if embedded else "openai-compat",
            slots_free=1 if backend_client else 0,
            max_ctx=8192,
            busy=False,
        )
        kp = load_or_create_identity(identity_b)
        node_cfg = NodeConfig(
            listen=listen_b,
            enable_mdns=enable_mdns,
            enable_dcutr=enable_dcutr,
            allowlist=allow,
            local_caps=caps,
            keypair=kp,
            relay_addrs=relays,
            reserve_relay=reserve,
            dial_via_relay=via_relay,
            rendezvous_addr=rzv,
            mesh_namespace=ns,
            bootstrap_peers=dials,
            connect_mode=mode,
            auto_timers=timers,
        )
        node = PeerLmNode(node_cfg, backend_client)
        gw_app = build_gateway_app(node, backend_client, prefer_local=prefer)

        async with trio.open_nursery() as nursery:
            if mock_task_started:
                nursery.start_soon(hypercorn_serve, mock_app, mock_cfg)

            async with node.run():
                await serve_gateway(gateway_b, gw_app)

    trio.run(_run)


def main() -> None:
    app()


if __name__ == "__main__":
    main()
