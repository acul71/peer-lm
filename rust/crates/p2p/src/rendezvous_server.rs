use anyhow::Result;
use futures::StreamExt;
use libp2p::{
    identity::Keypair, noise, rendezvous,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux, Multiaddr,
};
use std::time::Duration;

#[derive(NetworkBehaviour)]
struct RendezvousBehaviour {
    identify: libp2p::identify::Behaviour,
    rendezvous: rendezvous::server::Behaviour,
}

/// Run a rendezvous point until cancelled.
pub async fn run_rendezvous_server(keypair: Keypair, listen: Multiaddr) -> Result<()> {
    let local_peer_id = keypair.public().to_peer_id();
    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(keypair)
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_behaviour(|key| RendezvousBehaviour {
            identify: libp2p::identify::Behaviour::new(libp2p::identify::Config::new(
                "/peer-lm-rendezvous/1.0.0".into(),
                key.public(),
            )),
            rendezvous: rendezvous::server::Behaviour::new(rendezvous::server::Config::default()),
        })?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
        .build();

    swarm.listen_on(listen)?;
    tracing::info!(%local_peer_id, "rendezvous server started");

    loop {
        match swarm.select_next_some().await {
            SwarmEvent::NewListenAddr { address, .. } => {
                let full = address
                    .clone()
                    .with(libp2p::multiaddr::Protocol::P2p(local_peer_id));
                tracing::info!(%full, "rendezvous listening (share this multiaddr)");
            }
            SwarmEvent::Behaviour(RendezvousBehaviourEvent::Rendezvous(
                rendezvous::server::Event::PeerRegistered { peer, registration },
            )) => {
                tracing::info!(
                    %peer,
                    namespace = %registration.namespace,
                    "peer registered"
                );
            }
            SwarmEvent::Behaviour(RendezvousBehaviourEvent::Rendezvous(
                rendezvous::server::Event::DiscoverServed {
                    enquirer,
                    registrations,
                },
            )) => {
                tracing::info!(
                    %enquirer,
                    count = registrations.len(),
                    "served discovery"
                );
            }
            other => tracing::trace!(?other, "rendezvous swarm"),
        }
    }
}
