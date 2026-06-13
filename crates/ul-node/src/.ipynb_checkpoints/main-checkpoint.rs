use anyhow::{ensure, Result};
use clap::{Parser, Subcommand};
use libp2p::Multiaddr;
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use tracing::{info, warn};
use ul_crypto::Keypair;
use ul_ledger::Ledger;
use ul_p2p::{topics, KnownPeer, NetworkMessage};
use ul_types::{
    hash_block, hex_hash, AccountId, Amount, Block, BlockCapacity, SignedTx, TxKind, Vote, Wire,
};

#[derive(Debug, Parser)]
#[command(name = "ul-node", version, about = "Tally Free node")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create the original chain once.
    ///
    /// This creates genesis locally. The wallet used here owns the original 1.0 token.
    CreateChain(NodeArgs),

    /// Join an existing chain by syncing blocks from peers.
    ///
    /// This does not create genesis. It asks the network for the chain.
    JoinChain(NodeArgs),
}

#[derive(Debug, Parser, Clone)]
struct NodeArgs {
    /// Human-readable name for this node.
    #[arg(long, default_value = "node")]
    name: String,

    /// Local sled database path.
    #[arg(long, default_value = "./node-db")]
    db: String,

    /// Wallet keystore path for this node.
    #[arg(long, default_value = "./keystore.json")]
    keystore: String,

    /// Password for the wallet keystore.
    #[arg(long, default_value = "dev")]
    password: String,

    /// Chain ID string.
    #[arg(long, default_value = "tally-free-testnet")]
    chain_id: String,

    /// Listen address. May be passed multiple times.
    #[arg(long = "listen")]
    listen: Vec<String>,

    /// Peer/relay address to dial. May be passed multiple times.
    #[arg(long = "dial")]
    dial: Vec<String>,

    /// If set, this node will propose blocks from its mempool.
    #[arg(long)]
    proposer: bool,

    /// Send a signed transfer after startup.
    ///
    /// Receiver does not sign or accept. Sender gives the units away.
    #[arg(long)]
    send_to: Option<String>,

    /// Amount to send, as a decimal token amount.
    #[arg(long)]
    amount: Option<String>,

    /// Seconds to wait before sending startup transaction.
    #[arg(long, default_value_t = 10)]
    send_after_secs: u64,

    /// Seconds between proposal attempts.
    #[arg(long, default_value_t = 12)]
    propose_every_secs: u64,

    /// Seconds between chain sync requests if this node is joining.
    #[arg(long, default_value_t = 6)]
    sync_every_secs: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NodeMode {
    CreateChain,
    JoinChain,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "info,libp2p=warn,libp2p_swarm=warn".to_string()),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::CreateChain(args) => run_node(NodeMode::CreateChain, args).await,
        Command::JoinChain(args) => run_node(NodeMode::JoinChain, args).await,
    }
}

