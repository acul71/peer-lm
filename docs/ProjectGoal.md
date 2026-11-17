# Project Goal: Replicating LM Link with libp2p

## Project Vision (peer-lm)

The goal is to replicate **LM Studio's LM Link** using **libp2p** instead of Tailscale.

### What LM Link Does

1. **Remote inference**: Run a large LLM on a powerful machine (e.g. a GPU workstation) and access it from a weaker client (e.g. a low-spec laptop) as if the model were local.
2. **Discovery**: Devices find each other automatically.
3. **Security**: Traffic is encrypted end-to-end.
4. **NAT traversal**: Works across different networks (home, office, mobile).

### Our Equivalent (libp2p)

| LM Link (Tailscale) | libp2p Equivalent |
|--------------------|-------------------|
| Device discovery   | mDNS, Rendezvous, DHT |
| Encrypted links    | libp2p-noise / libp2p-tls |
| NAT traversal      | Circuit relay v2, hole-punching |
| Point-to-point     | libp2p TCP/QUIC/WebSocket transports |

---

## How libp2p Replicates the Tailscale/LM Link Mesh VPN Model

LM Link builds on **Tailscale mesh VPNs**: devices form a private network over the internet, with end-to-end encryption, automatic discovery, and NAT traversal so that no ports need to be opened to the public internet. This section explains how libp2p provides equivalent capabilities for our use case.

### Tailscale/LM Link: What We're Matching

Tailscale creates a **mesh VPN** (tailnet):

- **Mesh topology**: Peers connect directly when possible, instead of routing everything through a central gateway (which would add latency and become a bottleneck).
- **End-to-end encryption**: WireGuard encrypts all traffic between devices.
- **NAT traversal**: Works behind firewalls and NAT without manual port forwarding; uses relay servers only when direct connection fails.
- **No public exposure**: Devices never listen on public IPs; the coordination service helps with discovery and hole-punching, but does not relay data.
- **Discovery**: Devices are added to a tailnet and can find each other via Tailscale’s coordination servers.

For LM Link, this lets you run models on remote machines (home, office, cloud) and access them as if they were local, without opening your machines to the internet.

### libp2p: Application-Layer Mesh Networking

libp2p is **not a VPN** in the traditional sense—it does not create a virtual network interface or route all traffic. Instead, it provides **application-layer P2P connections** between peers. For peer-lm (LLM inference over streams), that is enough: we need secure, traversable connections between specific peers, not a full virtual LAN.

libp2p achieves similar outcomes through its own mechanisms:

| Tailscale/LM Link capability | How libp2p provides it |
|-----------------------------|------------------------|
| **Mesh / direct P2P** | libp2p prefers direct connections. When two peers can reach each other (same LAN or at least one has a public IP), they connect directly. Relay is used only when necessary. |
| **End-to-end encryption** | Every libp2p connection is secured with **Noise** or **TLS** before any application data is sent. No plaintext on the wire. |
| **NAT traversal** | **Circuit Relay v2**: a relay peer forwards traffic between two peers that cannot connect directly. **Hole-punching**: UDP-based techniques to establish direct connections through NAT when possible. |
| **No public ports** | A client behind NAT can connect *outbound* to a relay or a peer with a public address. The client never needs to accept inbound connections; the relay facilitates connectivity. |
| **Discovery** | **mDNS** (LAN), **Rendezvous** (central “room” server), **DHT (Kademlia)** (distributed lookup). No single Tailscale-style coordination service is required. |

### Connection Topology: Direct vs Relay

```
                    Direct connection (preferred)
    ┌─────────────┐                              ┌─────────────┐
    │   Client    │◄──── encrypted stream ───────►│   Server    │
    │ (Mac mini)  │      (low latency)            │ (Dell GPU)  │
    └─────────────┘                              └─────────────┘
         │                                               │
         │  same LAN, or at least one has public IP      │
         └─────────────────────────────────────────────┘


                    Via relay (when direct fails)
    ┌─────────────┐         ┌─────────────┐         ┌─────────────┐
    │   Client    │         │   Relay     │         │   Server    │
    │ (behind     │◄───────►│   peer      │◄───────►│ (behind     │
    │  NAT)       │  enc.   │ (reachable) │  enc.   │  NAT)       │
    └─────────────┘         └─────────────┘         └─────────────┘
         │                        │                        │
         └── outbound only ───────┴── outbound only ───────┘
```

Both paths are encrypted. The relay never sees plaintext; it forwards ciphertext. This mirrors Tailscale’s use of relays when direct connection is not possible.

### Trust and Coordination

- **Tailscale/LM Link**: Tailscale operates coordination servers for discovery and NAT traversal. You trust Tailscale (and LM Studio) for that. The mesh itself is end-to-end encrypted.
- **libp2p**: You choose who runs discovery and relay:
  - **Rendezvous**: You run the rendezvous server (or use a public one); peers register and discover each other in namespaces.
  - **Relay**: You run relay nodes, or rely on public/libp2p bootstrap relays.
  - **DHT**: Fully distributed; no central coordination, but more setup.

