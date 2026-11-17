"""Connectivity strategies (parity with Rust ConnectMode)."""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum


class ConnectMode(str, Enum):
    MANUAL = "manual"
    LAN = "lan"
    RELAY = "relay"
    AUTO = "auto"

    @classmethod
    def parse(cls, raw: str) -> ConnectMode:
        s = raw.strip().lower()
        if s in ("mdns",):
            return cls.LAN
        try:
            return cls(s)
        except ValueError as e:
            raise ValueError(
                f"unknown connect mode '{raw}' (expected manual|lan|relay|auto)"
            ) from e


@dataclass
class AutoConnectTimers:
    mdns_timeout_secs: float = 5.0
    hole_punch_grace_secs: float = 15.0


@dataclass
class ResolvedConnect:
    mode: ConnectMode
    enable_mdns: bool
    enable_dcutr: bool
    reserve_relay: bool
    dial_via_relay: bool
    timers: AutoConnectTimers

    @classmethod
    def resolve(
        cls,
        mode: ConnectMode,
        enable_mdns: bool,
        enable_dcutr: bool,
        reserve_relay: bool,
        dial_via_relay: bool,
        has_relays: bool,
        timers: AutoConnectTimers | None = None,
    ) -> ResolvedConnect:
        timers = timers or AutoConnectTimers()
        if mode is ConnectMode.MANUAL:
            return cls(mode, enable_mdns, enable_dcutr, reserve_relay, dial_via_relay, timers)
        if mode is ConnectMode.LAN:
            return cls(mode, True, False, False, False, timers)
        if mode is ConnectMode.RELAY:
            return cls(mode, False, enable_dcutr, has_relays, has_relays, timers)
        # AUTO
        return cls(
            mode,
            True,
            has_relays and enable_dcutr,
            has_relays,
            False,
            timers,
        )
