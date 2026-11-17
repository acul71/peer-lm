"""libp2p node: discovery, auto ladder, caps/infer request-response."""

from __future__ import annotations

import logging
from contextlib import asynccontextmanager
from dataclasses import dataclass, field
from typing import Any, AsyncIterator

import multiaddr
import trio
from libp2p import new_host
from libp2p.crypto.keys import KeyPair
from libp2p.custom_types import TProtocol
from libp2p.discovery.events.peerDiscovery import peerDiscovery
from libp2p.peer.id import ID
from libp2p.peer.peerinfo import PeerInfo, info_from_p2p_addr
from libp2p.relay.circuit_v2.config import RelayConfig, RelayRole
from libp2p.relay.circuit_v2.dcutr import DCUtRProtocol
from libp2p.relay.circuit_v2.discovery import RelayDiscovery
from libp2p.relay.circuit_v2.protocol import (
    PROTOCOL_ID as HOP_PROTOCOL_ID,
    STOP_PROTOCOL_ID,
    CircuitV2Protocol,
)
from libp2p.relay.circuit_v2.resources import RelayLimits
from libp2p.relay.circuit_v2.transport import CircuitV2Transport
from libp2p.request_response import RequestContext, RequestResponse, RequestResponseConfig
from libp2p.tools.anyio_service import background_trio_service

from peer_lm.backend import BackendClient
from peer_lm.cbor_codec import CborMapCodec
from peer_lm.connect import AutoConnectTimers, ConnectMode, ResolvedConnect
from peer_lm.protocol import (
    CAPS_PROTOCOL,
    CapsInfo,
    InferResponse,
    decode_request,
    decode_response,
    encode_request_caps,
    encode_request_infer,
    encode_response_caps,
    encode_response_error,
    encode_response_infer,
)

logger = logging.getLogger("peer_lm.network")


@dataclass
class PeerSnapshot:
    peer_id: str
    addresses: list[str] = field(default_factory=list)
    caps: CapsInfo | None = None


@dataclass
class NodeConfig:
    listen: str = "/ip4/0.0.0.0/tcp/0"
    enable_mdns: bool = True
    enable_dcutr: bool = True
    allowlist: set[str] = field(default_factory=set)
    local_caps: CapsInfo = field(
        default_factory=lambda: CapsInfo([], "openai-compat", 0, 8192, False)
    )
    keypair: KeyPair | None = None
    relay_addrs: list[str] = field(default_factory=list)
    reserve_relay: bool = False
    dial_via_relay: bool = False
    rendezvous_addr: str | None = None
    mesh_namespace: str = "peer-lm"
    bootstrap_peers: list[str] = field(default_factory=list)
    connect_mode: ConnectMode = ConnectMode.MANUAL
    auto_timers: AutoConnectTimers = field(default_factory=AutoConnectTimers)


class _NullAsyncCM:
    async def __aenter__(self):
        return self

    async def __aexit__(self, *args):
        return False