For peer-lm, a small group of your own devices can use a private rendezvous server and optionally your own relays, keeping control over coordination and infrastructure.

### Differences from Tailscale/LM Link

| Aspect | Tailscale/LM Link | libp2p (peer-lm) |
|--------|-------------------|------------------|
| **Layer** | Network layer (VPN, virtual NIC) | Application layer (streams/sockets) |
| **Scope** | All apps can use the tailnet | Only apps using libp2p (our LLM client/server) |
| **Coordination** | Tailscale / LM Studio backend | Your choice: rendezvous, DHT, or fixed addresses |
| **Setup** | Login, link devices in LM Studio | Configure peers, discovery, optional relay |
| **Maturity** | Production mesh VPN | libp2p is production-grade in many projects (IPFS, Filecoin, etc.) |

### Summary: What peer-lm Gets from libp2p

For remote LLM inference, we need:

- ✅ **Encrypted connections** — Noise/TLS on every stream  
- ✅ **Mesh-style topology** — Direct when possible, relay when needed  
- ✅ **NAT traversal** — Without opening ports on client or server  
- ✅ **Discovery** — Find LLM servers via mDNS, rendezvous, or DHT  
- ✅ **Privacy** — No requirement to send data through third parties; relay and discovery can be self-hosted  

libp2p provides all of this at the application layer. It does not give a full VPN tailnet like Tailscale, but for “run a model on machine A, call it from machine B securely over the internet” it offers equivalent guarantees and control.

---

## How py-libp2p Works

### Core Concepts

1. **Host** (`new_host`) — Your peer identity + network stack:
   - Key pair (identity)
   - Transports (TCP, QUIC, WebSocket)
   - Security (Noise, TLS)
   - Stream muxers (mplex, yamux)
   - Optional discovery (mDNS, bootstrap, rendezvous, DHT)
   - Optional circuit relay for NAT traversal

2. **Stream handler** — What runs when a peer opens a stream to your protocol:
   ```python
   host.set_stream_handler(PROTOCOL_ID, my_handler)
   ```

3. **Stream** — Bidirectional channel for request/response:
   ```python
   stream = await host.new_stream(peer_id, [PROTOCOL_ID])
   await stream.write(data)
   response = await stream.read()
   ```

4. **Peer connection** — Direct or via relay:
   ```python
   info = info_from_p2p_addr(multiaddr)
   await host.connect(info)
   ```

### Relevant Examples in py-libp2p

| Example | Role |
|---------|------|
| **`echo/echo.py`** | Simple request/response (closest to LLM inference pattern) |
| **`chat/chat.py`** | Bidirectional stream, read/write from both ends |
| **`rendezvous/rendezvous.py`** | Discovery without knowing exact addresses |
| **`circuit_relay/relay_example.py`** | NAT traversal via relay |
| **`kademlia/kademlia.py`** | DHT-based peer lookup |

### The Echo Pattern Maps Well to LLM Inference

```python
# Server
host.set_stream_handler(PROTOCOL_ID, _echo_stream_handler)  # receives msg, sends back

# Client
stream = await host.new_stream(info.peer_id, [PROTOCOL_ID])
await stream.write(msg)
response = await stream.read()
```

For an LLM: server handler receives a prompt, runs inference, writes response. Client sends prompt, reads tokens back.

### Discovery Options

- **Direct multiaddr** — like echo/chat: `/ip4/IP/tcp/PORT/p2p/PEER_ID`
- **mDNS** — LAN discovery
- **Rendezvous** — central "room" for peers to find each other
- **DHT (Kademlia)** — fully distributed lookup (more complex setup)

---

## Architecture for peer-lm

```
┌──────────────────────────────────────────────────────────────────┐
│                        LLM SERVER (GPU machine)                    │
│  - libp2p host (listener)                                          │
│  - Stream handler for /peer-lm/1.0.0                               │
│  - On incoming prompt: run inference → write token stream back     │
│  - Discovery: rendezvous + mDNS (optional)                         │
│  - Optional: circuit relay for clients behind NAT                  │
└──────────────────────────────────────────────────────────────────┘
                                    │
                                    │ libp2p (encrypted)
                                    │
┌──────────────────────────────────────────────────────────────────┐
│                        LLM CLIENT (lightweight)                    │
│  - libp2p host (client)                                           │
│  - Connects to server (via address or discovery)                   │
│  - new_stream() → write prompt → read tokens                       │
└──────────────────────────────────────────────────────────────────┘
```

---

## Dependencies and LLM Backend

You'll need a Python LLM backend, for example:

- **llama-cpp-python** — GGUF models (like Jeff Geerling's setup)
- **vLLM** — for hosted APIs
- **transformers + torch** — for local inference
- **OpenAI-compatible** — to standardize the protocol layer
