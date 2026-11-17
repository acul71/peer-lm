use crate::connect::{AutoConnectTimers, ConnectMode, ResolvedConnect};
use crate::protocol::{CapsInfo, InferResponse, PeerLmRequest, PeerLmResponse, CAPS_PROTOCOL};
use anyhow::{anyhow, Context, Result};
use futures::StreamExt;
use libp2p::{
    dcutr,
    identity::Keypair,
    mdns,
    multiaddr::Protocol,
    noise, relay, rendezvous,
    request_response::{self, ProtocolSupport, ResponseChannel},
    swarm::{behaviour::toggle::Toggle, NetworkBehaviour, SwarmEvent},
    tcp, yamux, Multiaddr, PeerId, StreamProtocol, Swarm,
};
use peer_lm_backend::BackendClient;
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio::time::sleep;

#[derive(Debug, Clone)]
pub struct NodeConfig {
    pub listen: Multiaddr,
    pub enable_mdns: bool,
    pub enable_dcutr: bool,
    pub allowlist: HashSet<PeerId>,
    pub local_caps: CapsInfo,
    pub keypair: Keypair,
    /// Relay multiaddrs ending in `/p2p/<RELAY_PEER_ID>`.
    pub relay_addrs: Vec<Multiaddr>,
    /// Listen for incoming connections via relay (`/p2p-circuit`).
    pub reserve_relay: bool,
    /// Prefer dialing peers through a configured relay.
    pub dial_via_relay: bool,
    /// Optional rendezvous point multiaddr ending in `/p2p/<ID>`.
    pub rendezvous_addr: Option<Multiaddr>,
    pub mesh_namespace: String,
    /// Peers to dial at startup (via relay when `dial_via_relay` is set).
    pub bootstrap_peers: Vec<PeerId>,
    /// Connectivity ladder (`auto` = mDNS → hole-punch/direct → relay).
    pub connect_mode: ConnectMode,
    pub auto_timers: AutoConnectTimers,
}

#[derive(Debug, Clone)]
pub struct PeerSnapshot {
    pub peer_id: PeerId,
    pub addresses: Vec<Multiaddr>,
    pub caps: Option<CapsInfo>,
}

#[derive(Clone)]
pub struct PeerLmNode {
    cmd_tx: mpsc::Sender<Command>,
    local_peer_id: PeerId,
}

enum Command {
    Infer {
        peer: PeerId,
        body: Vec<u8>,
        stream: bool,
        reply: oneshot::Sender<Result<InferResponse>>,
    },
    RefreshCaps {
        peer: PeerId,
        reply: oneshot::Sender<Result<CapsInfo>>,
    },
    ListPeers {
        reply: oneshot::Sender<Vec<PeerSnapshot>>,
    },
}

struct PendingConnect {
    peer: PeerId,
    body: Vec<u8>,
    stream: bool,
    reply: oneshot::Sender<Result<InferResponse>>,
}

#[derive(NetworkBehaviour)]
struct Behaviour {
    identify: libp2p::identify::Behaviour,
    mdns: Toggle<mdns::tokio::Behaviour>,
    relay_client: relay::client::Behaviour,
    dcutr: Toggle<dcutr::Behaviour>,
    rendezvous: Toggle<rendezvous::client::Behaviour>,
    request_response: request_response::cbor::Behaviour<PeerLmRequest, PeerLmResponse>,
}

