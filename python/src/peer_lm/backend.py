"""OpenAI-compatible backend HTTP client."""

from __future__ import annotations

from dataclasses import dataclass

import httpx


@dataclass
class BackendHealth:
    ok: bool
    detail: str


@dataclass
class ForwardedResponse:
    status: int
    content_type: str
    body: bytes


class BackendClient:
    def __init__(self, base_url: str, timeout: float = 300.0) -> None:
        self.base_url = base_url.rstrip("/")
        self._client = httpx.AsyncClient(timeout=timeout)

    async def aclose(self) -> None:
        await self._client.aclose()

    async def health(self) -> BackendHealth:
        try:
            resp = await self._client.get(f"{self.base_url}/health")
            if resp.is_success:
                return BackendHealth(True, f"status {resp.status_code}")
            return BackendHealth(False, f"status {resp.status_code}")
        except Exception as err:  # noqa: BLE001
            return BackendHealth(False, str(err))

    async def get_models_json(self) -> dict:
        resp = await self._client.get(f"{self.base_url}/v1/models")
        resp.raise_for_status()
        return resp.json()

    async def forward_chat_completions(
        self, body: bytes, stream: bool = False
    ) -> ForwardedResponse:
        headers = {"content-type": "application/json"}
        if stream:
            headers["accept"] = "text/event-stream"
        resp = await self._client.post(
            f"{self.base_url}/v1/chat/completions",
            content=body,
            headers=headers,
        )
        return ForwardedResponse(
            status=resp.status_code,
            content_type=resp.headers.get("content-type", "application/json"),
            body=resp.content,
        )
