"""Wire types for /peer-lm/caps/1.0.0 (CBOR maps)."""

from __future__ import annotations

from dataclasses import asdict, dataclass
from typing import Any

CAPS_PROTOCOL = "/peer-lm/caps/1.0.0"
INFER_PROTOCOL = "/peer-lm/infer/1.0.0"  # exported for parity; wire uses CAPS_PROTOCOL


@dataclass
class CapsInfo:
    models: list[str]
    engine: str
    slots_free: int
    max_ctx: int
    busy: bool

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> CapsInfo:
        return cls(
            models=list(d.get("models") or []),
            engine=str(d.get("engine") or ""),
            slots_free=int(d.get("slots_free") or 0),
            max_ctx=int(d.get("max_ctx") or 0),
            busy=bool(d.get("busy") or False),
        )


@dataclass
class InferResponse:
    status: int
    content_type: str
    body: bytes

    def to_dict(self) -> dict[str, Any]:
        return {
            "status": self.status,
            "content_type": self.content_type,
            "body": self.body,
        }

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> InferResponse:
        body = d.get("body") or b""
        if isinstance(body, str):
            body = body.encode()
        return cls(
            status=int(d.get("status") or 500),
            content_type=str(d.get("content_type") or "application/json"),
            body=bytes(body),
        )


def encode_request_caps() -> dict[str, Any]:
    return {"type": "caps"}


def encode_request_infer(body: bytes, stream: bool) -> dict[str, Any]:
    return {"type": "infer", "body": body, "stream": stream}


def decode_request(msg: dict[str, Any]) -> tuple[str, dict[str, Any]]:
    t = str(msg.get("type") or "")
    return t, msg


def encode_response_caps(caps: CapsInfo) -> dict[str, Any]:
    return {"type": "caps", "caps": caps.to_dict()}


def encode_response_infer(resp: InferResponse) -> dict[str, Any]:
    return {"type": "infer", "infer": resp.to_dict()}


def encode_response_error(err: str) -> dict[str, Any]:
    return {"type": "error", "error": err}


def decode_response(msg: dict[str, Any]) -> CapsInfo | InferResponse | str:
    t = str(msg.get("type") or "")
    if t == "caps":
        return CapsInfo.from_dict(msg.get("caps") or {})
    if t == "infer":
        return InferResponse.from_dict(msg.get("infer") or {})
    if t == "error":
        return str(msg.get("error") or "unknown error")
    raise ValueError(f"unknown response type: {t}")
