use anyhow::{anyhow, Result};
use libp2p::{
    autonat, dcutr, gossipsub,
    gossipsub::{IdentTopic, Message, MessageAuthenticity},
    identify, identity, kad,
    futures::StreamExt,
    swarm::{NetworkBehaviour, SwarmEvent},
    Multiaddr, Swarm,     // <-- add Swarm
    multiaddr::Protocol,             // already added earlier
};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::time::Duration;


pub const TOPIC_STR: &str = "unity-ledger-v1";

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Wire {
    // Client transactions
    TxCreateAccount {
        addr: [u8; 32],
        vk: Vec<u8>,              // sender vk (so others can verify AccountId if needed)
        sig: Vec<u8>,             // sig(addr)
    },
    TxTransfer {
        from: [u8; 32],
        to: [u8; 32],
        amount_units_be: Vec<u8>, // BigUint bytes
        vk: Vec<u8>,              // sender vk
        sig: Vec<u8>,             // sig(from||to||amount_bytes)
    },

    // Block proposal / vote / commit
    Proposal {
        height: u64,
        parent: [u8; 32],
        block_bytes: Vec<u8>,     // bincode(Block)
        block_hash: [u8; 32],     // blake3(block_bytes)
        proposer_account: [u8; 32],
        proposer_vk: Vec<u8>,
        proposer_sig: Vec<u8>,    // sig(height||block_hash)
    },
    Vote {
        height: u64,
        block_hash: [u8; 32],
        accept: bool,
        voter_account: [u8; 32],
        voter_stake_be: Vec<u8>,  // BigUint bytes
        voter_vk: Vec<u8>,
        voter_sig: Vec<u8>,       // sig(height||block_hash||accept)
    },
    Commit {
        height: u64,
        block_hash: [u8; 32],
        committer_account: [u8; 32],
        committer_vk: Vec<u8>,
        committer_sig: Vec<u8>,   // sig(height||block_hash||"commit")
    },
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
    pub(crate) swarm: Swarm<Behaviour>,
    topic: IdentTopic,
    inbox: VecDeque<Vec<u8>>,
}

impl Net {
    pub fn publish(&mut self, bytes: &[u8]) -> Result<()> {
        self.swarm
            .behaviour_mut()
            .gossipsub
            .publish(self.topic.clone(), bytes)?;
        Ok(())
    }

    /// Drive the swarm a bit; return one gossiped message if available.
    pub async fn next_bytes(&mut self) -> Option<Vec<u8>> {
        loop {
            if let Some(b) = self.inbox.pop_front() {
                return Some(b);
            }
            match self.swarm.select_next_some().await {
                SwarmEvent::Behaviour(BehaviourEvent::Gossipsub(e)) => {
                    if let gossipsub::Event::Message {
                        message: Message { data, .. },
                        ..
                    } = e
                    {
                        self.inbox.push_back(data);
                    }
                }
                _ => { /* ignore other events for now */ }
            }
        }
    }
}

pub async fn start(listen: Option<Multiaddr>, bootstrap: Vec<Multiaddr>) -> Result<Net> {
    let kp = identity::Keypair::generate_ed25519();

    // gossipsub config
    let mut gsb = {
        let cfg = gossipsub::ConfigBuilder::default()
            .validation_mode(gossipsub::ValidationMode::Strict)
            .heartbeat_interval(Duration::from_secs(2))
            .build()
            .map_err(|e| anyhow!("gossipsub config: {e}"))?; // map error
        gossipsub::Behaviour::new(MessageAuthenticity::Signed(kp.clone()), cfg)
            .map_err(|e| anyhow!("gossipsub new: {e}"))?      // map error
    };
    let topic = IdentTopic::new(TOPIC_STR);
    gsb.subscribe(&topic)?;

    let identify = identify::Behaviour::new(identify::Config::new(
        "/unity-ledger/1.0".into(),
        kp.public(),
    ));

    // Kademlia with peer bootstrap
    let local_peer = kp.public().to_peer_id();
    let store = kad::store::MemoryStore::new(local_peer);
    let mut kademlia = kad::Behaviour::new(local_peer, store);
    for a in &bootstrap {
        let mut addr = a.clone();
        // BEFORE (wrong for current libp2p):
        // if let Some(Protocol::P2p(mh)) = addr.pop() {
        //     if let Ok(peer) = PeerId::from_multihash(mh) {
        //         kademlia.add_address(&peer, addr);
        //     }
        // }

        // AFTER (correct: P2p carries PeerId)
        if let Some(Protocol::P2p(peer)) = addr.pop() {
            kademlia.add_address(&peer, addr);
        }
    }

    let autonat = autonat::Behaviour::new(local_peer, Default::default());

    // DCUtR needs the local PeerId
    // (we’ll re-derive inside the closure to avoid move issues)
    // let dcutr = dcutr::Behaviour::new(local_peer);

    // Build swarm
    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(kp)
        .with_tokio()
        .with_quic() // sensible defaults for QUIC transport
        .with_behaviour(move |keypair| {
            let lp = keypair.public().to_peer_id();
            Ok(Behaviour {
                gossipsub: gsb,
                identify,
                kademlia,
                autonat,
                dcutr: dcutr::Behaviour::new(lp),
            })
        })?
        .build();

    if let Some(addr) = listen {
        swarm.listen_on(addr)?;
    }
    for a in bootstrap {
        swarm.dial(a)?;
    }

    Ok(Net {
        swarm,
        topic,
        inbox: VecDeque::new(),
    })
}

