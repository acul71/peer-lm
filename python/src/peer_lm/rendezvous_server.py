"""Rendezvous point server."""

from __future__ import annotations

import logging

import multiaddr
import trio
from libp2p import new_host
from libp2p.crypto.keys import KeyPair
from libp2p.discovery.rendezvous import RendezvousService

logger = logging.getLogger("peer_lm.rendezvous")


async def run_rendezvous_server(keypair: KeyPair, listen: str) -> None:
    host = new_host(key_pair=keypair)
    listen_addr = multiaddr.Multiaddr(listen)
    async with host.run(listen_addrs=[listen_addr]), trio.open_nursery() as nursery:
        nursery.start_soon(host.get_peerstore().start_cleanup_task, 60)
        service = RendezvousService(host)
        peer_id = host.get_id()
        logger.info("rendezvous started local_peer_id=%s", peer_id)
        for addr in host.get_addrs():
            logger.info("rendezvous listening full=%s/p2p/%s", addr, peer_id)
        while True:
            await trio.sleep(30)
            stats = service.get_namespace_stats()
            if stats:
                logger.info("namespace stats=%s", stats)
