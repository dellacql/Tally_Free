use anyhow::Result;
use clap::Parser;
use libp2p::{
    Multiaddr, Swarm, SwarmBuilder,
    futures::StreamExt,
    gossipsub,
    gossipsub::{IdentTopic, MessageAuthenticity},
    identify, identity, relay,
    swarm::{NetworkBehaviour, SwarmEvent},
};
use std::time::Duration;
use tracing::{info, warn};
use ul_p2p::topics;

#[derive(Debug, Parser)]
#[command(
    name = "ul-relay",
    version,
    about = "Tally Free multi-IP relay and gossip bridge"
)]
struct Cli {
    /// Listen address. May be passed multiple times.
    ///
    /// Examples:
    /// --listen /ip4/0.0.0.0/udp/7000/quic-v1
    /// --listen /ip4/127.0.0.1/udp/7000/quic-v1
    #[arg(long = "listen")]
    listen: Vec<String>,

    /// Optional external/public address to advertise. May be passed multiple times.
    ///
    /// Example:
    /// --external /ip4/YOUR_PUBLIC_IP/udp/7000/quic-v1
    #[arg(long = "external")]
    external: Vec<String>,
}

#[derive(NetworkBehaviour)]
struct Behaviour {
    relay: relay::Behaviour,
    identify: identify::Behaviour,
    gossipsub: gossipsub::Behaviour,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "info,libp2p=warn,libp2p_swarm=warn".to_string()),
        )
        .init();

    let cli = Cli::parse();

    let listen_addrs: Vec<Multiaddr> = if cli.listen.is_empty() {
        vec!["/ip4/0.0.0.0/udp/7000/quic-v1".parse()?]
    } else {
        cli.listen
            .iter()
            .map(|s| s.parse())
            .collect::<Result<Vec<_>, _>>()?
    };

    let external_addrs: Vec<Multiaddr> = cli
        .external
        .iter()
        .map(|s| s.parse())
        .collect::<Result<Vec<_>, _>>()?;

    let keypair = identity::Keypair::generate_ed25519();
    let peer_id = keypair.public().to_peer_id();

    let mut gossipsub = {
        let cfg = gossipsub::ConfigBuilder::default()
            .validation_mode(gossipsub::ValidationMode::Strict)
            .heartbeat_interval(Duration::from_secs(2))
            .build()
            .expect("valid gossipsub config");

        gossipsub::Behaviour::new(MessageAuthenticity::Signed(keypair.clone()), cfg)
            .expect("valid gossipsub behaviour")
    };

    for topic_name in topics::all_topics() {
        let topic = IdentTopic::new(topic_name);
        gossipsub.subscribe(&topic)?;
    }

    let mut swarm: Swarm<Behaviour> = SwarmBuilder::with_existing_identity(keypair)
        .with_tokio()
        .with_quic()
        .with_behaviour(|keypair| {
            let local_peer = keypair.public().to_peer_id();

            Ok(Behaviour {
                relay: relay::Behaviour::new(local_peer, relay::Config::default()),
                identify: identify::Behaviour::new(identify::Config::new(
                    "/tally-free-relay/1.0.0".to_string(),
                    keypair.public(),
                )),
                gossipsub,
            })
        })?
        .build();

    for addr in listen_addrs {
        swarm.listen_on(addr)?;
    }

    for addr in external_addrs {
        info!("advertising external address: {addr}");
        swarm.add_external_address(addr);
    }

    println!("relay peerId: {peer_id}");
    println!("shareable relay address format:");
    println!("  /ip4/<RELAY_IP>/udp/7000/quic-v1/p2p/{peer_id}");

    while let Some(event) = swarm.next().await {
        match event {
            SwarmEvent::NewListenAddr { address, .. } => {
                info!("listening on: {address}");
                println!("listening on: {address}");
            }
            SwarmEvent::ConnectionEstablished {
                peer_id, endpoint, ..
            } => {
                info!("connection established peer={peer_id} endpoint={endpoint:?}");
            }
            SwarmEvent::ConnectionClosed { peer_id, cause, .. } => {
                warn!("connection closed peer={peer_id} cause={cause:?}");
            }
            SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                warn!("outgoing connection error peer={peer_id:?} error={error}");
            }
            SwarmEvent::IncomingConnectionError { error, .. } => {
                warn!("incoming connection error error={error}");
            }
            SwarmEvent::Behaviour(BehaviourEvent::Gossipsub(gossipsub::Event::Message {
                message,
                propagation_source,
                ..
            })) => {
                info!(
                    "gossip bridge saw message topic={} bytes={} from={}",
                    message.topic,
                    message.data.len(),
                    propagation_source
                );
            }
            other => {
                info!("relay event: {other:?}");
            }
        }
    }

    Ok(())
}
