"""Circuit Relay v2 server."""

from __future__ import annotations

import logging

import multiaddr
import trio
from libp2p import new_host
from libp2p.crypto.keys import KeyPair
from libp2p.relay.circuit_v2.config import RelayConfig, RelayRole
from libp2p.relay.circuit_v2.protocol import (
    PROTOCOL_ID,
    STOP_PROTOCOL_ID,
    CircuitV2Protocol,
)
from libp2p.relay.circuit_v2.resources import RelayLimits
from libp2p.relay.circuit_v2.transport import CircuitV2Transport
from libp2p.tools.anyio_service import background_trio_service

logger = logging.getLogger("peer_lm.relay")


async def run_relay_server(keypair: KeyPair, listen: str) -> None:
    host = new_host(key_pair=keypair)
    limits = RelayLimits(
        duration=30 * 60,
        data=64 << 20,
        max_circuit_conns=16,
        max_reservations=128,
    )
    relay_config = RelayConfig(
        roles=RelayRole.HOP | RelayRole.STOP | RelayRole.CLIENT,
        limits=limits,
    )
    protocol = CircuitV2Protocol(host, limits=limits, allow_hop=True)
    listen_addr = multiaddr.Multiaddr(listen)

    async with host.run(listen_addrs=[listen_addr]), trio.open_nursery() as nursery:
        nursery.start_soon(host.get_peerstore().start_cleanup_task, 60)
        host.set_stream_handler(PROTOCOL_ID, protocol._handle_hop_stream)
        host.set_stream_handler(STOP_PROTOCOL_ID, protocol._handle_stop_stream)

        async with background_trio_service(protocol):
            _transport = CircuitV2Transport(host, protocol, relay_config)
            peer_id = host.get_id()
            logger.info("relay server started local_peer_id=%s", peer_id)
            for addr in host.get_addrs():
                addr_s = str(addr)
                full = (
                    addr_s
                    if f"/p2p/{peer_id}" in addr_s
                    else f"{addr_s}/p2p/{peer_id}"
                )
                logger.info("relay listening (share this multiaddr) full=%s", full)
            await trio.sleep_forever()
