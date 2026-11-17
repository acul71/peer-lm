use anyhow::Result;
use futures::StreamExt;
use libp2p::{
    identity::Keypair, noise, relay,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux, Multiaddr,
};
use std::time::Duration;

#[derive(NetworkBehaviour)]
struct RelayBehaviour {
    relay: relay::Behaviour,
    identify: libp2p::identify::Behaviour,
}

/// Run a Circuit Relay v2 server until cancelled.
pub async fn run_relay_server(keypair: Keypair, listen: Multiaddr) -> Result<()> {
    let local_peer_id = keypair.public().to_peer_id();
    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(keypair)
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_quic()
        .with_behaviour(|key| RelayBehaviour {
            // Defaults are too tight for agent prompts (128 KiB / 2 min).
            relay: relay::Behaviour::new(
                key.public().to_peer_id(),
                relay::Config {
                    max_circuit_duration: Duration::from_secs(30 * 60),
                    max_circuit_bytes: 64 << 20, // 64 MiB
                    ..Default::default()
                },
            ),
            identify: libp2p::identify::Behaviour::new(libp2p::identify::Config::new(
                "/peer-lm-relay/1.0.0".into(),
                key.public(),
            )),
        })?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
        .build();

    swarm.listen_on(listen)?;
    tracing::info!(%local_peer_id, "relay server started");

    loop {
        match swarm.select_next_some().await {
            SwarmEvent::NewListenAddr { address, .. } => {
                let full = address
                    .clone()
                    .with(libp2p::multiaddr::Protocol::P2p(local_peer_id));
                tracing::info!(%full, "relay listening (share this multiaddr)");
            }
            SwarmEvent::Behaviour(RelayBehaviourEvent::Identify(
                libp2p::identify::Event::Received { info, .. },
            )) => {
                swarm.add_external_address(info.observed_addr);
            }
            SwarmEvent::Behaviour(ev) => {
                tracing::debug!(?ev, "relay behaviour");
            }
            other => tracing::trace!(?other, "relay swarm"),
        }
    }
}
