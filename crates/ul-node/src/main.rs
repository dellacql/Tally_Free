use anyhow::{Result, anyhow, ensure};
use blake3::hash;
use clap::{Args, Parser, Subcommand};
use ed25519_dalek::{Signature as DalekSignature, Verifier, VerifyingKey};
use hex::encode as hex_encode;
use libp2p::Multiaddr;
use num_bigint::BigUint;
use rpassword::prompt_password;
use serde::{Deserialize, Serialize};
use sled::CompareAndSwapError;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{
    fs,
    io::{self, Read},
};
use ul_crypto::Keypair;
use ul_p2p::{Net, Wire, start as p2p_start};

#[derive(Debug, Args, Clone)]
struct SecretCli {
    #[arg(long)]
    password: Option<String>,

    #[arg(long, conflicts_with = "password")]
    password_file: Option<PathBuf>,

    #[arg(long, conflicts_with_all = ["password", "password_file"])]
    password_stdin: bool,
}

fn get_password(secret: &SecretCli, prompt: &str) -> anyhow::Result<String> {
    if let Some(p) = &secret.password {
        return Ok(p.clone());
    }
    if let Some(f) = &secret.password_file {
        let s = fs::read_to_string(f)?;
        return Ok(s.trim().to_owned());
    }
    if secret.password_stdin {
        let mut s = String::new();
        io::stdin().read_to_string(&mut s)?;
        return Ok(s.trim().to_owned());
    }
    Ok(prompt_password(prompt)?)
}

#[derive(Debug, Parser, Clone)]
#[command(name = "ul-node", version, about = "Unity-Ledger node")]
struct Cli {
    #[arg(long)]
    db: String,

    #[arg(long)]
    keystore: String,

    #[command(flatten)]
    secret: SecretCli,

    /// Listen multiaddr (e.g. /ip4/0.0.0.0/udp/7001/quic-v1)
    #[arg(long)]
    listen: Option<String>,

    /// One or more peer multiaddrs to dial
    #[arg(long)]
    dial: Vec<String>,

    #[command(subcommand)]
    cmd: Command,
}

#[derive(Debug, Subcommand, Clone)]
enum Command {
    Genesis,
    Stake(StakeArgs),
    CreateAccount(CreateAccountArgs),
    Transfer(TransferArgs),
    Balance(BalanceArgs),
    PrintBlocks(PrintBlocksArgs),
    Run,
}

#[derive(Args, Debug, Clone)]
struct StakeArgs {
    #[arg(long)]
    amount: String,
}

#[derive(Args, Debug, Clone)]
struct CreateAccountArgs {
    #[arg(long)]
    addr_hex32: String,
}

#[derive(Args, Debug, Clone)]
struct TransferArgs {
    #[arg(long)]
    to_hex32: String,
    #[arg(long)]
    amount: String,
}

#[derive(Args, Debug, Clone)]
struct BalanceArgs {
    #[arg(long)]
    addr_hex32: Option<String>,
}

#[derive(Args, Debug, Clone)]
struct PrintBlocksArgs {
    #[arg(long, default_value_t = 20)]
    last: usize,
}

/* ---------------- Block + Tx ---------------- */

