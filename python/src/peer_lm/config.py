"""YAML configuration loader (parity with Rust FileConfig)."""

from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import yaml

from peer_lm.connect import AutoConnectTimers, ConnectMode


@dataclass
class ConnectFileConfig:
    mode: str | None = None
    mdns: bool | None = None
    dcutr: bool | None = None
    reserve_relay: bool | None = None
    dial_via_relay: bool | None = None
    mdns_timeout_secs: float | None = None
    hole_punch_grace_secs: float | None = None
    relays: list[str] | None = None
    dial_peers: list[str] | None = None
    rendezvous: str | None = None
    mesh_namespace: str | None = None


@dataclass
class FileConfig:
    gateway: str | None = None
    listen: str | None = None
    backend: str | None = None
    embedded_mock: bool | None = None
    prefer_local: bool | None = None
    identity: str | None = None
    allowlist: list[str] | None = None
    connect: ConnectFileConfig = field(default_factory=ConnectFileConfig)

    @classmethod
    def load(cls, path: Path) -> FileConfig:
        raw = yaml.safe_load(path.read_text()) or {}
        c = raw.get("connect") or {}
        connect = ConnectFileConfig(
            mode=c.get("mode"),
            mdns=c.get("mdns"),
            dcutr=c.get("dcutr"),
            reserve_relay=c.get("reserve_relay"),
            dial_via_relay=c.get("dial_via_relay"),
            mdns_timeout_secs=c.get("mdns_timeout_secs"),
            hole_punch_grace_secs=c.get("hole_punch_grace_secs"),
            relays=c.get("relays"),
            dial_peers=c.get("dial_peers"),
            rendezvous=c.get("rendezvous"),
            mesh_namespace=c.get("mesh_namespace"),
        )
        return cls(
            gateway=raw.get("gateway"),
            listen=raw.get("listen"),
            backend=raw.get("backend"),
            embedded_mock=raw.get("embedded_mock"),
            prefer_local=raw.get("prefer_local"),
            identity=raw.get("identity"),
            allowlist=raw.get("allowlist"),
            connect=connect,
        )

    def connect_mode(self) -> ConnectMode:
        if not self.connect.mode:
            return ConnectMode.MANUAL
        return ConnectMode.parse(self.connect.mode)

    def auto_timers(self) -> AutoConnectTimers:
        t = AutoConnectTimers()
        if self.connect.mdns_timeout_secs is not None:
            t.mdns_timeout_secs = float(self.connect.mdns_timeout_secs)
        if self.connect.hole_punch_grace_secs is not None:
            t.hole_punch_grace_secs = float(self.connect.hole_punch_grace_secs)
        return t
