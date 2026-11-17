import json

import httpx
import pytest
import trio
from hypercorn.config import Config
from hypercorn.trio import serve

from peer_lm.mock_backend import MOCK_MODEL_ID, build_mock_app


@pytest.fixture
async def mock_url(nursery):
    import socket

    sock = socket.socket()
    sock.bind(("127.0.0.1", 0))
    port = sock.getsockname()[1]
    sock.close()
    bind = f"127.0.0.1:{port}"
    app = build_mock_app()
    cfg = Config()
    cfg.bind = [bind]
    nursery.start_soon(serve, app, cfg)
    await trio.sleep(0.2)
    yield f"http://{bind}"


async def test_health_and_chat(mock_url):
    async with httpx.AsyncClient() as client:
        h = await client.get(f"{mock_url}/health")
        assert h.status_code == 200
        assert h.json()["status"] == "ok"
        m = await client.get(f"{mock_url}/v1/models")
        assert m.json()["data"][0]["id"] == MOCK_MODEL_ID
        c = await client.post(
            f"{mock_url}/v1/chat/completions",
            json={
                "model": MOCK_MODEL_ID,
                "messages": [{"role": "user", "content": "hello"}],
            },
        )
        assert c.status_code == 200
        body = c.json()
        assert "hello" in body["choices"][0]["message"]["content"]