#[derive(Serialize, Deserialize, Debug, Clone)]
enum Tx {
    CreateAccount {
        addr: [u8; 32],
    },
    Transfer {
        from: [u8; 32],
        to: [u8; 32],
        amount_units_be: Vec<u8>,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct Block {
    height: u64,
    parent: [u8; 32],
    ts: u64,
    txs: Vec<Tx>,
    tx_ids: Vec<[u8; 32]>, // blake3(tx_bytes) in order
    state_root: [u8; 32],
    tx_root: [u8; 32],
}

/* ---------------- Merkle helper ---------------- */

/// Compute a simple Merkle root over a list of 32‑byte hashes using blake3.
/// If there is an odd number of leaves, the last hash is duplicated.
fn merkle_root_hashes(mut leaves: Vec<[u8; 32]>) -> [u8; 32] {
    if leaves.is_empty() {
        return [0u8; 32];
    }
    while leaves.len() > 1 {
        let mut next = Vec::with_capacity((leaves.len() + 1) / 2);
        for pair in leaves.chunks(2) {
            let h = if pair.len() == 2 {
                hash(&[pair[0].as_slice(), pair[1].as_slice()].concat()).into()
            } else {
                pair[0]
            };
            next.push(h);
        }
        leaves = next;
    }
    leaves[0]
}

/* ---------------- Main ---------------- */

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Resolve password securely (hidden prompt if not provided)
    let pw = get_password(&cli.secret, "Keystore password: ")?;
    let kp: ul_crypto::Keypair = ul_keystore::load_or_create(&cli.keystore, &pw)?;

    // Derive my account as [u8; 32] (adjust to your API if needed)
    let my_account: [u8; 32] = kp.account_id().0;

    match &cli.cmd {
        Command::Genesis => {
            do_genesis(&cli.db, &my_account)?;
            println!("✔ genesis initialized at {}", &cli.db);
            Ok(())
        }
        Command::Stake(a) => {
            stake(&cli.db, &my_account, &parse_amount_units(&a.amount)?)?;
            println!("✔ staked {}", a.amount);
            Ok(())
        }
        Command::CreateAccount(a) => {
            let addr = parse_hex32(&a.addr_hex32)?;
            let tx = Tx::CreateAccount { addr };
            enqueue_tx_and_gossip(&cli, &kp, tx)
        }
        Command::Transfer(a) => {
            let to = parse_hex32(&a.to_hex32)?;
            let tx = Tx::Transfer {
                from: my_account,
                to,
                amount_units_be: parse_amount_units(&a.amount)?.to_bytes_be(),
            };
            enqueue_tx_and_gossip(&cli, &kp, tx)
        }
        Command::Balance(a) => {
            let who = if let Some(h) = a.addr_hex32.as_ref() {
                parse_hex32(h)?
            } else {
                my_account
            };
            let dec = load_balance_decimal(&cli.db, who)?;
            println!("{dec}");
            Ok(())
        }
        Command::PrintBlocks(a) => {
            print_blocks(&cli.db, a.last)?;
            Ok(())
        }
        Command::Run => do_run(&cli, &kp).await,
    }
}

/* ---------------- Run: consensus ---------------- */

async fn do_run(cli: &Cli, kp: &Keypair) -> Result<()> {
    let listen = cli.listen.as_deref().map(parse_multiaddr).transpose()?;
    let mut bootstrap = Vec::new();
    for d in &cli.dial {
        bootstrap.push(parse_multiaddr(d)?);
    }
    let mut net: Net = p2p_start(listen, bootstrap).await?;
    println!("p2p started; waiting for events…");

    // subscribe mempool watch loop: every epoch, if we are eligible, propose.
    let epoch = Duration::from_secs(5);

    // vote accumulators: (height, hash) -> stake tally + voters set
    let mut tallies: HashMap<(u64, [u8; 32]), (BigUint, HashSet<[u8; 32]>)> = HashMap::new();

    loop {
        tokio::select! {
            _ = tokio::time::sleep(epoch) => {
                // Propose if we have txs and we have stake > 0
                if get_stake(&cli.db, acct_from_vk(kp.vk.as_bytes())) > BigUint::from(0u32) {
                    if let Some((blk, blk_bytes, blk_hash)) = build_block_from_mempool(&cli.db)? {
                        // sign & publish proposal
                        let height = blk.height;
                        let sig = kp.sign(&[&height.to_be_bytes()[..], &blk_hash[..]].concat());
                        let wire = Wire::Proposal {
                            height,
                            parent: blk.parent,
                            block_bytes: blk_bytes.clone(),
                            block_hash: blk_hash,
                            proposer_account: acct_from_vk(kp.vk.as_bytes()),
                            proposer_vk: kp.vk.to_bytes().to_vec(),
                            proposer_sig: sig,
                        };
                        let msg = bincode::serialize(&wire)?;
                        net.publish(&msg)?;
                        println!("→ PROPOSAL h={} hash={}", height, hex_encode(blk_hash));
                    }
                }
            }

            maybe = net.next_bytes() => {
                if let Some(data) = maybe {
                    if let Ok(wire) = bincode::deserialize::<Wire>(&data) {
                        match wire {
                            Wire::TxCreateAccount{ addr, vk, sig } => {
                                if verify_sig_acct(&kp, "tx_create", &vk, &sig, &addr)? {
                                    enqueue_tx(&cli.db, Tx::CreateAccount{ addr })?;
                                }
                            }
                            Wire::TxTransfer{ from, to, amount_units_be, vk, sig } => {
                                if verify_sig_transfer(&vk, &sig, from, to, &amount_units_be)? {
                                    enqueue_tx(&cli.db, Tx::Transfer{ from, to, amount_units_be })?;
                                }
                            }
                            Wire::Proposal{ height, parent, block_bytes, block_hash, proposer_account, proposer_vk, proposer_sig } => {
                                if verify_block_sig(height, &block_hash, &proposer_vk, &proposer_sig, proposer_account)? &&
                                   verify_and_stage_block(&cli.db, height, parent, &block_bytes, &block_hash)? {
                                    // vote accept with our stake
                                    let my_acct = acct_from_vk(kp.vk.as_bytes());
                                    let my_stake = get_stake(&cli.db, my_acct);
                                    if my_stake > BigUint::from(0u32) {
                                        let accept = true;
                                        let vote_sig = kp.sign(&[&height.to_be_bytes()[..], &block_hash[..], &[1u8][..]].concat());
                                        let wire = Wire::Vote {
                                            height,
                                            block_hash,
                                            accept,
                                            voter_account: my_acct,
                                            voter_stake_be: my_stake.to_bytes_be(),
                                            voter_vk: kp.vk.to_bytes().to_vec(),
                                            voter_sig: vote_sig,
                                        };
                                        net.publish(&bincode::serialize(&wire)?)?;
                                        println!("→ VOTE   h={} hash={} stake={}", height, hex_encode(block_hash), fmt_amount_1e45(&my_stake));
                                    }
                                }
                            }
                            Wire::Vote{ height, block_hash, accept, voter_account, voter_stake_be, voter_vk, voter_sig } => {
                                if accept && verify_vote_sig(height, &block_hash, &voter_vk, &voter_sig, voter_account)? {
                                    // accept only if stake matches our view (anti-spoof)
                                    let declared = BigUint::from_bytes_be(&voter_stake_be);
                                    let local = get_stake(&cli.db, voter_account);
                                    if declared == local && local > BigUint::from(0u32) {
                                        let key = (height, block_hash);
                                        let (tally, voters) = tallies.entry(key).or_insert((BigUint::from(0u32), HashSet::new()));
                                        if voters.insert(voter_account) {
                                            *tally += local.clone();
                                        }
                                        // check quorum
                                        let total = total_active_stake(&cli.db);
                                        if total > BigUint::from(0u32) {
                                            let pct = (&*tally * BigUint::from(100u32)) / total.clone();
                                            if pct >= BigUint::from(72u32) {
                                                // emit commit
                                                let my_acct = acct_from_vk(kp.vk.as_bytes());
                                                let sig = kp.sign(&[&height.to_be_bytes()[..], &block_hash[..], b"commit"].concat());
                                                let wire = Wire::Commit {
                                                    height, block_hash,
                                                    committer_account: my_acct,
                                                    committer_vk: kp.vk.to_bytes().to_vec(),
                                                    committer_sig: sig,
                                                };
                                                net.publish(&bincode::serialize(&wire)?)?;
                                                println!("→ COMMIT h={} hash={} (quorum reached)", height, hex_encode(block_hash));
                                            }
                                        }
                                    }
                                }
                            }
                            Wire::Commit{ height, block_hash, committer_account: _, committer_vk, committer_sig } => {
                                if verify_commit_sig(height, &block_hash, &committer_vk, &committer_sig)? {
                                    if commit_if_valid(&cli.db, height, &block_hash)? {
                                        println!("✔ COMMITTED h={} hash={}", height, hex_encode(block_hash));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/* ---------------- State + sled helpers ---------------- */

fn do_genesis(db_path: &str, my_account: &[u8; 32]) -> Result<()> {
    let db = sled::open(db_path)?;
    let meta = db.open_tree("meta")?;
    let balances = db.open_tree("balances")?;
    let admins = db.open_tree("admins")?;
    db.open_tree("stake")?;
    db.open_tree("blocks")?;
    db.open_tree("mempool")?;

    if meta.get("height")?.is_some() {
        return Err(anyhow!(
            "genesis refused: DB already initialized (meta.height exists)"
        ));
    }

    meta.insert("height", 0u64.to_be_bytes().to_vec())?;
    meta.insert("parent", [0u8; 32].to_vec())?;
    balances.insert(my_account, pow10_be(45))?;
    admins.insert(my_account, &[])?;
    db.flush()?;
    Ok(())
}

fn compute_state_root_for_block(db_path: &str, blk: &Block) -> Result<[u8; 32]> {
    // open balances tree and load existing balances into a BTreeMap
    let db = sled::open(db_path)?;
    let balances = db.open_tree("balances")?;
    let mut shadow: BTreeMap<[u8; 32], BigUint> = BTreeMap::new();
    for kv in balances.iter() {
        let (k, v) = kv?;
        let mut key = [0u8; 32];
        key.copy_from_slice(&k);
        let val = if v.is_empty() {
            BigUint::from(0u32)
        } else {
            BigUint::from_bytes_be(v.as_ref())
        };
        shadow.insert(key, val);
    }
    // apply the block’s transactions to the shadow state
    for tx in &blk.txs {
        match tx {
            Tx::CreateAccount { addr } => {
                shadow.entry(*addr).or_insert(BigUint::from(0u32));
            }
            Tx::Transfer {
                from,
                to,
                amount_units_be,
            } => {
                let amt = BigUint::from_bytes_be(amount_units_be);
                let f = shadow
                    .get_mut(from)
                    .ok_or_else(|| anyhow!("from not found"))?;
                ensure!(*f >= amt, "insufficient funds");
                *f -= &amt;
                let t = shadow.entry(*to).or_insert(BigUint::from(0u32));
                *t += amt;
            }
        }
    }
    // serialize the state into a byte buffer in key order
    let mut buf: Vec<u8> = Vec::new();
    for (k, v) in &shadow {
        buf.extend_from_slice(&k[..]);
        let vb = v.to_bytes_be();
        // store length prefix to avoid collisions when concatenating
        buf.extend_from_slice(&(vb.len() as u32).to_be_bytes());
        buf.extend_from_slice(&vb);
    }
    // hash with blake3
    Ok(blake3::hash(&buf).into())
}

fn stake(db_path: &str, who: &[u8; 32], amount: &BigUint) -> Result<()> {
    let db = sled::open(db_path)?;
    let balances = db.open_tree("balances")?;
    let stake = db.open_tree("stake")?;

    let bal = get_units(&balances, *who);
    ensure!(bal >= *amount, "insufficient balance to stake");
    balances.insert(who, (bal - amount).to_bytes_be())?;

    let cur = get_units(&stake, *who);
    stake.insert(who, (cur + amount).to_bytes_be())?;
    Ok(())
}

fn enqueue_tx_and_gossip(cli: &Cli, _kp: &Keypair, tx: Tx) -> Result<()> {
    enqueue_tx(&cli.db, tx)?;
    println!("✔ submitted TX (will be included in next proposal)");
    Ok(())
}

fn enqueue_tx(db_path: &str, tx: Tx) -> Result<()> {
    let db = sled::open(db_path)?;
    let mp = db.open_tree("mempool")?;
    let bytes = bincode::serialize(&tx)?;
    let key = hash(&bytes).as_bytes().to_vec(); // txid
    // insert if missing
    let res = mp.compare_and_swap(key.clone(), None::<&[u8]>, Some(bytes))?;
    match res {
        Ok(()) => Ok(()),
        Err(CompareAndSwapError {
            current: Some(_), ..
        }) => Ok(()), // already present
        Err(e) => Err(anyhow!("mempool CAS: {e:?}")),
    }
}

/// Try to build a block from mempool; returns (Block, bytes, hash).
fn build_block_from_mempool(db_path: &str) -> Result<Option<(Block, Vec<u8>, [u8; 32])>> {
    let db = sled::open(db_path)?;
    let mempool = db.open_tree("mempool")?;
    let meta = db.open_tree("meta")?;
    // read current height and parent
    let (height, parent) = read_meta(&meta)?;
    // gather txs from mempool and compute total amount of transfers
    let mut txs: Vec<Tx> = Vec::new();
    let mut tx_ids: Vec<[u8; 32]> = Vec::new();
    let mut total_transfers = BigUint::from(0u32);
    for kv in mempool.iter() {
        let (_k, v) = kv?;
        let tx: Tx = bincode::deserialize(&v)?;
        if let Tx::Transfer {
            amount_units_be, ..
        } = &tx
        {
            total_transfers += BigUint::from_bytes_be(&amount_units_be);
        }
        tx_ids.push(hash(&v).into());
        txs.push(tx);
    }
    if txs.is_empty() {
        return Ok(None);
    }
    // compute tx_root as Merkle root over tx_ids
    let tx_root = merkle_root_hashes(tx_ids.clone());
    // check proposer stake; skip proposal if transfers exceed stake
    // In practice this function should take the proposer’s stake as a parameter.
    // As a placeholder we read the proposer’s stake from the local DB using the
    // identity derived from an empty key; callers must update this code to
    // correctly identify their own account.
    let my_acct = acct_from_vk(blake3::hash(&[]).as_bytes());
    let my_stake = get_stake(db_path, my_acct);
    if total_transfers > my_stake {
        return Ok(None);
    }
    // construct block with zero state_root; will be filled in below
    let mut blk = Block {
        height: height + 1,
        parent,
        ts: now_secs(),
        txs: txs.clone(),
        tx_ids: tx_ids.clone(),
        state_root: [0u8; 32],
        tx_root,
    };
    // dry-run: verify block applies
    verify_block_application(db_path, &blk)?;
    // compute state root and update block
    let root = compute_state_root_for_block(db_path, &blk)?;
    blk.state_root = root;
    // serialize and hash
    let bytes = bincode::serialize(&blk)?;
    let bh = hash(&bytes).into();
    Ok(Some((blk, bytes, bh)))
}

/// Verify proposal and stage (no commit yet). Returns true if OK.
fn verify_and_stage_block(
    db_path: &str,
    height: u64,
    parent: [u8; 32],
    block_bytes: &[u8],
    block_hash: &[u8; 32],
) -> Result<bool> {
    let blk: Block = bincode::deserialize(block_bytes)?;
    ensure!(blk.height == height, "height mismatch");
    ensure!(blk.parent == parent, "parent mismatch to proposal meta");
    ensure!(
        hash(block_bytes).as_bytes() == block_hash,
        "block hash mismatch"
    );
    verify_block_application(db_path, &blk)?;
    Ok(true)
}

/// After quorum, re-check parent==meta.parent and commit (idempotent).
fn commit_if_valid(db_path: &str, height: u64, block_hash: &[u8; 32]) -> Result<bool> {
    let db = sled::open(db_path)?;
    let meta = db.open_tree("meta")?;
    let blocks = db.open_tree("blocks")?;
    // if already committed, return Ok(false)
    if let Some(hbytes) = meta.get("height")? {
        let cur = u64::from_be_bytes(hbytes.as_ref().try_into().unwrap_or([0u8; 8]));
        if cur >= height {
            return Ok(false);
        }
    }
    // find the proposed block bytes by scanning mempool selection again isn’t great;
    // but proposer sent full block in Proposal; cache it in blocks under temp key? Simplify:
    // store using key=height just before commit: require Proposal path to have run apply verify (which it did).
    // For commit, we only trust the parent continuity.
    let (cur_h, _cur_parent) = read_meta(&meta)?;
    ensure!(height == cur_h + 1, "commit height continuity failed");
    // In this simplified path, we require block_bytes to be re-fetched — here omitted; in practice cache it.
    // For safety we return true only if parent continuity matches; application already verified on Proposal/Vote sides.
    meta.insert("height", height.to_be_bytes().to_vec())?;
    meta.insert("parent", block_hash.as_slice())?;
    blocks.insert(height.to_be_bytes(), block_hash.as_slice())?; // store hash-as-value; optional
    // Clear mempool txs that were included in the last verified build (best-effort):
    // In a production version, the Commit should carry tx_ids; here we optimistically clear nothing.
    blocks.flush()?;
    meta.flush()?;
    Ok(true)
}

fn verify_block_application(db_path: &str, blk: &Block) -> Result<()> {
    let db = sled::open(db_path)?;
    let balances = db.open_tree("balances")?;
    // simulate application on a shadow map
    let mut shadow: BTreeMap<[u8; 32], BigUint> = BTreeMap::new();
    for kv in balances.iter() {
        let (k, v) = kv?;
        let mut key = [0u8; 32];
        key.copy_from_slice(&k);
        let val = if v.is_empty() {
            BigUint::from(0u32)
        } else {
            BigUint::from_bytes_be(v.as_ref())
        };
        shadow.insert(key, val);
    }
    for tx in &blk.txs {
        match tx {
            Tx::CreateAccount { addr } => {
                shadow.entry(*addr).or_insert(BigUint::from(0u32));
            }
            Tx::Transfer {
                from,
                to,
                amount_units_be,
            } => {
                let amt = BigUint::from_bytes_be(amount_units_be);
                let f = shadow
                    .get_mut(from)
                    .ok_or_else(|| anyhow!("from not found"))?;
                ensure!(*f >= amt, "insufficient funds");
                *f -= &amt;
                let t = shadow.entry(*to).or_insert(BigUint::from(0u32));
                *t += amt;
            }
        }
    }
    // supply invariant
    let mut sum = BigUint::from(0u32);
    for v in shadow.values() {
        sum += v.clone();
    }
    ensure!(sum == pow10_big(45), "supply invariant broken");
    Ok(())
}

/* ---------------- Sign/verify helpers ---------------- */

fn acct_from_vk(vk_bytes: &[u8]) -> [u8; 32] {
    hash(vk_bytes).into()
}

fn verify_sig_acct(
    _kp: &Keypair,
    _tag: &str,
    vk: &[u8],
    sig: &[u8],
    addr: &[u8; 32],
) -> Result<bool> {
    let vk = VerifyingKey::from_bytes(vk.try_into().map_err(|_| anyhow!("bad vk len"))?)?;
    let acct = acct_from_vk(vk.as_bytes());
    ensure!(&acct == addr, "addr != blake3(vk)");
    let s = DalekSignature::from_slice(sig)?;
    vk.verify(addr, &s).map_err(|e| anyhow!("sig fail: {e}"))?;
    Ok(true)
}

fn verify_sig_transfer(
    vk: &[u8],
    sig: &[u8],
    from: [u8; 32],
    to: [u8; 32],
    amt: &[u8],
) -> Result<bool> {
    let vk = VerifyingKey::from_bytes(vk.try_into().map_err(|_| anyhow!("bad vk len"))?)?;
    let acct = acct_from_vk(vk.as_bytes());
    ensure!(acct == from, "from != blake3(vk)");
    let s = DalekSignature::from_slice(sig)?;
    vk.verify(&[&from[..], &to[..], amt].concat(), &s)
        .map_err(|e| anyhow!("sig fail: {e}"))?;
    Ok(true)
}

fn verify_block_sig(
    height: u64,
    bh: &[u8; 32],
    proposer_vk: &[u8],
    sig: &[u8],
    acct: [u8; 32],
) -> Result<bool> {
    let vk = VerifyingKey::from_bytes(proposer_vk.try_into().map_err(|_| anyhow!("bad vk len"))?)?;
    ensure!(
        acct_from_vk(vk.as_bytes()) == acct,
        "proposer acct != blake3(vk)"
    );
    let s = DalekSignature::from_slice(sig)?;
    vk.verify(&[&height.to_be_bytes()[..], &bh[..]].concat(), &s)
        .map_err(|e| anyhow!("proposal sig fail: {e}"))?;
    Ok(true)
}

fn verify_vote_sig(
    height: u64,
    bh: &[u8; 32],
    voter_vk: &[u8],
    sig: &[u8],
    acct: [u8; 32],
) -> Result<bool> {
    let vk = VerifyingKey::from_bytes(voter_vk.try_into().map_err(|_| anyhow!("bad vk len"))?)?;
    ensure!(
        acct_from_vk(vk.as_bytes()) == acct,
        "voter acct != blake3(vk)"
    );
    let s = DalekSignature::from_slice(sig)?;
    vk.verify(
        &[&height.to_be_bytes()[..], &bh[..], &[1u8][..]].concat(),
        &s,
    )
    .map_err(|e| anyhow!("vote sig fail: {e}"))?;
    Ok(true)
}

fn verify_commit_sig(height: u64, bh: &[u8; 32], vk_bytes: &[u8], sig: &[u8]) -> Result<bool> {
    let vk = VerifyingKey::from_bytes(vk_bytes.try_into().map_err(|_| anyhow!("bad vk len"))?)?;
    let s = DalekSignature::from_slice(sig)?;
    vk.verify(
        &[&height.to_be_bytes()[..], &bh[..], b"commit"].concat(),
        &s,
    )
    .map_err(|e| anyhow!("commit sig fail: {e}"))?;
    Ok(true)
}

/* ---------------- sled/state helpers ---------------- */

fn parse_multiaddr(s: &str) -> Result<Multiaddr> {
    s.parse().map_err(|e| anyhow!("bad multiaddr: {e}"))
}

fn read_meta(meta: &sled::Tree) -> Result<(u64, [u8; 32])> {
    let height = meta
        .get("height")?
        .map(|v| u64::from_be_bytes(v.as_ref().try_into().unwrap()))
        .unwrap_or(0);
    let parent = meta
        .get("parent")?
        .map(|v| {
            let mut p = [0u8; 32];
            p.copy_from_slice(v.as_ref());
            p
        })
        .unwrap_or([0u8; 32]);
    Ok((height, parent))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn pow10_big(exp: u32) -> BigUint {
    let mut n = BigUint::from(1u32);
    for _ in 0..exp {
        n *= 10u32;
    }
    n
}

fn pow10_be(exp: u32) -> Vec<u8> {
    pow10_big(exp).to_bytes_be()
}

fn get_units(tree: &sled::Tree, who: [u8; 32]) -> BigUint {
    if let Ok(Some(v)) = tree.get(who) {
        if v.is_empty() {
            BigUint::from(0u32)
        } else {
            BigUint::from_bytes_be(v.as_ref())
        }
    } else {
        BigUint::from(0u32)
    }
}

fn get_stake(db_path: &str, who: [u8; 32]) -> BigUint {
    let db = sled::open(db_path).expect("db");
    let stake = db.open_tree("stake").expect("stake");
    get_units(&stake, who)
}

fn total_active_stake(db_path: &str) -> BigUint {
    let db = sled::open(db_path).expect("db");
    let stake = db.open_tree("stake").expect("stake");
    let mut s = BigUint::from(0u32);
    for kv in stake.iter() {
        let (_k, v) = kv.unwrap();
        if !v.is_empty() {
            s += BigUint::from_bytes_be(v.as_ref());
        }
    }
    s
}

fn load_balance_decimal(db_path: &str, who: [u8; 32]) -> Result<String> {
    let db = sled::open(db_path)?;
    let balances = db.open_tree("balances")?;
    Ok(fmt_amount_1e45(&get_units(&balances, who)))
}

fn fmt_amount_1e45(n: &BigUint) -> String {
    let scale = pow10_big(45);
    let (q, r) = (n / &scale, n % &scale);
    let int_part = q.to_string();
    let mut frac = r.to_string();
    if frac.len() < 45 {
        frac = "0".repeat(45 - frac.len()) + &frac;
    }
    let frac_trim = frac.trim_end_matches('0').to_string();
    if frac_trim.is_empty() {
        int_part
    } else {
        format!("{int_part}.{frac_trim}")
    }
}

fn parse_hex32(s: &str) -> Result<[u8; 32]> {
    let b = hex::decode(s)?;
    ensure!(b.len() == 32, "expected 32 bytes hex");
    let mut out = [0u8; 32];
    out.copy_from_slice(&b);
    Ok(out)
}

fn parse_amount_units(s: &str) -> Result<BigUint> {
    let s = s.trim();
    ensure!(!s.is_empty(), "empty amount");
    let parts: Vec<&str> = s.split('.').collect();
    ensure!(parts.len() <= 2, "bad decimal");
    let whole = parts[0].replace('_', "");
    let mut frac = if parts.len() == 2 {
        parts[1].replace('_', "")
    } else {
        "".into()
    };
    ensure!(whole.chars().all(|c| c.is_ascii_digit()), "bad digits");
    ensure!(frac.chars().all(|c| c.is_ascii_digit()), "bad digits");
    ensure!(frac.len() <= 45, "too many fractional digits");
    while frac.len() < 45 {
        frac.push('0');
    }
    let units = format!("{whole}{frac}").trim_start_matches('0').to_string();
    Ok(if units.is_empty() {
        BigUint::from(0u32)
    } else {
        BigUint::parse_bytes(units.as_bytes(), 10).unwrap()
    })
}

fn print_blocks(db_path: &str, last: usize) -> Result<()> {
    let db = sled::open(db_path)?;
    let blocks = db.open_tree("blocks")?;
    let mut keys: Vec<Vec<u8>> = blocks.iter().map(|kv| kv.unwrap().0.to_vec()).collect();
    keys.sort();
    let start = if last == 0 || last >= keys.len() {
        0
    } else {
        keys.len() - last
    };
    for k in &keys[start..] {
        let v = blocks.get(k)?.unwrap();
        println!(
            "h={} val_len={} value_hex={}",
            u64::from_be_bytes(k.as_slice().try_into().unwrap()),
            v.len(),
            hex_encode(v)
        );
    }
    Ok(())
}
