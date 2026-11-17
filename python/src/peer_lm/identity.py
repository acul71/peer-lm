"""Ed25519 identity load/create (protobuf private key bytes)."""

from __future__ import annotations

from pathlib import Path

from libp2p.crypto.ed25519 import create_new_key_pair
from libp2p.crypto.keys import KeyPair, PrivateKey
from libp2p.crypto.serialization import deserialize_private_key


def load_or_create_identity(path: Path | None) -> KeyPair:
    if path is None:
        return create_new_key_pair()
    path = Path(path)
    if path.exists():
        data = path.read_bytes()
        try:
            priv = deserialize_private_key(data)
            return KeyPair(priv, priv.get_public_key())
        except Exception:
            # Fallback: raw 32-byte seed
            return create_new_key_pair(data if len(data) >= 32 else data.ljust(32, b"\0"))
    path.parent.mkdir(parents=True, exist_ok=True)
    kp = create_new_key_pair()
    path.write_bytes(kp.private_key.serialize())
    return kp


def peer_id_str(kp: KeyPair) -> str:
    from libp2p.peer.id import ID

    return str(ID.from_pubkey(kp.public_key))
