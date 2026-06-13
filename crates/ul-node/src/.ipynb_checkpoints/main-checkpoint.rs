use anyhow::Result;
use clap::Parser;
use libp2p::Multiaddr;
use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};
use tracing::{info, warn};
use ul_p2p::{topics, KnownPeer, NetworkMessage};

#[derive(Debug, Parser)]
#[command(
    name = "ul-node",
    version,
    about = "Tally Free network protocol test node"
)]
struct Cli {
    /// Human-readable name for this test node.
    #[arg(long, default_value = "node")]
    name: String,

    /// Listen address. May be passed multiple times.
    ///
    /// Example:
    /// --listen /ip4/0.0.0.0/udp/7001/quic-v1
    #[arg(long = "listen")]
    listen: Vec<String>,

    /// Peer/relay address to dial. May be passed multiple times.
    ///
    /// Example:
    /// --dial /ip4/127.0.0.1/udp/7000/quic-v1/p2p/<RELAY_PEER_ID>
    #[arg(long = "dial")]
    dial: Vec<String>,

    /// Send a fake block proposal after startup.
    #[arg(long)]
    propose_fake_block: bool,
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

    let listen: Vec<Multiaddr> = cli
        .listen
        .iter()
        .map(|s| s.parse())
        .collect::<Result<Vec<_>, _>>()?;

    let dial: Vec<Multiaddr> = cli
        .dial
        .iter()
        .map(|s| s.parse())
        .collect::<Result<Vec<_>, _>>()?;

    let mut net = ul_p2p::start_named(cli.name.clone(), listen, dial).await?;

    println!("node name: {}", cli.name);
    println!("peer id: {}", net.local_peer_id_string());

    let mut known_peers: HashMap<String, KnownPeer> = HashMap::new();
    let mut seen_messages: HashSet<[u8; 32]> = HashSet::new();

    let mut hello_timer = tokio::time::interval(Duration::from_secs(5));
    let mut peer_list_timer = tokio::time::interval(Duration::from_secs(9));
    let mut ping_timer = tokio::time::interval(Duration::from_secs(7));
    let mut fake_block_timer = tokio::time::interval(Duration::from_secs(15));

    loop {
        tokio::select! {
            _ = hello_timer.tick() => {
                let listen_addrs = net.known_listeners();

                net.publish_message(
                    topics::TOPIC_PEERS,
                    NetworkMessage::PeerHello {
                        listen_addrs,
                    },
                )?;

                info!("sent PeerHello");
            }

            _ = peer_list_timer.tick() => {
                net.publish_message(
                    topics::TOPIC_PEERS,
                    NetworkMessage::PeerListRequest,
                )?;

                info!("sent PeerListRequest");
            }

            _ = ping_timer.tick() => {
                let nonce = now_nonce();

                net.publish_message(
                    topics::TOPIC_HEALTH,
                    NetworkMessage::Ping { nonce },
                )?;

                info!("sent Ping nonce={nonce}");
            }

            _ = fake_block_timer.tick(), if cli.propose_fake_block => {
                let fake_bytes = format!("fake block from {} at {}", cli.name, now_nonce()).into_bytes();
                let block_hash_hex = blake3::hash(&fake_bytes).to_hex().to_string();

                net.publish_message(
                    topics::TOPIC_PROPOSAL,
                    NetworkMessage::FakeBlockProposal {
                        height: 1,
                        block_hash_hex: block_hash_hex.clone(),
                        block_bytes: fake_bytes,
                    },
                )?;

                info!("sent FakeBlockProposal hash={block_hash_hex}");
            }

            msg = net.next_event() => {
                let msg = msg?;

                if !seen_messages.insert(msg.envelope.msg_id) {
                    continue;
                }

                if msg.envelope.from_peer == net.local_peer_id_string() {
                    continue;
                }

                match msg.envelope.payload {
                    NetworkMessage::Ping { nonce } => {
                        println!(
                            "← Ping nonce={} from {} ({})",
                            nonce,
                            msg.envelope.node_name,
                            msg.envelope.from_peer
                        );

                        net.publish_message(
                            topics::TOPIC_HEALTH,
                            NetworkMessage::Pong { nonce },
                        )?;
                    }

                    NetworkMessage::Pong { nonce } => {
                        println!(
                            "← Pong nonce={} from {} ({})",
                            nonce,
                            msg.envelope.node_name,
                            msg.envelope.from_peer
                        );
                    }

                    NetworkMessage::PeerHello { listen_addrs } => {
                        let peer = KnownPeer {
                            peer_id: msg.envelope.from_peer.clone(),
                            node_name: msg.envelope.node_name.clone(),
                            listen_addrs,
                            last_seen_ms: msg.envelope.timestamp_ms,
                        };

                        println!(
                            "← PeerHello from {} ({}) addrs={:?}",
                            peer.node_name,
                            peer.peer_id,
                            peer.listen_addrs
                        );

                        known_peers.insert(peer.peer_id.clone(), peer);

                        let peers = known_peers.values().cloned().collect::<Vec<_>>();

                        net.publish_message(
                            topics::TOPIC_PEERS,
                            NetworkMessage::PeerListResponse { peers },
                        )?;
                    }

                    NetworkMessage::PeerListRequest => {
                        println!(
                            "← PeerListRequest from {} ({})",
                            msg.envelope.node_name,
                            msg.envelope.from_peer
                        );

                        let peers = known_peers.values().cloned().collect::<Vec<_>>();

                        net.publish_message(
                            topics::TOPIC_PEERS,
                            NetworkMessage::PeerListResponse { peers },
                        )?;
                    }

                    NetworkMessage::PeerListResponse { peers } => {
                        println!("← PeerListResponse count={}", peers.len());

                        for peer in peers {
                            if peer.peer_id != net.local_peer_id_string() {
                                known_peers.insert(peer.peer_id.clone(), peer);
                            }
                        }
                    }

                    NetworkMessage::FakeBlockProposal {
                        height,
                        block_hash_hex,
                        block_bytes,
                    } => {
                        println!(
                            "← FakeBlockProposal h={} hash={} bytes={} from {}",
                            height,
                            block_hash_hex,
                            block_bytes.len(),
                            msg.envelope.node_name
                        );

                        net.publish_message(
                            topics::TOPIC_VOTE,
                            NetworkMessage::FakeVote {
                                height,
                                block_hash_hex,
                                approve: true,
                            },
                        )?;
                    }

                    NetworkMessage::FakeVote {
                        height,
                        block_hash_hex,
                        approve,
                    } => {
                        println!(
                            "← FakeVote h={} hash={} approve={} from {}",
                            height,
                            block_hash_hex,
                            approve,
                            msg.envelope.node_name
                        );
                    }

                    NetworkMessage::RawBytes { bytes } => {
                        println!(
                            "← RawBytes len={} from {}",
                            bytes.len(),
                            msg.envelope.node_name
                        );
                    }

                    other => {
                        warn!("received non-test message on topic {}: {:?}", msg.topic, other);
                    }
                }
            }
        }
    }
}

fn now_nonce() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}