class PeerLmNode:
    def __init__(
        self,
        config: NodeConfig,
        backend: BackendClient | None = None,
    ) -> None:
        self.config = config
        self.backend = backend
        self._peers: dict[str, PeerSnapshot] = {}
        self._host = None
        self._rr: RequestResponse | None = None
        self._transport: CircuitV2Transport | None = None
        self._codec = CborMapCodec()
        self._resolved = ResolvedConnect.resolve(
            config.connect_mode,
            config.enable_mdns,
            config.enable_dcutr,
            config.reserve_relay,
            config.dial_via_relay,
            bool(config.relay_addrs),
            config.auto_timers,
        )
        self._dial_via_relay = self._resolved.dial_via_relay
        self._auto_mdns_phase = self._resolved.mode is ConnectMode.AUTO
        self._auto_relay_escalated: set[str] = set()
        self._nursery: trio.Nursery | None = None

    @property
    def local_peer_id(self) -> str:
        assert self._host is not None
        return str(self._host.get_id())

    def _allowlisted(self, peer: str) -> bool:
        return not self.config.allowlist or peer in self.config.allowlist

    def _pid(self, peer: str) -> ID:
        return ID.from_string(peer)

    async def list_peers(self) -> list[PeerSnapshot]:
        return list(self._peers.values())

    async def pick_peer_for_model(self, model: str | None) -> str | None:
        best: tuple[str, int] | None = None
        for p in self._peers.values():
            caps = p.caps
            if caps is None or caps.busy or caps.slots_free == 0:
                continue
            if model and model not in caps.models:
                continue
            if best is None or caps.slots_free > best[1]:
                best = (p.peer_id, caps.slots_free)
        return best[0] if best else None

    async def _remember(self, peer_id: str, addr: str | None = None) -> None:
        snap = self._peers.get(peer_id) or PeerSnapshot(peer_id=peer_id)
        if addr and addr not in snap.addresses:
            snap.addresses.append(addr)
        self._peers[peer_id] = snap

    def _is_connected(self, peer: str) -> bool:
        assert self._host is not None
        try:
            return bool(self._host.get_network().connections.get(self._pid(peer)))
        except Exception:  # noqa: BLE001
            return False

    async def _handle_rr(
        self, request: dict[str, Any], context: RequestContext
    ) -> dict[str, Any]:
        peer = str(context.peer_id)
        if not self._allowlisted(peer):
            return encode_response_error(f"peer {peer} not allowlisted")
        kind, msg = decode_request(request)
        if kind == "caps":
            return encode_response_caps(self.config.local_caps)
        if kind == "infer":
            if self.backend is None:
                return encode_response_error("this node has no local backend")
            body = msg.get("body") or b""
            if isinstance(body, str):
                body = body.encode()
            stream = bool(msg.get("stream"))
            try:
                fwd = await self.backend.forward_chat_completions(bytes(body), stream)
                return encode_response_infer(
                    InferResponse(fwd.status, fwd.content_type, fwd.body)
                )
            except Exception as err:  # noqa: BLE001
                return encode_response_error(str(err))
        return encode_response_error(f"unknown request type {kind}")

    async def _ensure_connected(self, peer: str, via_relay: bool) -> None:
        assert self._host is not None
        if self._is_connected(peer):
            return
        if via_relay:
            if not self.config.relay_addrs:
                raise RuntimeError("dial_via_relay set but no relays configured")
            if self._transport is None:
                raise RuntimeError("circuit transport not ready")
            relay = multiaddr.Multiaddr(self.config.relay_addrs[0])
            relay_str = str(relay).replace("/ip4/0.0.0.0/", "/ip4/127.0.0.1/")
            circuit = multiaddr.Multiaddr(f"{relay_str}/p2p-circuit/p2p/{peer}")
            logger.info("dialing peer via relay circuit=%s", circuit)
            await self._transport.dial(circuit)
            return
        snap = self._peers.get(peer)
        if snap and snap.addresses:
            for a in snap.addresses:
                try:
                    maddr = multiaddr.Multiaddr(a if "/p2p/" in a else f"{a}/p2p/{peer}")
                    logger.info("dialing peer directly %s", maddr)
                    await self._host.connect(info_from_p2p_addr(maddr))
                    return
                except Exception as err:  # noqa: BLE001
                    logger.debug("direct dial attempt failed: %s", err)
        raise RuntimeError(f"no dialable address for peer {peer}")

    async def refresh_caps(self, peer: str) -> CapsInfo:
        assert self._rr is not None
        if not self._allowlisted(peer):
            raise RuntimeError(f"peer {peer} not allowlisted")
        try:
            await self._ensure_connected(peer, self._dial_via_relay)
        except Exception as err:  # noqa: BLE001
            raise RuntimeError(f"dialing peer {peer}; retry caps shortly: {err}") from err
        resp = await self._rr.send_request(
            peer_id=self._pid(peer),
            protocol_ids=[TProtocol(CAPS_PROTOCOL)],
            request=encode_request_caps(),
            codec=self._codec,
        )
        decoded = decode_response(resp)
        if isinstance(decoded, CapsInfo):
            await self._remember(peer)
            self._peers[peer].caps = decoded
            return decoded
        raise RuntimeError(str(decoded))

    async def infer(self, peer: str, body: bytes, stream: bool = False) -> InferResponse:
        assert self._rr is not None
        if not self._allowlisted(peer):
            raise RuntimeError(f"peer {peer} not allowlisted")
        try:
            await self._ensure_connected(peer, self._dial_via_relay)
        except Exception:
            if self._resolved.mode is ConnectMode.AUTO and self.config.relay_addrs:
                self._auto_relay_escalated.add(peer)
                await self._ensure_connected(peer, True)
            else:
                raise
        resp = await self._rr.send_request(
            peer_id=self._pid(peer),
            protocol_ids=[TProtocol(CAPS_PROTOCOL)],
            request=encode_request_infer(body, stream),
            codec=self._codec,
        )
        decoded = decode_response(resp)
        if isinstance(decoded, InferResponse):
            return decoded
        raise RuntimeError(str(decoded))

    async def _on_mdns(self, peerinfo: PeerInfo) -> None:
        peer = str(peerinfo.peer_id)
        if peer == self.local_peer_id or not self._allowlisted(peer):
            return
        addrs = [str(a) for a in (peerinfo.addrs or [])]
        logger.info("mdns discovered peer=%s addrs=%s", peer, addrs)
        for a in addrs:
            await self._remember(peer, a)
        try:
            if addrs:
                maddr = multiaddr.Multiaddr(
                    addrs[0] if "/p2p/" in addrs[0] else f"{addrs[0]}/p2p/{peer}"
                )
                await self._host.connect(info_from_p2p_addr(maddr))
            if self._resolved.mode is ConnectMode.AUTO:
                logger.info("auto: using mDNS/direct path peer=%s", peer)
            try:
                await self.refresh_caps(peer)
            except Exception as err:  # noqa: BLE001
                logger.warning("caps after mdns failed: %s", err)
        except Exception as err:  # noqa: BLE001
            logger.warning("mdns dial failed peer=%s err=%s", peer, err)
            if (
                self._resolved.mode is ConnectMode.AUTO
                and peer not in self._auto_relay_escalated
                and self.config.relay_addrs
            ):
                self._auto_relay_escalated.add(peer)
                logger.info("auto: escalating failed dial to relay peer=%s", peer)
                try:
                    await self._ensure_connected(peer, True)
                    await self.refresh_caps(peer)
                except Exception as err2:  # noqa: BLE001
                    logger.warning("auto escalate failed: %s", err2)

    async def _auto_mdns_timer(self) -> None:
        if self._resolved.mode is not ConnectMode.AUTO:
            return
        await trio.sleep(self._resolved.timers.mdns_timeout_secs)
        self._auto_mdns_phase = False
        if not self.config.relay_addrs:
            logger.warning("auto: mDNS window elapsed but no relays configured")
            return
        logger.info(
            "auto: mDNS window elapsed — dialing missing peers via relay "
            "(DCUtR may upgrade to direct)"
        )
        self._dial_via_relay = True
        for peer in self.config.bootstrap_peers:
            if self._is_connected(peer):
                logger.info("auto: peer already connected peer=%s", peer)
                continue
            self._auto_relay_escalated.add(peer)
            try:
                await self._ensure_connected(peer, True)
                await self.refresh_caps(peer)
                logger.info("auto: relay fallback dial ok peer=%s", peer)
            except Exception as err:  # noqa: BLE001
                logger.warning(
                    "auto: relay fallback dial failed peer=%s err=%s", peer, err
                )

    async def _bootstrap(self) -> None:
        for peer in self.config.bootstrap_peers:
            await self._remember(peer)
            if self._resolved.mode is ConnectMode.AUTO:
                logger.info(
                    "auto: waiting for mDNS before relay fallback peer=%s", peer
                )
                continue
            try:
                await self._ensure_connected(peer, self._dial_via_relay)
                await self.refresh_caps(peer)
            except Exception as err:  # noqa: BLE001
                logger.warning("bootstrap dial failed peer=%s err=%s", peer, err)

    @asynccontextmanager
    async def run(self) -> AsyncIterator[PeerLmNode]:
        cfg = self.config
        resolved = self._resolved
        logger.info(
            "connect strategy mode=%s mdns=%s dcutr=%s reserve=%s dial_via_relay=%s",
            resolved.mode.value,
            resolved.enable_mdns,
            resolved.enable_dcutr,
            resolved.reserve_relay,
            resolved.dial_via_relay,
        )
        assert cfg.keypair is not None
        host = new_host(key_pair=cfg.keypair, enable_mDNS=resolved.enable_mdns)
        self._host = host
        listen = multiaddr.Multiaddr(cfg.listen)

        limits = RelayLimits(
            duration=30 * 60,
            data=64 << 20,
            max_circuit_conns=16,
            max_reservations=128,
        )
        relay_config = RelayConfig(
            roles=RelayRole.STOP | RelayRole.CLIENT,
            limits=limits,
        )
        circuit_proto = CircuitV2Protocol(host, limits=limits, allow_hop=False)
        dcutr = DCUtRProtocol(host) if resolved.enable_dcutr else None

        rr = RequestResponse(host)
        self._rr = rr
        rr_cfg = RequestResponseConfig(
            timeout=300.0,
            max_request_size=32 << 20,
            max_response_size=32 << 20,
        )

        async with host.run(listen_addrs=[listen]), trio.open_nursery() as nursery:
            self._nursery = nursery
            nursery.start_soon(host.get_peerstore().start_cleanup_task, 60)

            def _mdns_handler(pi: PeerInfo) -> None:
                nursery.start_soon(self._on_mdns, pi)

            peerDiscovery.register_peer_discovered_handler(_mdns_handler)

            rr.set_handler(
                TProtocol(CAPS_PROTOCOL),
                handler=self._handle_rr,
                codec=self._codec,
                config=rr_cfg,
            )
            host.set_stream_handler(HOP_PROTOCOL_ID, circuit_proto._handle_hop_stream)
            host.set_stream_handler(STOP_PROTOCOL_ID, circuit_proto._handle_stop_stream)

            async with background_trio_service(circuit_proto):
                transport = CircuitV2Transport(host, circuit_proto, relay_config)
                self._transport = transport
                discovery = RelayDiscovery(
                    host,
                    auto_reserve=resolved.reserve_relay and bool(cfg.relay_addrs),
                )
                transport.discovery = discovery
                async with background_trio_service(discovery):
                    dcutr_cm = (
                        background_trio_service(dcutr)
                        if dcutr is not None
                        else _NullAsyncCM()
                    )
                    async with dcutr_cm:
                        logger.info("local peer id %s", self.local_peer_id)
                        for addr in host.get_addrs():
                            logger.info("listening %s", addr)

                        for ra in cfg.relay_addrs:
                            try:
                                info = info_from_p2p_addr(multiaddr.Multiaddr(ra))
                                logger.info("dialing relay %s", ra)
                                await host.connect(info)
                                if resolved.reserve_relay:
                                    # _add_relay auto-reserves when auto_reserve=True
                                    await discovery._add_relay(info.peer_id)
                                    relay_info = discovery._discovered_relays.get(
                                        info.peer_id
                                    )
                                    ok = bool(
                                        relay_info and relay_info.has_reservation
                                    )
                                    if not ok:
                                        ok = await discovery.make_reservation(
                                            info.peer_id
                                        )
                                    logger.info(
                                        "relay reservation %s relay_peer_id=%s",
                                        "accepted" if ok else "failed",
                                        info.peer_id,
                                    )
                                    if not ok:
                                        raise RuntimeError(
                                            f"relay reservation failed for {info.peer_id}"
                                        )
                            except Exception as err:  # noqa: BLE001
                                logger.warning("relay dial failed %s: %s", ra, err)

                        if cfg.rendezvous_addr:
                            try:
                                from libp2p.discovery.rendezvous import (
                                    RendezvousDiscovery,
                                )

                                rzv_info = info_from_p2p_addr(
                                    multiaddr.Multiaddr(cfg.rendezvous_addr)
                                )
                                await host.connect(rzv_info)
                                rzv = RendezvousDiscovery(host)
                                await rzv.register(cfg.mesh_namespace, rzv_info.peer_id)
                                found = await rzv.discover(
                                    cfg.mesh_namespace, rzv_info.peer_id
                                )
                                for p in found or []:
                                    pid = str(getattr(p, "peer_id", p))
                                    if self._allowlisted(pid):
                                        await self._remember(pid)
                            except Exception as err:  # noqa: BLE001
                                logger.warning("rendezvous setup failed: %s", err)

                        nursery.start_soon(self._bootstrap)
                        nursery.start_soon(self._auto_mdns_timer)
                        try:
                            yield self
                        finally:
                            # Prefer cancel over graceful service stop: CircuitV2
                            # / DCUtR manager.stop() can block indefinitely under
                            # pytest and on Ctrl+C paths with open circuits.
                            nursery.cancel_scope.cancel()