async fn run_node(mode: NodeMode, args: NodeArgs) -> Result<()> {
    let kp = ul_keystore::load_or_create(&args.keystore, &args.password)?;
    let my_account = kp.account_id();

    println!("node name: {}", args.name);
    println!("wallet address: {}", my_account.to_hex());
    println!("db: {}", args.db);
    println!("mode: {:?}", mode);

    let ledger = Ledger::open(&args.db)?;

    match mode {
        NodeMode::CreateChain => {
            let meta = ledger.init_genesis(args.chain_id.clone(), my_account.clone())?;

            println!("created/opened chain");
            println!("chain id: {}", meta.chain_id);
            println!("height: {}", meta.height);
            println!("head: {}", hex_hash(&meta.head_hash));
            println!("genesis owner: {}", my_account.to_hex());
            println!("my balance: {}", ledger.balance_of(&my_account)?);
        }
        NodeMode::JoinChain => {
            if ledger.is_initialized()? {
                let meta = ledger.meta()?;
                println!("local chain already exists");
                println!("chain id: {}", meta.chain_id);
                println!("height: {}", meta.height);
                println!("head: {}", hex_hash(&meta.head_hash));
                println!("my balance: {}", ledger.balance_of(&my_account)?);
            } else {
                println!("local chain is empty; will request blocks from peers");
            }
        }
    }

    let listen: Vec<Multiaddr> = args
        .listen
        .iter()
        .map(|s| s.parse())
        .collect::<Result<Vec<_>, _>>()?;

    let dial: Vec<Multiaddr> = args
        .dial
        .iter()
        .map(|s| s.parse())
        .collect::<Result<Vec<_>, _>>()?;

    let mut net = ul_p2p::start_named(args.name.clone(), listen, dial).await?;

    println!("peer id: {}", net.local_peer_id_string());

    let mut known_peers: HashMap<String, KnownPeer> = HashMap::new();
    let mut seen_messages: HashSet<[u8; 32]> = HashSet::new();
    let mut mempool: Vec<SignedTx> = Vec::new();

    let mut hello_timer = tokio::time::interval(Duration::from_secs(10));
    let mut peer_list_timer = tokio::time::interval(Duration::from_secs(15));
    let mut ping_timer = tokio::time::interval(Duration::from_secs(20));
    let mut sync_timer = tokio::time::interval(Duration::from_secs(args.sync_every_secs));
    let mut proposal_timer = tokio::time::interval(Duration::from_secs(args.propose_every_secs));

    let mut startup_send = if args.send_to.is_some() && args.amount.is_some() {
        Some(Box::pin(tokio::time::sleep(Duration::from_secs(args.send_after_secs))))
    } else {
        None
    };

    loop {
        tokio::select! {
            _ = hello_timer.tick() => {
                let listen_addrs = net.known_listeners();

                publish_network_message(
                    &mut net,
                    topics::TOPIC_PEERS,
                    NetworkMessage::PeerHello { listen_addrs },
                )?;
            }

            _ = peer_list_timer.tick() => {
                publish_network_message(
                    &mut net,
                    topics::TOPIC_PEERS,
                    NetworkMessage::PeerListRequest,
                )?;
            }

            _ = ping_timer.tick() => {
                let nonce = now_nonce();

                publish_network_message(
                    &mut net,
                    topics::TOPIC_HEALTH,
                    NetworkMessage::Ping { nonce },
                )?;
            }

            _ = sync_timer.tick(), if mode == NodeMode::JoinChain => {
                request_sync(&ledger, &mut net)?;
            }

            _ = async {
                if let Some(timer) = startup_send.as_mut() {
                    timer.as_mut().await;
                } else {
                    std::future::pending::<()>().await;
                }
            }, if startup_send.is_some() => {
                startup_send = None;

                ensure!(
                    ledger.is_initialized()?,
                    "cannot send before chain is synced or created"
                );

                let to = AccountId::from_hex(args.send_to.as_ref().unwrap())?;
                let amount = Amount::from_decimal_str(args.amount.as_ref().unwrap())?;

                let tx = make_transfer_tx(&ledger, &kp, to.clone(), amount.clone())?;

                println!(
                    "created signed transfer: from={} to={} amount={} nonce={}",
                    tx.from,
                    to,
                    amount,
                    tx.nonce
                );

                accept_tx_into_mempool(&ledger, &mut mempool, tx.clone())?;
                publish_wire(&mut net, Wire::Tx(tx))?;
            }

            _ = proposal_timer.tick(), if args.proposer => {
                if !ledger.is_initialized()? {
                    info!("proposer: cannot propose before chain is initialized");
                    continue;
                }

                if mempool.is_empty() {
                    info!("proposer: mempool empty");
                    continue;
                }

                match build_and_commit_block(&ledger, &my_account, &mut mempool) {
                    Ok(block) => {
                        let block_hash = hash_block(&block);

                        println!(
                            "proposed and committed block height={} hash={} txs={}",
                            block.header.height,
                            hex_hash(&block_hash),
                            block.txs.len()
                        );

                        publish_wire(&mut net, Wire::Proposal(block))?;
                    }
                    Err(e) => {
                        warn!("proposal failed: {e}");
                    }
                }
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
                        println!("← Ping nonce={} from {}", nonce, msg.envelope.node_name);

                        publish_network_message(
                            &mut net,
                            topics::TOPIC_HEALTH,
                            NetworkMessage::Pong { nonce },
                        )?;
                    }

                    NetworkMessage::Pong { nonce } => {
                        println!("← Pong nonce={} from {}", nonce, msg.envelope.node_name);
                    }

                    NetworkMessage::PeerHello { listen_addrs } => {
                        let peer = KnownPeer {
                            peer_id: msg.envelope.from_peer.clone(),
                            node_name: msg.envelope.node_name.clone(),
                            listen_addrs,
                            last_seen_ms: msg.envelope.timestamp_ms,
                        };

                        println!("← PeerHello from {} ({})", peer.node_name, peer.peer_id);

                        known_peers.insert(peer.peer_id.clone(), peer);

                        let peers = known_peers.values().cloned().collect::<Vec<_>>();

                        publish_network_message(
                            &mut net,
                            topics::TOPIC_PEERS,
                            NetworkMessage::PeerListResponse { peers },
                        )?;
                    }

                    NetworkMessage::PeerListRequest => {
                        let peers = known_peers.values().cloned().collect::<Vec<_>>();

                        publish_network_message(
                            &mut net,
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

                    NetworkMessage::RawBytes { bytes } => {
                        match bincode::deserialize::<Wire>(&bytes) {
                            Ok(wire) => {
                                handle_wire(
                                    &ledger,
                                    &kp,
                                    &my_account,
                                    &mut net,
                                    &mut mempool,
                                    wire,
                                )?;
                            }
                            Err(e) => {
                                warn!("bad Wire bytes: {e}");
                            }
                        }
                    }

                    other => {
                        warn!("ignored non-token network message: {:?}", other);
                    }
                }
            }
        }
    }
}

