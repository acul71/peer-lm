"""OpenAI-compatible localhost gateway."""

from __future__ import annotations

import json
import logging
from typing import Any

from hypercorn.config import Config as HyperConfig
from hypercorn.trio import serve as hypercorn_serve
from starlette.applications import Starlette
from starlette.requests import Request
from starlette.responses import JSONResponse, Response
from starlette.routing import Route

from peer_lm.backend import BackendClient
from peer_lm.network import PeerLmNode

logger = logging.getLogger("peer_lm.gateway")


def build_gateway_app(
    node: PeerLmNode,
    backend: BackendClient | None,
    prefer_local: bool = True,
) -> Starlette:
    async def health(_: Request) -> JSONResponse:
        if backend is not None:
            h = await backend.health()
            local_ok, detail = h.ok, h.detail
        else:
            local_ok, detail = False, "no local backend"
        peers = await node.list_peers()
        return JSONResponse(
            {
                "status": "ok",
                "local_peer_id": node.local_peer_id,
                "local_backend_ok": local_ok,
                "local_backend_detail": detail,
                "peers": len(peers),
            }
        )

    async def peers(_: Request) -> JSONResponse:
        data = []
        for p in await node.list_peers():
            data.append(
                {
                    "peer_id": p.peer_id,
                    "addresses": p.addresses,
                    "caps": None if p.caps is None else p.caps.to_dict(),
                }
            )
        return JSONResponse({"peers": data})

    async def models(_: Request) -> Response:
        if backend is not None:
            h = await backend.health()
            if h.ok:
                try:
                    return JSONResponse(await backend.get_models_json())
                except Exception as err:  # noqa: BLE001
                    logger.warning("local models failed: %s", err)
        out = []
        for p in await node.list_peers():
            if p.caps:
                for m in p.caps.models:
                    out.append(
                        {
                            "id": m,
                            "object": "model",
                            "created": 0,
                            "owned_by": f"peer:{p.peer_id}",
                        }
                    )
        return JSONResponse({"object": "list", "data": out})

    async def chat(request: Request) -> Response:
        body = await request.body()
        try:
            parsed: dict[str, Any] = json.loads(body)
        except Exception:  # noqa: BLE001
            parsed = {}
        stream = bool(parsed.get("stream"))
        model = parsed.get("model")
        if isinstance(model, str):
            model_s: str | None = model
        else:
            model_s = None

        if prefer_local and backend is not None:
            h = await backend.health()
            if h.ok:
                try:
                    fwd = await backend.forward_chat_completions(body, stream)
                    return Response(
                        content=fwd.body,
                        status_code=fwd.status,
                        media_type=fwd.content_type,
                    )
                except Exception as err:  # noqa: BLE001
                    logger.warning("local backend failed; trying peers: %s", err)

        peer = await node.pick_peer_for_model(model_s)
        if peer is None:
            peers_list = await node.list_peers()
            peer = peers_list[0].peer_id if peers_list else None
        if peer is None:
            return JSONResponse(
                {
                    "error": {
                        "message": "no local backend and no capable peers",
                        "type": "peer_lm_unavailable",
                    }
                },
                status_code=503,
            )
        try:
            resp = await node.infer(peer, body, stream)
            return Response(
                content=resp.body,
                status_code=resp.status,
                media_type=resp.content_type,
            )
        except Exception as err:  # noqa: BLE001
            return JSONResponse(
                {
                    "error": {
                        "message": f"peer infer failed: {err}",
                        "type": "peer_lm_remote_error",
                        "peer": peer,
                    }
                },
                status_code=502,
            )

    return Starlette(
        routes=[
            Route("/health", health),
            Route("/v1/models", models),
            Route("/v1/chat/completions", chat, methods=["POST"]),
            Route("/peer-lm/peers", peers),
        ]
    )


async def serve_gateway(bind: str, app: Starlette) -> None:
    host, _, port_s = bind.partition(":")
    cfg = HyperConfig()
    cfg.bind = [f"{host}:{port_s or '3000'}"]
    cfg.worker_class = "trio"
    logger.info("gateway listening on http://%s", bind)
    await hypercorn_serve(app, cfg)
