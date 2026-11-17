"""CBOR codec for peer-lm request/response maps."""

from __future__ import annotations

from typing import Any

import cbor2

from libp2p.request_response import RequestResponseCodec


class CborMapCodec(RequestResponseCodec[dict[str, Any], dict[str, Any]]):
    def encode_request(self, request: dict[str, Any]) -> bytes:
        return cbor2.dumps(request)

    def decode_request(self, payload: bytes) -> dict[str, Any]:
        return cbor2.loads(payload)

    def encode_response(self, response: dict[str, Any]) -> bytes:
        return cbor2.dumps(response)

    def decode_response(self, payload: bytes) -> dict[str, Any]:
        return cbor2.loads(payload)
