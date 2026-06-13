use anyhow::{anyhow, Result};
use libp2p::{
    autonat, dcutr,
    futures::StreamExt,
    gossipsub,
    gossipsub::{IdentTopic, MessageAuthenticity},
    identify, identity, kad,
    multiaddr::Protocol,
    swarm::{NetworkBehaviour, SwarmEvent},
    Multiaddr, PeerId, Swarm, SwarmBuilder,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub mod topics;

// Backward compatibility for the existing ul-node/src/main.rs.
// Your old node imports ul_p2p::Wire.
pub use ul_types::Wire;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Envelope {
    pub protocol: String,
    pub version: String,
    pub from_peer: String,
    pub node_name: String,
    pub msg_id: [u8; 32],
    pub timestamp_ms: u128,
    pub payload: NetworkMessage,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum NetworkMessage {
    // Compatibility wrapper for the old node.
    // Old ul-node publishes raw bincode-serialized Wire bytes.
    RawBytes {
        bytes: Vec<u8>,
    },

    Ping {
        nonce: u64,
    },
    Pong {
        nonce: u64,
    },
    PeerHello {
        listen_addrs: Vec<String>,
    },
    PeerListRequest,
    PeerListResponse {
        peers: Vec<KnownPeer>,
    },
    FakeBlockProposal {
        height: u64,
        block_hash_hex: String,
        block_bytes: Vec<u8>,
    },
    FakeVote {
        height: u64,
        block_hash_hex: String,
        approve: bool,
    },

    // Future ledger/consensus protocol messages.
    TxCreateAccount {
        addr: [u8; 32],
        vk: Vec<u8>,
        sig: Vec<u8>,
    },
    TxTransfer {
        from: [u8; 32],
        to: [u8; 32],
        amount_units_be: Vec<u8>,
        vk: Vec<u8>,
        sig: Vec<u8>,
    },
    Proposal {
        height: u64,
        parent: [u8; 32],
        block_bytes: Vec<u8>,
        block_hash: [u8; 32],
        proposer_account: [u8; 32],
        proposer_vk: Vec<u8>,
        proposer_sig: Vec<u8>,
    },
    Vote {
        height: u64,
        block_hash: [u8; 32],
        accept: bool,
        voter_account: [u8; 32],
        voter_stake_be: Vec<u8>,
        voter_vk: Vec<u8>,
        voter_sig: Vec<u8>,
    },
    Commit {
        height: u64,
        block_hash: [u8; 32],
        committer_account: [u8; 32],
        committer_vk: Vec<u8>,
        committer_sig: Vec<u8>,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct KnownPeer {
    pub peer_id: String,
    pub node_name: String,
    pub listen_addrs: Vec<String>,
    pub last_seen_ms: u128,
}

#[derive(Debug, Clone)]
pub struct ReceivedMessage {
    pub topic: String,
    pub envelope: Envelope,
}

#[derive(NetworkBehaviour)]
struct Behaviour {
    gossipsub: gossipsub::Behaviour,
    identify: identify::Behaviour,
    kademlia: kad::Behaviour<kad::store::MemoryStore>,
    autonat: autonat::Behaviour,
    dcutr: dcutr::Behaviour,
}

pub struct Net {
    swarm: Swarm<Behaviour>,
    node_name: String,
    topics: HashMap<String, IdentTopic>,
    inbox: VecDeque<ReceivedMessage>,
}

impl Net {
    pub fn local_peer_id(&self) -> PeerId {
        *self.swarm.local_peer_id()
    }

    pub fn local_peer_id_string(&self) -> String {
        self.local_peer_id().to_string()
    }

    pub fn known_listeners(&self) -> Vec<String> {
        self.swarm.listeners().map(|a| a.to_string()).collect()
    }

    pub fn publish_message(&mut self, topic_name: &str, payload: NetworkMessage) -> Result<[u8; 32]> {
        let topic = self
            .topics
            .get(topic_name)
            .ok_or_else(|| anyhow!("unknown topic: {topic_name}"))?
            .clone();

        let now = now_ms();
        let from_peer = self.local_peer_id_string();

        let seed = format!("{from_peer}:{topic_name}:{now}:{payload:?}");
        let msg_id: [u8; 32] = blake3::hash(seed.as_bytes()).into();

        let envelope = Envelope {
            protocol: topics::PROTOCOL_PREFIX.to_string(),
            version: topics::PROTOCOL_VERSION.to_string(),
            from_peer,
            node_name: self.node_name.clone(),
            msg_id,
            timestamp_ms: now,
            payload,
        };

        let bytes = bincode::serialize(&envelope)?;

        match self.swarm.behaviour_mut().gossipsub.publish(topic, bytes) {
            Ok(_) => {
                tracing::debug!("published message to topic={topic_name}");
            }
            Err(gossipsub::PublishError::InsufficientPeers) => {
                tracing::warn!(
                    "not enough gossip peers yet for topic={topic_name}; keeping node alive"
                );
            }
            Err(e) => {
                return Err(e.into());
            }
        }

        Ok(msg_id)
    }

    // Backward compatibility for old ul-node/src/main.rs.
    // Old code calls net.publish(&bincode::serialize(&wire)?)?.
    pub fn publish(&mut self, bytes: &[u8]) -> Result<()> {
        self.publish_message(
            topics::TOPIC_TX,
            NetworkMessage::RawBytes {
                bytes: bytes.to_vec(),
            },
        )?;

        Ok(())
    }

    pub async fn next_event(&mut self) -> Result<ReceivedMessage> {
        loop {
            if let Some(msg) = self.inbox.pop_front() {
                return Ok(msg);
            }

            match self.swarm.select_next_some().await {
                SwarmEvent::NewListenAddr { address, .. } => {
                    tracing::info!("listening on {address}");
                }
                SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => {
                    tracing::info!("connection established peer={peer_id} endpoint={endpoint:?}");
                }
                SwarmEvent::ConnectionClosed { peer_id, cause, .. } => {
                    tracing::warn!("connection closed peer={peer_id} cause={cause:?}");
                }
                SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                    tracing::warn!("outgoing connection error peer={peer_id:?} error={error}");
                }
                SwarmEvent::IncomingConnectionError { error, .. } => {
                    tracing::warn!("incoming connection error error={error}");
                }
                SwarmEvent::Behaviour(BehaviourEvent::Gossipsub(
                    gossipsub::Event::Message {
                        message,
                        propagation_source,
                        ..
                    },
                )) => {
                    let topic = message.topic.to_string();

                    match bincode::deserialize::<Envelope>(&message.data) {
                        Ok(envelope) => {
                            tracing::debug!(
                                "gossip message topic={} propagation_source={} from={}",
                                topic,
                                propagation_source,
                                envelope.from_peer
                            );

                            self.inbox.push_back(ReceivedMessage { topic, envelope });
                        }
                        Err(e) => {
                            tracing::warn!("failed to decode gossip envelope: {e}");
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // Backward compatibility for old ul-node/src/main.rs.
    // Old code waits for Option<Vec<u8>> and then deserializes Wire.
    pub async fn next_bytes(&mut self) -> Option<Vec<u8>> {
        loop {
            match self.next_event().await {
                Ok(msg) => {
                    if msg.envelope.from_peer == self.local_peer_id_string() {
                        continue;
                    }

                    match msg.envelope.payload {
                        NetworkMessage::RawBytes { bytes } => {
                            return Some(bytes);
                        }
                        other => {
                            tracing::debug!(
                                "next_bytes ignored non-raw message on topic {}: {:?}",
                                msg.topic,
                                other
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("next_bytes error: {e}");
                    return None;
                }
            }
        }
    }
}

// Backward-compatible start function for the existing ul-node/src/main.rs.
// Old code calls: p2p_start(listen, bootstrap).await?
pub async fn start(
    listen: Option<Multiaddr>,
    bootstrap: Vec<Multiaddr>,
) -> Result<Net> {
    let listen_addrs = listen.into_iter().collect::<Vec<_>>();
    start_named("ul-node", listen_addrs, bootstrap).await
}

// New explicit start function for protocol tests.
pub async fn start_named(
    node_name: impl Into<String>,
    listen: Vec<Multiaddr>,
    bootstrap: Vec<Multiaddr>,
) -> Result<Net> {
    let node_name = node_name.into();

    let kp = identity::Keypair::generate_ed25519();
    let local_peer = kp.public().to_peer_id();

    let mut gossipsub = {
        let cfg = gossipsub::ConfigBuilder::default()
            .validation_mode(gossipsub::ValidationMode::Strict)
            .heartbeat_interval(Duration::from_secs(2))
            .build()
            .map_err(|e| anyhow!("gossipsub config failed: {e}"))?;

        gossipsub::Behaviour::new(MessageAuthenticity::Signed(kp.clone()), cfg)
            .map_err(|e| anyhow!("gossipsub init failed: {e}"))?
    };

    let mut topics_map = HashMap::new();

    for topic_name in topics::all_topics() {
        let topic = IdentTopic::new(topic_name);
        gossipsub.subscribe(&topic)?;
        topics_map.insert(topic_name.to_string(), topic);
    }

    let identify = identify::Behaviour::new(identify::Config::new(
        "/tally-free/1.0.0".to_string(),
        kp.public(),
    ));

    let store = kad::store::MemoryStore::new(local_peer);
    let mut kademlia = kad::Behaviour::new(local_peer, store);

    for addr in &bootstrap {
        let mut without_peer = addr.clone();

        if let Some(Protocol::P2p(peer)) = without_peer.pop() {
            kademlia.add_address(&peer, without_peer);
        }
    }

    let autonat = autonat::Behaviour::new(local_peer, Default::default());

    let mut swarm = SwarmBuilder::with_existing_identity(kp)
        .with_tokio()
        .with_quic()
        .with_behaviour(move |keypair| {
            let local_peer = keypair.public().to_peer_id();

            Ok(Behaviour {
                gossipsub,
                identify,
                kademlia,
                autonat,
                dcutr: dcutr::Behaviour::new(local_peer),
            })
        })?
        .build();

    for addr in listen {
        swarm.listen_on(addr)?;
    }

    for addr in bootstrap {
        swarm.dial(addr)?;
    }

    Ok(Net {
        swarm,
        node_name,
        topics: topics_map,
        inbox: VecDeque::new(),
    })
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}