impl PeerLmNode {
    pub async fn start(
        config: NodeConfig,
        backend: Option<BackendClient>,
    ) -> Result<(Self, tokio::task::JoinHandle<()>)> {
        let local_peer_id = config.keypair.public().to_peer_id();
        let allowlist = config.allowlist.clone();
        let local_caps = config.local_caps.clone();
        let relay_addrs = config.relay_addrs.clone();
        let resolved = ResolvedConnect::resolve(
            config.connect_mode,
            config.enable_mdns,
            config.enable_dcutr,
            config.reserve_relay,
            config.dial_via_relay,
            !relay_addrs.is_empty(),
            config.auto_timers,
        );
        let enable_mdns = resolved.enable_mdns;
        let enable_dcutr = resolved.enable_dcutr;
        let reserve_relay = resolved.reserve_relay;
        let mut dial_via_relay = resolved.dial_via_relay;
        let connect_mode = resolved.mode;
        let auto_timers = resolved.timers;
        tracing::info!(
            %connect_mode,
            enable_mdns,
            enable_dcutr,
            reserve_relay,
            dial_via_relay,
            mdns_timeout_secs = auto_timers.mdns_timeout.as_secs(),
            "connect strategy resolved"
        );
        let rendezvous_addr = config.rendezvous_addr.clone();
        let mesh_namespace = config.mesh_namespace.clone();
        let enable_rendezvous = rendezvous_addr.is_some();
        let bootstrap_peers = config.bootstrap_peers.clone();

        let mut swarm = libp2p::SwarmBuilder::with_existing_identity(config.keypair.clone())
            .with_tokio()
            .with_tcp(
                tcp::Config::default().nodelay(true),
                noise::Config::new,
                yamux::Config::default,
            )?
            .with_quic()
            .with_dns()?
            .with_relay_client(noise::Config::new, yamux::Config::default)?
            .with_behaviour(|key, relay_client| {
                let identify = libp2p::identify::Behaviour::new(libp2p::identify::Config::new(
                    "/peer-lm/1.0.0".into(),
                    key.public(),
                ));
                let mdns = if enable_mdns {
                    Toggle::from(Some(mdns::tokio::Behaviour::new(
                        mdns::Config::default(),
                        key.public().to_peer_id(),
                    )?))
                } else {
                    Toggle::from(None)
                };
                let dcutr = if enable_dcutr {
                    Toggle::from(Some(dcutr::Behaviour::new(key.public().to_peer_id())))
                } else {
                    Toggle::from(None)
                };
                let rendezvous = if enable_rendezvous {
                    Toggle::from(Some(rendezvous::client::Behaviour::new(key.clone())))
                } else {
                    Toggle::from(None)
                };
                let request_response = request_response::cbor::Behaviour::new(
                    [(StreamProtocol::new(CAPS_PROTOCOL), ProtocolSupport::Full)],
                    // Infer can take minutes against a real engine; default is 10s.
                    request_response::Config::default()
                        .with_request_timeout(Duration::from_secs(300)),
                );
                Ok(Behaviour {
                    identify,
                    mdns,
                    relay_client,
                    dcutr,
                    rendezvous,
                    request_response,
                })
            })?
            .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
            .build();

        swarm.listen_on(config.listen.clone())?;

        for addr in &relay_addrs {
            tracing::info!(%addr, "dialing relay");
            swarm.dial(addr.clone())?;
        }

        if let Some(rzv) = &rendezvous_addr {
            // External address placeholder so rendezvous has something to advertise locally.
            swarm.add_external_address("/ip4/127.0.0.1/tcp/0".parse()?);
            swarm.dial(rzv.clone())?;
        }

        let peers = std::sync::Arc::new(tokio::sync::RwLock::new(HashMap::<
            PeerId,
            PeerSnapshot,
        >::new()));
        for peer in &bootstrap_peers {
            peers.write().await.insert(
                *peer,
                PeerSnapshot {
                    peer_id: *peer,
                    addresses: vec![],
                    caps: None,
                },
            );
            // In auto mode, prefer waiting for mDNS before forcing a circuit dial.
            let use_relay_now = dial_via_relay && connect_mode != ConnectMode::Auto;
            if connect_mode == ConnectMode::Auto {
                tracing::info!(%peer, "auto: waiting for mDNS before relay fallback");
                // Best-effort direct dial in case we already know an address.
                let _ = swarm.dial(*peer);
            } else if let Err(err) =
                ensure_connected(&mut swarm, *peer, &relay_addrs, use_relay_now)
            {
                tracing::warn!(%peer, error = %err, "bootstrap dial failed");
            } else {
                tracing::info!(%peer, "bootstrap dial queued");
            }
        }
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<Command>(64);

        let node = Self {
            cmd_tx,
            local_peer_id,
        };

        let handle = tokio::spawn(async move {
            let mut pending_infer: HashMap<
                request_response::OutboundRequestId,
                oneshot::Sender<Result<InferResponse>>,
            > = HashMap::new();
            let mut pending_caps: HashMap<
                request_response::OutboundRequestId,
                oneshot::Sender<Result<CapsInfo>>,
            > = HashMap::new();
            let mut pending_connect: Vec<PendingConnect> = Vec::new();
            let mut reserved_relays: HashSet<PeerId> = HashSet::new();
            let relay_peer_ids: HashSet<PeerId> = relay_addrs
                .iter()
                .filter_map(peer_id_from_multiaddr)
                .collect();
            let mut rendezvous_peer: Option<PeerId> = None;
            if let Some(addr) = &rendezvous_addr {
                rendezvous_peer = peer_id_from_multiaddr(addr);
            }
            // Peers we already escalated to relay in auto mode (avoid loops).
            let mut auto_relay_escalated: HashSet<PeerId> = HashSet::new();
            let mut auto_mdns_phase = connect_mode == ConnectMode::Auto;
            let mdns_deadline = sleep(auto_timers.mdns_timeout);
            tokio::pin!(mdns_deadline);

            loop {
                tokio::select! {
                    _ = &mut mdns_deadline, if auto_mdns_phase => {
                        auto_mdns_phase = false;
                        if relay_addrs.is_empty() {
                            tracing::warn!("auto: mDNS window elapsed but no relays configured");
                            continue;
                        }
                        tracing::info!(
                            timeout_secs = auto_timers.mdns_timeout.as_secs(),
                            "auto: mDNS window elapsed — dialing missing peers via relay (DCUtR may upgrade to direct)"
                        );
                        dial_via_relay = true;
                        for peer in &bootstrap_peers {
                            if swarm.is_connected(peer) {
                                tracing::info!(%peer, "auto: peer already connected (likely mDNS/direct)");
                                continue;
                            }
                            auto_relay_escalated.insert(*peer);
                            if let Err(err) = ensure_connected(&mut swarm, *peer, &relay_addrs, true) {
                                tracing::warn!(%peer, error = %err, "auto: relay fallback dial failed");
                            } else {
                                tracing::info!(%peer, "auto: relay fallback dial queued");
                            }
                        }
                    }
                    cmd = cmd_rx.recv() => {
                        let Some(cmd) = cmd else { break };
                        match cmd {
                            Command::ListPeers { reply } => {
                                let snap = peers.read().await.values().cloned().collect();
                                let _ = reply.send(snap);
                            }
                            Command::RefreshCaps { peer, reply } => {
                                if !allowlist_ok(&allowlist, &peer) {
                                    let _ = reply.send(Err(anyhow!("peer {peer} not allowlisted")));
                                    continue;
                                }
                                if swarm.is_connected(&peer) {
                                    let id = swarm.behaviour_mut().request_response.send_request(
                                        &peer,
                                        PeerLmRequest::Caps,
                                    );
                                    pending_caps.insert(id, reply);
                                } else if let Err(err) = ensure_connected(
                                    &mut swarm,
                                    peer,
                                    &relay_addrs,
                                    dial_via_relay,
                                ) {
                                    let _ = reply.send(Err(err));
                                } else {
                                    // Will retry caps after connection; drop for now with error if never connects
                                    let _ = reply.send(Err(anyhow!("dialing peer {peer}; retry caps shortly")));
                                }
                            }
                            Command::Infer { peer, body, stream, reply } => {
                                if !allowlist_ok(&allowlist, &peer) {
                                    let _ = reply.send(Err(anyhow!("peer {peer} not allowlisted")));
                                    continue;
                                }
                                if swarm.is_connected(&peer) {
                                    let id = swarm.behaviour_mut().request_response.send_request(
                                        &peer,
                                        PeerLmRequest::Infer { body, stream },
                                    );
                                    pending_infer.insert(id, reply);
                                } else {
                                    if let Err(err) = ensure_connected(
                                        &mut swarm,
                                        peer,
                                        &relay_addrs,
                                        dial_via_relay,
                                    ) {
                                        let _ = reply.send(Err(err));
                                        continue;
                                    }
                                    pending_connect.push(PendingConnect { peer, body, stream, reply });
                                }
                            }
                        }
                    }
                    event = swarm.select_next_some() => {
                        match event {
                            SwarmEvent::NewListenAddr { address, .. } => {
                                tracing::info!(%address, peer = %local_peer_id, "listening");
                            }
                            SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                                tracing::warn!(?peer_id, %error, "outgoing connection error");
                                // Auto ladder: direct/mDNS dial failed → try relay (+ DCUtR).
                                if connect_mode == ConnectMode::Auto {
                                    if let Some(peer) = peer_id {
                                        if !relay_peer_ids.contains(&peer)
                                            && !auto_relay_escalated.contains(&peer)
                                            && !relay_addrs.is_empty()
                                        {
                                            auto_relay_escalated.insert(peer);
                                            tracing::info!(%peer, "auto: escalating failed dial to relay");
                                            if let Err(err) =
                                                ensure_connected(&mut swarm, peer, &relay_addrs, true)
                                            {
                                                tracing::warn!(%peer, error = %err, "auto: relay escalate failed");
                                            }
                                        }
                                    }
                                }
                                let mut keep = Vec::new();
                                for p in pending_connect.drain(..) {
                                    if Some(p.peer) == peer_id {
                                        // Keep waiting if we just queued a relay escalate.
                                        if connect_mode == ConnectMode::Auto
                                            && auto_relay_escalated.contains(&p.peer)
                                        {
                                            keep.push(p);
                                        } else {
                                            let _ = p.reply.send(Err(anyhow!("dial failed: {error}")));
                                        }
                                    } else {
                                        keep.push(p);
                                    }
                                }
                                pending_connect = keep;
                            }
                            SwarmEvent::Behaviour(BehaviourEvent::Mdns(mdns::Event::Discovered(list))) => {
                                for (peer, addr) in list {
                                    if peer == local_peer_id { continue; }
                                    if !allowlist_ok(&allowlist, &peer) { continue; }
                                    tracing::info!(%peer, %addr, "mdns discovered");
                                    swarm.add_peer_address(peer, addr.clone());
                                    let _ = swarm.dial(peer);
                                    remember_peer(&peers, peer, Some(addr)).await;
                                    if connect_mode == ConnectMode::Auto {
                                        tracing::info!(%peer, "auto: using mDNS/direct path");
                                    }
                                }
                            }
                            SwarmEvent::Behaviour(BehaviourEvent::Mdns(mdns::Event::Expired(list))) => {
                                for (peer, addr) in list {
                                    if let Some(entry) = peers.write().await.get_mut(&peer) {
                                        entry.addresses.retain(|a| a != &addr);
                                    }
                                }
                            }
                            SwarmEvent::Behaviour(BehaviourEvent::RelayClient(ev)) => {
                                tracing::info!(?ev, "relay client");
                                if let relay::client::Event::ReservationReqAccepted { relay_peer_id, .. } = &ev {
                                    tracing::info!(%relay_peer_id, "relay reservation accepted");
                                }
                            }
                            SwarmEvent::Behaviour(BehaviourEvent::Dcutr(ev)) => {
                                tracing::info!(?ev, "dcutr (hole punch toward direct connection)");
                            }
                            SwarmEvent::Behaviour(BehaviourEvent::Rendezvous(ev)) => {
                                handle_rendezvous_client_event(
                                    &mut swarm,
                                    &peers,
                                    &allowlist,
                                    &mesh_namespace,
                                    rendezvous_peer,
                                    ev,
                                ).await;
                            }
                            SwarmEvent::Behaviour(BehaviourEvent::Identify(libp2p::identify::Event::Received {
                                peer_id,
                                info,
                                ..
                            })) => {
                                tracing::debug!(%peer_id, observed = %info.observed_addr, "identify received");
                                swarm.add_external_address(info.observed_addr.clone());

                                // Official relay flow: identify with the relay, then listen on /p2p-circuit.
                                if reserve_relay
                                    && relay_peer_ids.contains(&peer_id)
                                    && reserved_relays.insert(peer_id)
                                {
                                    if let Some(addr) = relay_addrs
                                        .iter()
                                        .find(|a| peer_id_from_multiaddr(a) == Some(peer_id))
                                    {
                                        let circuit = addr.clone().with(Protocol::P2pCircuit);
                                        match swarm.listen_on(circuit.clone()) {
                                            Ok(_) => tracing::info!(%circuit, "listening via relay circuit"),
                                            Err(err) => {
                                                tracing::warn!(%circuit, error = %err, "relay listen failed");
                                                reserved_relays.remove(&peer_id);
                                            }
                                        }
                                    }
                                }

                                if Some(peer_id) == rendezvous_peer {
                                    if let Some(rzv) = swarm.behaviour_mut().rendezvous.as_mut() {
                                        let ns = match rendezvous::Namespace::new(mesh_namespace.clone()) {
                                            Ok(ns) => ns,
                                            Err(err) => {
                                                tracing::error!(error = %err, "invalid mesh namespace");
                                                continue;
                                            }
                                        };
                                        if let Err(err) = rzv.register(ns, peer_id, None) {
                                            tracing::warn!(error = %err, "rendezvous register failed");
                                        } else {
                                            tracing::info!(%peer_id, namespace = %mesh_namespace, "rendezvous register sent");
                                        }
                                        rzv.discover(
                                            Some(rendezvous::Namespace::new(mesh_namespace.clone()).unwrap()),
                                            None,
                                            None,
                                            peer_id,
                                        );
                                    }
                                }
                            }
                            SwarmEvent::Behaviour(BehaviourEvent::RequestResponse(ev)) => {
                                match ev {
                                    request_response::Event::Message { peer, message, .. } => {
                                        match message {
                                            request_response::Message::Request { request, channel, .. } => {
                                                handle_inbound_request(
                                                    &mut swarm,
                                                    &backend,
                                                    &allowlist,
                                                    &local_caps,
                                                    peer,
                                                    request,
                                                    channel,
                                                ).await;
                                            }
                                            request_response::Message::Response { request_id, response } => {
                                                match response {
                                                    PeerLmResponse::Caps(info) => {
                                                        if let Some(entry) = peers.write().await.get_mut(&peer) {
                                                            entry.caps = Some(info.clone());
                                                        }
                                                        if let Some(tx) = pending_caps.remove(&request_id) {
                                                            let _ = tx.send(Ok(info));
                                                        }
                                                    }
                                                    PeerLmResponse::Infer(resp) => {
                                                        if let Some(tx) = pending_infer.remove(&request_id) {
                                                            let _ = tx.send(Ok(resp));
                                                        }
                                                    }
                                                    PeerLmResponse::Error(err) => {
                                                        if let Some(tx) = pending_infer.remove(&request_id) {
                                                            let _ = tx.send(Err(anyhow!(err.clone())));
                                                        }
                                                        if let Some(tx) = pending_caps.remove(&request_id) {
                                                            let _ = tx.send(Err(anyhow!(err)));
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    request_response::Event::OutboundFailure { request_id, error, .. } => {
                                        if let Some(tx) = pending_infer.remove(&request_id) {
                                            let _ = tx.send(Err(anyhow!("infer outbound failure: {error}")));
                                        }
                                        if let Some(tx) = pending_caps.remove(&request_id) {
                                            let _ = tx.send(Err(anyhow!("caps outbound failure: {error}")));
                                        }
                                    }
                                    request_response::Event::InboundFailure { error, .. } => {
                                        tracing::warn!(%error, "inbound request failure");
                                    }
                                    request_response::Event::ResponseSent { .. } => {}
                                }
                            }
                            SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => {
                                tracing::info!(%peer_id, ?endpoint, "connection established");

                                // Bootstrap dials can race the relay TCP connect; retry once the
                                // relay is up so /p2p-circuit dials actually land.
                                if dial_via_relay && relay_peer_ids.contains(&peer_id) {
                                    for peer in &bootstrap_peers {
                                        if swarm.is_connected(peer) {
                                            continue;
                                        }
                                        if connect_mode == ConnectMode::Auto && auto_mdns_phase {
                                            // Still in LAN window — don't force circuit yet.
                                            continue;
                                        }
                                        if let Err(err) = ensure_connected(
                                            &mut swarm,
                                            *peer,
                                            &relay_addrs,
                                            true,
                                        ) {
                                            tracing::warn!(%peer, error = %err, "relay-ready bootstrap redial failed");
                                        } else {
                                            tracing::info!(%peer, "relay-ready bootstrap redial queued");
                                        }
                                    }
                                }

                                if allowlist_ok(&allowlist, &peer_id)
                                    && Some(peer_id) != rendezvous_peer
                                    && !relay_peer_ids.contains(&peer_id)
                                {
                                    remember_peer(&peers, peer_id, None).await;
                                    let _ = swarm.behaviour_mut().request_response.send_request(
                                        &peer_id,
                                        PeerLmRequest::Caps,
                                    );
                                }

                                // Flush infer requests waiting for this peer.
                                let mut keep = Vec::new();
                                for p in pending_connect.drain(..) {
                                    if p.peer == peer_id {
                                        let id = swarm.behaviour_mut().request_response.send_request(
                                            &p.peer,
                                            PeerLmRequest::Infer {
                                                body: p.body,
                                                stream: p.stream,
                                            },
                                        );
                                        pending_infer.insert(id, p.reply);
                                    } else {
                                        keep.push(p);
                                    }
                                }
                                pending_connect = keep;
                            }
                            SwarmEvent::ConnectionClosed { peer_id, .. } => {
                                tracing::debug!(%peer_id, "connection closed");
                            }
                            _ => {}
                        }
                    }
                }
            }
        });

        Ok((node, handle))
    }

    pub fn local_peer_id(&self) -> PeerId {
        self.local_peer_id
    }

    pub async fn list_peers(&self) -> Result<Vec<PeerSnapshot>> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(Command::ListPeers { reply: tx })
            .await
            .context("p2p command channel closed")?;
        Ok(rx.await.context("list_peers response dropped")?)
    }

    pub async fn refresh_caps(&self, peer: PeerId) -> Result<CapsInfo> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(Command::RefreshCaps { peer, reply: tx })
            .await
            .context("p2p command channel closed")?;
        rx.await.context("refresh_caps response dropped")?
    }

    pub async fn infer(&self, peer: PeerId, body: Vec<u8>, stream: bool) -> Result<InferResponse> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(Command::Infer {
                peer,
                body,
                stream,
                reply: tx,
            })
            .await
            .context("p2p command channel closed")?;
        rx.await.context("infer response dropped")?
    }

    pub async fn pick_peer_for_model(&self, model: Option<&str>) -> Option<PeerId> {
        let peers = self.list_peers().await.ok()?;
        peers
            .into_iter()
            .filter_map(|p| {
                let caps = p.caps.as_ref()?;
                if caps.busy || caps.slots_free == 0 {
                    return None;
                }
                if let Some(m) = model {
                    if !caps.models.iter().any(|x| x == m) {
                        return None;
                    }
                }
                Some((p.peer_id, caps.slots_free))
            })
            .max_by_key(|(_, slots)| *slots)
            .map(|(id, _)| id)
    }
}

fn allowlist_ok(allowlist: &HashSet<PeerId>, peer: &PeerId) -> bool {
    allowlist.is_empty() || allowlist.contains(peer)
}

fn peer_id_from_multiaddr(addr: &Multiaddr) -> Option<PeerId> {
    addr.iter().find_map(|p| match p {
        Protocol::P2p(id) => Some(id),
        _ => None,
    })
}

fn ensure_connected(
    swarm: &mut Swarm<Behaviour>,
    peer: PeerId,
    relay_addrs: &[Multiaddr],
    dial_via_relay: bool,
) -> Result<()> {
    if swarm.is_connected(&peer) {
        return Ok(());
    }
    if dial_via_relay {
        let Some(relay) = relay_addrs.first() else {
            return Err(anyhow!("dial_via_relay set but no relay_addrs configured"));
        };
        let circuit = relay
            .clone()
            .with(Protocol::P2pCircuit)
            .with(Protocol::P2p(peer));
        tracing::info!(%circuit, "dialing peer via relay");
        swarm.dial(circuit)?;
    } else {
        swarm.dial(peer)?;
    }
    Ok(())
}

async fn remember_peer(
    peers: &tokio::sync::RwLock<HashMap<PeerId, PeerSnapshot>>,
    peer: PeerId,
    addr: Option<Multiaddr>,
) {
    let mut guard = peers.write().await;
    let entry = guard.entry(peer).or_insert_with(|| PeerSnapshot {
        peer_id: peer,
        addresses: vec![],
        caps: None,
    });
    if let Some(addr) = addr {
        if !entry.addresses.contains(&addr) {
            entry.addresses.push(addr);
        }
    }
}

async fn handle_rendezvous_client_event(
    swarm: &mut Swarm<Behaviour>,
    peers: &tokio::sync::RwLock<HashMap<PeerId, PeerSnapshot>>,
    allowlist: &HashSet<PeerId>,
    mesh_namespace: &str,
    rendezvous_peer: Option<PeerId>,
    ev: rendezvous::client::Event,
) {
    match ev {
        rendezvous::client::Event::Registered { namespace, ttl, .. } => {
            tracing::info!(%namespace, ttl, "registered at rendezvous");
            if let (Some(rzv_peer), Some(rzv)) =
                (rendezvous_peer, swarm.behaviour_mut().rendezvous.as_mut())
            {
                let _ = rzv.discover(
                    Some(rendezvous::Namespace::new(mesh_namespace.to_string()).unwrap()),
                    None,
                    None,
                    rzv_peer,
                );
            }
        }
        rendezvous::client::Event::Discovered { registrations, .. } => {
            for reg in registrations {
                let peer = reg.record.peer_id();
                if !allowlist_ok(allowlist, &peer) {
                    continue;
                }
                for addr in reg.record.addresses() {
                    swarm.add_peer_address(peer, addr.clone());
                    remember_peer(peers, peer, Some(addr.clone())).await;
                }
                let _ = swarm.dial(peer);
                tracing::info!(%peer, "discovered via rendezvous");
            }
        }
        other => tracing::debug!(?other, "rendezvous client"),
    }
}

async fn handle_inbound_request(
    swarm: &mut Swarm<Behaviour>,
    backend: &Option<BackendClient>,
    allowlist: &HashSet<PeerId>,
    local_caps: &CapsInfo,
    peer: PeerId,
    request: PeerLmRequest,
    channel: ResponseChannel<PeerLmResponse>,
) {
    if !allowlist_ok(allowlist, &peer) {
        let _ = swarm
            .behaviour_mut()
            .request_response
            .send_response(channel, PeerLmResponse::Error("not allowlisted".into()));
        return;
    }

    let response = match request {
        PeerLmRequest::Caps => PeerLmResponse::Caps(local_caps.clone()),
        PeerLmRequest::Infer { body, stream } => match backend {
            Some(backend) => match serve_infer(backend, body, stream).await {
                Ok(resp) => PeerLmResponse::Infer(resp),
                Err(err) => PeerLmResponse::Error(err.to_string()),
            },
            None => PeerLmResponse::Error("this node has no local backend".into()),
        },
    };

    if swarm
        .behaviour_mut()
        .request_response
        .send_response(channel, response)
        .is_err()
    {
        tracing::warn!(%peer, "failed to send response");
    }
}

async fn serve_infer(backend: &BackendClient, body: Vec<u8>, stream: bool) -> Result<InferResponse> {
    use bytes::Bytes;
    let forwarded = backend
        .forward_chat_completions(Bytes::from(body), stream)
        .await?;
    let content_type = forwarded
        .headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or(if stream {
            "text/event-stream"
        } else {
            "application/json"
        })
        .to_string();
    Ok(InferResponse {
        status: forwarded.status,
        content_type,
        body: forwarded.body.to_vec(),
    })
}
