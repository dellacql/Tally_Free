use anyhow::{anyhow, ensure, Result};
use argon2::Argon2;
use chacha20poly1305::{aead::Aead, ChaCha20Poly1305, KeyInit};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use num_bigint::BigUint;
use num_traits::Zero;
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sled::{Db, Tree};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use ul_types::*;

const TREE_META: &str = "meta";
const TREE_BLOCKS_BY_HEIGHT: &str = "blocks_by_height";
const TREE_BLOCKS_BY_HASH: &str = "blocks_by_hash";
const TREE_CANONICAL_HASH_BY_HEIGHT: &str = "canonical_hash_by_height";
const TREE_PROPOSALS_BY_HASH: &str = "proposals_by_hash";
const TREE_BALANCES: &str = "balances";
const TREE_STAKE: &str = "stake";
const TREE_ADMINS: &str = "admins";
const TREE_NONCES: &str = "nonces";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainMeta {
    pub chain_id: String,
    pub height: u64,
    pub head_hash: Hash32,
    pub genesis_hash: Hash32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StateSnapshot {
    pub balances: BTreeMap<AccountId, Amount>,
    pub stake: BTreeMap<AccountId, Amount>,
    pub admins: BTreeSet<AccountId>,
    pub nonces: BTreeMap<AccountId, u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainExport {
    pub magic: String,
    pub version: u32,
    pub meta: ChainMeta,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyReport {
    pub ok: bool,
    pub checked_blocks: usize,
    pub head_height: u64,
    pub head_hash: Hash32,
    pub errors: Vec<String>,
}

#[derive(Clone)]
pub struct Ledger {
    db: Db,
}

impl Ledger {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let db = sled::open(path)?;
        let ledger = Self { db };
        ledger.ensure_trees()?;
        Ok(ledger)
    }

    pub fn from_db(db: Db) -> Result<Self> {
        let ledger = Self { db };
        ledger.ensure_trees()?;
        Ok(ledger)
    }

    fn ensure_trees(&self) -> Result<()> {
        for name in [
            TREE_META,
            TREE_BLOCKS_BY_HEIGHT,
            TREE_BLOCKS_BY_HASH,
            TREE_CANONICAL_HASH_BY_HEIGHT,
            TREE_PROPOSALS_BY_HASH,
            TREE_BALANCES,
            TREE_STAKE,
            TREE_ADMINS,
            TREE_NONCES,
        ] {
            self.db.open_tree(name)?;
        }

        Ok(())
    }

    pub fn init_genesis(&self, chain_id: impl Into<String>, owner: AccountId) -> Result<ChainMeta> {
        let chain_id = chain_id.into();

        if self.meta_tree()?.get("height")?.is_some() {
            return self.meta();
        }

        self.balances_tree()?
            .insert(owner.0.to_vec(), bincode::serialize(&Amount::one_coin())?)?;

        self.nonces_tree()?
            .insert(owner.0.to_vec(), 0u64.to_be_bytes().to_vec())?;

        self.admins_tree()?.insert(owner.0.to_vec(), vec![1])?;

        let mut genesis = Block {
            header: BlockHeader {
                chain_id: chain_id.clone(),
                height: 0,
                parent_hash: [0u8; 32],
                tx_root: [0u8; 32],
                state_root: self.state_root()?,
                proposer: owner,
                period: 0,
                timestamp_ms: now_ms(),
                capacity: BlockCapacity::default(),
            },
            txs: vec![],
            proposer_sig: vec![],
        };

        genesis.header.state_root = self.state_root()?;
        let genesis_hash = hash_block(&genesis);

        self.store_canonical_block(&genesis, genesis_hash)?;

        self.write_meta(&ChainMeta {
            chain_id,
            height: 0,
            head_hash: genesis_hash,
            genesis_hash,
        })?;

        self.db.flush()?;
        self.meta()
    }

    pub fn meta(&self) -> Result<ChainMeta> {
        let meta = self.meta_tree()?;

        let chain_id = read_string(&meta, "chain_id")?.unwrap_or_else(|| "tally-free-dev".to_string());
        let height = read_u64(&meta, "height")?.unwrap_or(0);
        let head_hash = read_hash(&meta, "head_hash")?.unwrap_or([0u8; 32]);
        let genesis_hash = read_hash(&meta, "genesis_hash")?.unwrap_or([0u8; 32]);

        Ok(ChainMeta {
            chain_id,
            height,
            head_hash,
            genesis_hash,
        })
    }

    pub fn snapshot(&self) -> Result<StateSnapshot> {
        let mut out = StateSnapshot::default();

        for row in self.balances_tree()?.iter() {
            let (k, v) = row?;
            out.balances
                .insert(account_from_key(&k)?, bincode::deserialize(&v)?);
        }

        for row in self.stake_tree()?.iter() {
            let (k, v) = row?;
            out.stake
                .insert(account_from_key(&k)?, bincode::deserialize(&v)?);
        }

        for row in self.admins_tree()?.iter() {
            let (k, _) = row?;
            out.admins.insert(account_from_key(&k)?);
        }

        for row in self.nonces_tree()?.iter() {
            let (k, v) = row?;
            out.nonces
                .insert(account_from_key(&k)?, u64::from_be_bytes(v.as_ref().try_into()?));
        }

        Ok(out)
    }

    pub fn balance_of(&self, who: &AccountId) -> Result<Amount> {
        Ok(self
            .balances_tree()?
            .get(who.0.to_vec())?
            .map(|v| bincode::deserialize(&v))
            .transpose()?
            .unwrap_or_else(Amount::zero))
    }

    pub fn nonce_of(&self, who: &AccountId) -> Result<u64> {
        Ok(self
            .nonces_tree()?
            .get(who.0.to_vec())?
            .map(|v| u64::from_be_bytes(v.as_ref().try_into().expect("nonce length")))
            .unwrap_or(0))
    }

    pub fn stake_of(&self, who: &AccountId) -> Result<Amount> {
        Ok(self
            .stake_tree()?
            .get(who.0.to_vec())?
            .map(|v| bincode::deserialize(&v))
            .transpose()?
            .unwrap_or_else(Amount::zero))
    }

    pub fn state_root(&self) -> Result<Hash32> {
        let ss = self.snapshot()?;
        Ok(state_root_from_snapshot(&ss))
    }

    pub fn block_by_height(&self, height: u64) -> Result<Option<Block>> {
        self.blocks_height_tree()?
            .get(height_key(height))?
            .map(|v| bincode::deserialize(&v).map_err(Into::into))
            .transpose()
    }

    pub fn block_by_hash(&self, hash: Hash32) -> Result<Option<Block>> {
        self.blocks_hash_tree()?
            .get(hash.to_vec())?
            .map(|v| bincode::deserialize(&v).map_err(Into::into))
            .transpose()
    }

    pub fn canonical_hash_at_height(&self, height: u64) -> Result<Option<Hash32>> {
        self.canon_tree()?
            .get(height_key(height))?
            .map(|v| ivec_to_hash(&v))
            .transpose()
    }

    pub fn iter_blocks(&self, from: u64, to: u64) -> Result<Vec<Block>> {
        let mut out = Vec::new();

        for h in from..=to {
            if let Some(block) = self.block_by_height(h)? {
                out.push(block);
            }
        }

        Ok(out)
    }

    pub fn stage_proposal(&self, block: &Block) -> Result<Hash32> {
        self.validate_block_shape(block)?;

        let h = hash_block(block);

        self.proposals_tree()?
            .insert(h.to_vec(), bincode::serialize(block)?)?;

        Ok(h)
    }

    pub fn staged_proposal(&self, hash: Hash32) -> Result<Option<Block>> {
        self.proposals_tree()?
            .get(hash.to_vec())?
            .map(|v| bincode::deserialize(&v).map_err(Into::into))
            .transpose()
    }

    pub fn is_initialized(&self) -> Result<bool> {
        Ok(self.meta_tree()?.get("height")?.is_some())
    }

    pub fn install_chain_from_blocks(&self, blocks: Vec<Block>) -> Result<ChainMeta> {
        ensure!(!blocks.is_empty(), "cannot install empty chain");

        let genesis = &blocks[0];

        ensure!(genesis.header.height == 0, "first synced block must be genesis");
        ensure!(
            genesis.header.parent_hash == [0u8; 32],
            "genesis parent must be zero"
        );
        ensure!(genesis.txs.is_empty(), "genesis block must not contain txs");

        // Reset local state before installing the received chain.
        self.blocks_height_tree()?.clear()?;
        self.blocks_hash_tree()?.clear()?;
        self.canon_tree()?.clear()?;
        self.proposals_tree()?.clear()?;
        self.balances_tree()?.clear()?;
        self.stake_tree()?.clear()?;
        self.admins_tree()?.clear()?;
        self.nonces_tree()?.clear()?;
        self.meta_tree()?.clear()?;

        let genesis_owner = genesis.header.proposer.clone();

        let mut snapshot = StateSnapshot::default();
        snapshot
            .balances
            .insert(genesis_owner.clone(), Amount::one_coin());
        snapshot.nonces.insert(genesis_owner.clone(), 0);
        snapshot.admins.insert(genesis_owner);

        let expected_genesis_state_root = state_root_from_snapshot(&snapshot);

        ensure!(
            genesis.header.state_root == expected_genesis_state_root,
            "synced genesis state root is invalid"
        );

        let genesis_hash = hash_block(genesis);

        self.write_snapshot(&snapshot)?;
        self.store_canonical_block(genesis, genesis_hash)?;

        let mut previous_hash = genesis_hash;
        let mut height = 0u64;

        for block in blocks.iter().skip(1) {
            ensure!(
                block.header.height == height + 1,
                "bad synced block height continuity"
            );
            ensure!(
                block.header.parent_hash == previous_hash,
                "bad synced parent hash"
            );
            ensure!(
                block.header.chain_id == genesis.header.chain_id,
                "synced block has wrong chain id"
            );

            self.validate_block_shape(block)?;

            let mut next_snapshot = self.snapshot()?;

            for tx in &block.txs {
                apply_tx_to_snapshot(&mut next_snapshot, tx)?;
            }

            ensure!(
                sum_balances(&next_snapshot) == *TOTAL_SUPPLY,
                "synced chain violates fixed supply"
            );

            let expected_state_root = state_root_from_snapshot(&next_snapshot);

            ensure!(
                block.header.state_root == expected_state_root,
                "synced block state root mismatch at height {}",
                block.header.height
            );

            self.write_snapshot(&next_snapshot)?;

            let block_hash = hash_block(block);
            self.store_canonical_block(block, block_hash)?;

            previous_hash = block_hash;
            height = block.header.height;
        }

        let meta = ChainMeta {
            chain_id: genesis.header.chain_id.clone(),
            height,
            head_hash: previous_hash,
            genesis_hash,
        };

        self.write_meta(&meta)?;
        self.db.flush()?;

        Ok(meta)
    }

    pub fn blocks_from_height(&self, from_height: u64, limit: u64) -> Result<Vec<Block>> {
        let meta = self.meta()?;
        let mut out = Vec::new();

        if from_height > meta.height {
            return Ok(out);
        }

        let last = std::cmp::min(meta.height, from_height.saturating_add(limit).saturating_sub(1));

        for height in from_height..=last {
            if let Some(block) = self.block_by_height(height)? {
                out.push(block);
            }
        }

        Ok(out)
    }
    pub fn commit_block(&self, block: &Block) -> Result<Hash32> {
        let meta = self.meta()?;

        ensure!(block.header.height == meta.height + 1, "bad height continuity");
        ensure!(block.header.parent_hash == meta.head_hash, "bad parent hash");
        ensure!(block.header.chain_id == meta.chain_id, "wrong chain id");

        self.validate_block_shape(block)?;

        let next_snapshot = self.apply_block_to_snapshot(block)?;

        ensure!(
            sum_balances(&next_snapshot) == *TOTAL_SUPPLY,
            "supply invariant broken"
        );

        let expected_root = state_root_from_snapshot(&next_snapshot);

        ensure!(
            block.header.state_root == expected_root,
            "state root mismatch"
        );

        self.write_snapshot(&next_snapshot)?;

        let block_hash = hash_block(block);

        self.store_canonical_block(block, block_hash)?;

        self.write_meta(&ChainMeta {
            chain_id: meta.chain_id,
            height: block.header.height,
            head_hash: block_hash,
            genesis_hash: meta.genesis_hash,
        })?;

        self.proposals_tree()?.remove(block_hash.to_vec())?;
        self.db.flush()?;

        Ok(block_hash)
    }

    pub fn build_unsigned_block(
        &self,
        proposer: AccountId,
        period: u64,
        txs: Vec<SignedTx>,
        capacity: BlockCapacity,
    ) -> Result<Block> {
        let meta = self.meta()?;
        let tx_root_value = tx_root(&txs);

        let mut block = Block {
            header: BlockHeader {
                chain_id: meta.chain_id,
                height: meta.height + 1,
                parent_hash: meta.head_hash,
                tx_root: tx_root_value,
                state_root: [0u8; 32],
                proposer,
                period,
                timestamp_ms: now_ms(),
                capacity,
            },
            txs,
            proposer_sig: vec![],
        };

        self.validate_block_shape(&block)?;

        let snapshot = self.apply_block_to_snapshot(&block)?;
        block.header.state_root = state_root_from_snapshot(&snapshot);

        Ok(block)
    }

    pub fn validate_block_shape(&self, block: &Block) -> Result<()> {
        ensure!(block.header.tx_root == tx_root(&block.txs), "tx root mismatch");
        ensure!(
            block.txs.len() <= block.header.capacity.max_txs,
            "too many txs"
        );

        let block_bytes = bincode::serialize(block)?.len();

        ensure!(
            block_bytes <= block.header.capacity.max_bytes,
            "block too large"
        );

        let weight: u64 = block.txs.iter().map(|tx| tx_weight(tx).total).sum();

        ensure!(
            weight <= block.header.capacity.max_weight,
            "block weight too high"
        );

        Ok(())
    }

    pub fn verify_chain(&self) -> Result<VerifyReport> {
        let meta = self.meta()?;

        let mut errors = Vec::new();
        let mut prev = [0u8; 32];
        let mut checked = 0usize;

        for height in 0..=meta.height {
            let Some(block) = self.block_by_height(height)? else {
                errors.push(format!("missing block at height {height}"));
                continue;
            };

            let hash = hash_block(&block);

            if let Some(canon) = self.canonical_hash_at_height(height)? {
                if canon != hash {
                    errors.push(format!("canonical hash mismatch at height {height}"));
                }
            } else {
                errors.push(format!("missing canonical hash at height {height}"));
            }

            if height == 0 {
                if block.header.parent_hash != [0u8; 32] {
                    errors.push("genesis parent is not zero".to_string());
                }
            } else if block.header.parent_hash != prev {
                errors.push(format!("bad parent at height {height}"));
            }

            if block.header.tx_root != tx_root(&block.txs) {
                errors.push(format!("tx root mismatch at height {height}"));
            }

            prev = hash;
            checked += 1;
        }

        if prev != meta.head_hash {
            errors.push("head hash does not match last block".to_string());
        }

        Ok(VerifyReport {
            ok: errors.is_empty(),
            checked_blocks: checked,
            head_height: meta.height,
            head_hash: meta.head_hash,
            errors,
        })
    }

    pub fn export_chain(&self) -> Result<ChainExport> {
        let meta = self.meta()?;
        let blocks = self.iter_blocks(0, meta.height)?;

        Ok(ChainExport {
            magic: "TALLY_FREE_CHAIN".to_string(),
            version: 1,
            meta,
            blocks,
        })
    }

    pub fn export_json_file(&self, path: impl AsRef<Path>) -> Result<()> {
        let export = self.export_chain();
        fs::write(path, serde_json::to_vec_pretty(&export?)?)?;
        Ok(())
    }

    pub fn export_binary_file(&self, path: impl AsRef<Path>) -> Result<()> {
        let export = self.export_chain()?;
        fs::write(path, bincode::serialize(&export)?)?;
        Ok(())
    }

    pub fn export_encrypted_file(&self, path: impl AsRef<Path>, password: &str) -> Result<()> {
        let bytes = bincode::serialize(&self.export_chain()?)?;
        let enc = encrypt_export(&bytes, password)?;

        fs::write(path, serde_json::to_vec_pretty(&enc)?)?;

        Ok(())
    }

    fn apply_block_to_snapshot(&self, block: &Block) -> Result<StateSnapshot> {
        let mut ss = self.snapshot()?;

        for tx in &block.txs {
            apply_tx_to_snapshot(&mut ss, tx)?;
        }

        Ok(ss)
    }

    fn write_snapshot(&self, ss: &StateSnapshot) -> Result<()> {
        self.balances_tree()?.clear()?;
        self.stake_tree()?.clear()?;
        self.admins_tree()?.clear()?;
        self.nonces_tree()?.clear()?;

        for (acct, amt) in &ss.balances {
            self.balances_tree()?
                .insert(acct.0.to_vec(), bincode::serialize(amt)?)?;
        }

        for (acct, amt) in &ss.stake {
            self.stake_tree()?
                .insert(acct.0.to_vec(), bincode::serialize(amt)?)?;
        }

        for acct in &ss.admins {
            self.admins_tree()?.insert(acct.0.to_vec(), vec![1])?;
        }

        for (acct, nonce) in &ss.nonces {
            self.nonces_tree()?
                .insert(acct.0.to_vec(), nonce.to_be_bytes().to_vec())?;
        }

        Ok(())
    }

    fn store_canonical_block(&self, block: &Block, hash: Hash32) -> Result<()> {
        let bytes = bincode::serialize(block)?;

        self.blocks_height_tree()?
            .insert(height_key(block.header.height), bytes.clone())?;

        self.blocks_hash_tree()?.insert(hash.to_vec(), bytes)?;

        self.canon_tree()?
            .insert(height_key(block.header.height), hash.to_vec())?;

        Ok(())
    }

    fn write_meta(&self, meta: &ChainMeta) -> Result<()> {
        let tree = self.meta_tree()?;

        tree.insert("chain_id", meta.chain_id.as_bytes())?;
        tree.insert("height", meta.height.to_be_bytes().to_vec())?;
        tree.insert("head_hash", meta.head_hash.to_vec())?;
        tree.insert("genesis_hash", meta.genesis_hash.to_vec())?;

        Ok(())
    }

    fn meta_tree(&self) -> Result<Tree> {
        Ok(self.db.open_tree(TREE_META)?)
    }

    fn blocks_height_tree(&self) -> Result<Tree> {
        Ok(self.db.open_tree(TREE_BLOCKS_BY_HEIGHT)?)
    }

    fn blocks_hash_tree(&self) -> Result<Tree> {
        Ok(self.db.open_tree(TREE_BLOCKS_BY_HASH)?)
    }

    fn canon_tree(&self) -> Result<Tree> {
        Ok(self.db.open_tree(TREE_CANONICAL_HASH_BY_HEIGHT)?)
    }

    fn proposals_tree(&self) -> Result<Tree> {
        Ok(self.db.open_tree(TREE_PROPOSALS_BY_HASH)?)
    }

    fn balances_tree(&self) -> Result<Tree> {
        Ok(self.db.open_tree(TREE_BALANCES)?)
    }

    fn stake_tree(&self) -> Result<Tree> {
        Ok(self.db.open_tree(TREE_STAKE)?)
    }

    fn admins_tree(&self) -> Result<Tree> {
        Ok(self.db.open_tree(TREE_ADMINS)?)
    }

    fn nonces_tree(&self) -> Result<Tree> {
        Ok(self.db.open_tree(TREE_NONCES)?)
    }
}

fn apply_tx_to_snapshot(ss: &mut StateSnapshot, tx: &SignedTx) -> Result<()> {
    verify_signed_tx(ss, tx)?;

    match &tx.kind {
        TxKind::CreateAccount { new } => {
            ss.balances.entry(new.clone()).or_insert_with(Amount::zero);
            ss.nonces.entry(new.clone()).or_insert(0);
        }

        TxKind::Transfer { to, amount } => {
            ensure!(!amount.is_zero(), "transfer amount must be greater than zero");

            debit(&mut ss.balances, &tx.from, amount)?;
            credit(&mut ss.balances, to, amount);

            ss.nonces
                .entry(to.clone())
                .or_insert(0);
        }

        TxKind::Stake { amount } => {
            ensure!(!amount.is_zero(), "stake amount must be greater than zero");

            debit(&mut ss.balances, &tx.from, amount)?;
            credit(&mut ss.stake, &tx.from, amount);
        }

        TxKind::Unstake { amount } => {
            ensure!(!amount.is_zero(), "unstake amount must be greater than zero");

            debit(&mut ss.stake, &tx.from, amount)?;
            credit(&mut ss.balances, &tx.from, amount);
        }

        TxKind::AdminAction { .. } => {}

        TxKind::Join { .. } => {}
    }

    let current_nonce = ss.nonces.get(&tx.from).copied().unwrap_or(0);
    ensure!(tx.nonce == current_nonce, "bad nonce");
    ss.nonces.insert(tx.from.clone(), current_nonce + 1);

    ensure!(
        sum_balances(ss) == *TOTAL_SUPPLY,
        "supply invariant broken after tx"
    );

    Ok(())
}

fn verify_signed_tx(ss: &StateSnapshot, tx: &SignedTx) -> Result<()> {
    let expected_from = AccountId::from_public_key_bytes(&tx.public_key);

    ensure!(
        expected_from == tx.from,
        "public key does not match sender account"
    );

    let expected_nonce = ss.nonces.get(&tx.from).copied().unwrap_or(0);

    ensure!(
        tx.nonce == expected_nonce,
        "bad nonce: expected {}, got {}",
        expected_nonce,
        tx.nonce
    );

    let vk_bytes: [u8; 32] = tx
        .public_key
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("bad public key length"))?;

    let vk = VerifyingKey::from_bytes(&vk_bytes)?;

    let sig = Signature::from_slice(&tx.sig).map_err(|_| anyhow!("bad signature bytes"))?;

    vk.verify(&tx.signing_bytes(), &sig)
        .map_err(|_| anyhow!("bad transaction signature"))?;

    Ok(())
}

fn debit(map: &mut BTreeMap<AccountId, Amount>, who: &AccountId, amount: &Amount) -> Result<()> {
    let current = map.get(who).cloned().unwrap_or_else(Amount::zero);

    let next = current
        .checked_sub(amount)
        .ok_or_else(|| anyhow!("insufficient funds"))?;

    map.insert(who.clone(), next);

    Ok(())
}

fn credit(map: &mut BTreeMap<AccountId, Amount>, who: &AccountId, amount: &Amount) {
    let current = map.get(who).cloned().unwrap_or_else(Amount::zero);

    let next = current
        .checked_add(amount)
        .expect("BigUint cannot overflow");

    map.insert(who.clone(), next);
}

fn sum_balances(ss: &StateSnapshot) -> BigUint {
    ss.balances
        .values()
        .fold(BigUint::zero(), |acc, amount| acc + &amount.0)
}

fn state_root_from_snapshot(ss: &StateSnapshot) -> Hash32 {
    let mut leaves = Vec::new();

    for (acct, amount) in &ss.balances {
        leaves.push(hash_bincode(&(b"balance", acct, amount)));
    }

    for (acct, amount) in &ss.stake {
        leaves.push(hash_bincode(&(b"stake", acct, amount)));
    }

    for acct in &ss.admins {
        leaves.push(hash_bincode(&(b"admin", acct)));
    }

    for (acct, nonce) in &ss.nonces {
        leaves.push(hash_bincode(&(b"nonce", acct, nonce)));
    }

    merkle_root(leaves)
}

#[derive(Debug, Serialize, Deserialize)]
struct EncryptedExport {
    magic: String,
    version: u32,
    kdf: String,
    cipher: String,
    salt: Vec<u8>,
    nonce: Vec<u8>,
    ciphertext: Vec<u8>,
}

fn encrypt_export(bytes: &[u8], password: &str) -> Result<EncryptedExport> {
    let mut salt = [0u8; 16];
    let mut nonce = [0u8; 12];

    OsRng.fill_bytes(&mut salt);
    OsRng.fill_bytes(&mut nonce);

    let mut key = [0u8; 32];

    Argon2::default()
        .hash_password_into(password.as_bytes(), &salt, &mut key)
        .map_err(|e| anyhow!("argon2: {e}"))?;

    let cipher = ChaCha20Poly1305::new((&key).into());

    let ciphertext = cipher
        .encrypt(chacha20poly1305::Nonce::from_slice(&nonce), bytes)
        .map_err(|e| anyhow!("encrypt: {e}"))?;

    Ok(EncryptedExport {
        magic: "TALLY_FREE_CHAIN_ENCRYPTED".to_string(),
        version: 1,
        kdf: "argon2id-default".to_string(),
        cipher: "chacha20poly1305".to_string(),
        salt: salt.to_vec(),
        nonce: nonce.to_vec(),
        ciphertext,
    })
}

fn height_key(height: u64) -> Vec<u8> {
    height.to_be_bytes().to_vec()
}

fn account_from_key(k: &[u8]) -> Result<AccountId> {
    ensure!(k.len() == 32, "bad account key length");

    let mut out = [0u8; 32];
    out.copy_from_slice(k);

    Ok(AccountId(out))
}

fn ivec_to_hash(v: &[u8]) -> Result<Hash32> {
    ensure!(v.len() == 32, "bad hash length");

    let mut out = [0u8; 32];
    out.copy_from_slice(v);

    Ok(out)
}

fn read_u64(tree: &Tree, key: &str) -> Result<Option<u64>> {
    Ok(tree
        .get(key)?
        .map(|v| u64::from_be_bytes(v.as_ref().try_into().unwrap())))
}

fn read_hash(tree: &Tree, key: &str) -> Result<Option<Hash32>> {
    tree.get(key)?.map(|v| ivec_to_hash(&v)).transpose()
}

fn read_string(tree: &Tree, key: &str) -> Result<Option<String>> {
    Ok(tree
        .get(key)?
        .map(|v| String::from_utf8(v.to_vec()))
        .transpose()?)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn genesis_and_verify() {
        let db = sled::Config::new().temporary(true).open().unwrap();
        let ledger = Ledger::from_db(db).unwrap();

        let owner = AccountId([7u8; 32]);

        ledger.init_genesis("test", owner).unwrap();

        let report = ledger.verify_chain().unwrap();

        assert!(report.ok, "{:?}", report.errors);
        assert_eq!(report.head_height, 0);
    }
}