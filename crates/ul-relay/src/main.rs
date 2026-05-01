use libp2p::futures::StreamExt;
use libp2p::{Multiaddr, Swarm, SwarmBuilder, identity, relay, swarm::SwarmEvent}; // for .next()

#[tokio::main]
async fn main() {
    // Identity & peer id
    let kp = identity::Keypair::generate_ed25519();
    let peer_id = kp.public().to_peer_id();

    // Build a Swarm using the phased builder (Tokio + QUIC transport)
    let mut swarm: Swarm<relay::Behaviour> = SwarmBuilder::with_existing_identity(kp)
        .with_tokio()
        .with_quic() // default QUIC config
        .with_behaviour(|keypair| {
            let pid = keypair.public().to_peer_id();
            Ok(relay::Behaviour::new(pid, relay::Config::default()))
        })
        .expect("relay builder")
        .build();

    // Listen on UDP/7000 on all interfaces
    let addr: Multiaddr = "/ip4/0.0.0.0/udp/7000/quic-v1"
        .parse()
        .expect("invalid multiaddr");
    swarm.listen_on(addr.clone()).expect("listen_on failed");

    println!("relay peerId: {peer_id}");
    println!("listening on: {addr}");

    while let Some(event) = swarm.next().await {
        if let SwarmEvent::NewListenAddr { address, .. } = event {
            println!("addr: {address}");
        }
    }
}