fn request_sync(ledger: &Ledger, net: &mut ul_p2p::Net) -> Result<()> {
    let from_height = if ledger.is_initialized()? {
        ledger.meta()?.height + 1
    } else {
        0
    };

    println!("requesting blocks from height {}", from_height);

    publish_wire(
        net,
        Wire::GetHeaders {
            from_height,
            limit: 256,
        },
    )?;

    publish_wire(net, Wire::GetBlockByHeight { height: from_height })?;

    Ok(())
}

fn handle_wire(
    ledger: &Ledger,
    kp: &Keypair,
    my_account: &AccountId,
    net: &mut ul_p2p::Net,
    mempool: &mut Vec<SignedTx>,
    wire: Wire,
) -> Result<()> {
    match wire {
        Wire::GetHeaders { from_height, limit } => {
            if !ledger.is_initialized()? {
                return Ok(());
            }

            let blocks = ledger.blocks_from_height(from_height, limit)?;
            let headers = blocks.into_iter().map(|b| b.header).collect::<Vec<_>>();

            println!(
                "← Wire::GetHeaders from_height={} limit={} -> {} headers",
                from_height,
                limit,
                headers.len()
            );

            publish_wire(net, Wire::Headers { headers })?;
        }

        Wire::Headers { headers } => {
            println!("← Wire::Headers count={}", headers.len());

            for header in headers {
                if ledger.is_initialized()? {
                    let local_height = ledger.meta()?.height;

                    if header.height <= local_height {
                        continue;
                    }
                }

                publish_wire(
                    net,
                    Wire::GetBlockByHeight {
                        height: header.height,
                    },
                )?;
            }
        }

        Wire::GetBlockByHeight { height } => {
            if !ledger.is_initialized()? {
                return Ok(());
            }

            println!("← Wire::GetBlockByHeight height={}", height);

            let block = ledger.block_by_height(height)?;

            publish_wire(net, Wire::BlockResponse { block })?;
        }

        Wire::GetBlockByHash { hash } => {
            if !ledger.is_initialized()? {
                return Ok(());
            }

            println!("← Wire::GetBlockByHash hash={}", hex_hash(&hash));

            let block = ledger.block_by_hash(hash)?;

            publish_wire(net, Wire::BlockResponse { block })?;
        }

        Wire::BlockResponse { block } => {
            let Some(block) = block else {
                println!("← Wire::BlockResponse empty");
                return Ok(());
            };

            println!(
                "← Wire::BlockResponse height={} hash={} txs={}",
                block.header.height,
                hex_hash(&hash_block(&block)),
                block.txs.len()
            );

            handle_received_block(ledger, mempool, block)?;
            print_my_balance(ledger, my_account)?;
        }

        Wire::Tx(tx) => {
            println!(
                "← Wire::Tx from={} nonce={} kind={:?}",
                tx.from,
                tx.nonce,
                tx.kind
            );

            match accept_tx_into_mempool(ledger, mempool, tx.clone()) {
                Ok(()) => {
                    println!("accepted tx into mempool; mempool={}", mempool.len());
                    publish_wire(net, Wire::Tx(tx))?;
                }
                Err(e) => {
                    warn!("rejected tx: {e}");
                }
            }
        }

        Wire::Proposal(block) => {
            println!(
                "← Wire::Proposal height={} hash={} txs={}",
                block.header.height,
                hex_hash(&hash_block(&block)),
                block.txs.len()
            );

            handle_received_block(ledger, mempool, block.clone())?;
            print_my_balance(ledger, my_account)?;

            let vote = Vote {
                voter: my_account.clone(),
                height: block.header.height,
                block_hash: hash_block(&block),
                accept: true,
                stake_units: ledger.stake_of(my_account)?.0,
                public_key: kp.vk.as_bytes().to_vec(),
                sig: vec![],
            };

            publish_wire(net, Wire::Vote(vote))?;
        }

        Wire::Vote(vote) => {
            println!(
                "← Wire::Vote voter={} height={} accept={} hash={}",
                vote.voter,
                vote.height,
                vote.accept,
                hex_hash(&vote.block_hash)
            );
        }

        Wire::Commit(cert) => {
            println!(
                "← Wire::Commit height={} hash={} votes={}",
                cert.height,
                hex_hash(&cert.block_hash),
                cert.votes.len()
            );
        }

        other => {
            warn!("ignored Wire variant: {:?}", other);
        }
    }

    Ok(())
}

fn handle_received_block(
    ledger: &Ledger,
    mempool: &mut Vec<SignedTx>,
    block: Block,
) -> Result<()> {
    if !ledger.is_initialized()? {
        ensure!(block.header.height == 0, "empty node must receive genesis first");

        let meta = ledger.install_chain_from_blocks(vec![block])?;

        println!(
            "installed received genesis chain_id={} height={} head={}",
            meta.chain_id,
            meta.height,
            hex_hash(&meta.head_hash)
        );

        return Ok(());
    }

    let meta = ledger.meta()?;

    if block.header.height <= meta.height {
        return Ok(());
    }

    ensure!(
        block.header.height == meta.height + 1,
        "cannot install non-contiguous block; have {}, got {}",
        meta.height,
        block.header.height
    );

    let existing_blocks = ledger.blocks_from_height(0, meta.height + 1)?;
    let mut blocks = existing_blocks;
    blocks.push(block.clone());

    let new_meta = ledger.install_chain_from_blocks(blocks)?;

    println!(
        "installed received block height={} new_head={}",
        new_meta.height,
        hex_hash(&new_meta.head_hash)
    );

    remove_committed_txs(mempool, &block);

    Ok(())
}

fn make_transfer_tx(
    ledger: &Ledger,
    kp: &Keypair,
    to: AccountId,
    amount: Amount,
) -> Result<SignedTx> {
    let from = kp.account_id();
    let nonce = ledger.nonce_of(&from)?;

    let mut tx = SignedTx {
        from,
        nonce,
        kind: TxKind::Transfer { to, amount },
        public_key: kp.vk.as_bytes().to_vec(),
        sig: vec![],
        relay_node: None,
    };

    tx.sig = kp.sign(&tx.signing_bytes());

    Ok(tx)
}

fn accept_tx_into_mempool(
    ledger: &Ledger,
    mempool: &mut Vec<SignedTx>,
    tx: SignedTx,
) -> Result<()> {
    ensure!(
        ledger.is_initialized()?,
        "cannot accept tx before chain is initialized"
    );

    let tx_id = tx.tx_id();

    if mempool.iter().any(|existing| existing.tx_id() == tx_id) {
        return Ok(());
    }

    let meta = ledger.meta()?;

    let _candidate = ledger.build_unsigned_block(
        tx.from.clone(),
        meta.height + 1,
        vec![tx.clone()],
        BlockCapacity::default(),
    )?;

    mempool.push(tx);

    Ok(())
}

fn build_and_commit_block(
    ledger: &Ledger,
    proposer: &AccountId,
    mempool: &mut Vec<SignedTx>,
) -> Result<Block> {
    ensure!(!mempool.is_empty(), "mempool empty");

    let meta = ledger.meta()?;

    let txs = mempool.clone();

    let block = ledger.build_unsigned_block(
        proposer.clone(),
        meta.height + 1,
        txs,
        BlockCapacity::default(),
    )?;

    ledger.commit_block(&block)?;

    remove_committed_txs(mempool, &block);

    Ok(block)
}

fn remove_committed_txs(mempool: &mut Vec<SignedTx>, block: &Block) {
    let committed = block
        .txs
        .iter()
        .map(|tx| tx.tx_id())
        .collect::<HashSet<_>>();

    mempool.retain(|tx| !committed.contains(&tx.tx_id()));
}

fn publish_wire(net: &mut ul_p2p::Net, wire: Wire) -> Result<()> {
    let bytes = bincode::serialize(&wire)?;
    net.publish(&bytes)?;
    Ok(())
}

fn publish_network_message(
    net: &mut ul_p2p::Net,
    topic: &str,
    msg: NetworkMessage,
) -> Result<()> {
    net.publish_message(topic, msg)?;
    Ok(())
}

fn print_my_balance(ledger: &Ledger, who: &AccountId) -> Result<()> {
    let balance = ledger.balance_of(who)?;
    println!("my balance: {}", balance);
    Ok(())
}

fn now_nonce() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}