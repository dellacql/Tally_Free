use anyhow::{ensure, Result};
use num_bigint::BigUint;
use num_traits::Zero;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};

pub const SCALE_POW: u32 = 45;
pub static SCALE: Lazy<BigUint> = Lazy::new(|| BigUint::from(10u32).pow(SCALE_POW));
pub static TOTAL_SUPPLY: Lazy<BigUint> = Lazy::new(|| SCALE.clone());

pub type Hash32 = [u8; 32];
pub type SignatureBytes = Vec<u8>;
pub type PublicKeyBytes = Vec<u8>;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AccountId(pub [u8; 32]);

impl AccountId {
    pub fn from_public_key_bytes(vk: &[u8]) -> Self {
        Self(blake3::hash(vk).into())
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Amount(pub BigUint);

impl Amount {
    pub fn zero() -> Self {
        Self(BigUint::zero())
    }

    pub fn one_coin() -> Self {
        Self(TOTAL_SUPPLY.clone())
    }

    pub fn checked_add(&self, other: &Self) -> Option<Self> {
        Some(Self(&self.0 + &other.0))
    }

    pub fn checked_sub(&self, other: &Self) -> Option<Self> {
        if self.0 >= other.0 {
            Some(Self(&self.0 - &other.0))
        } else {
            None
        }
    }

    pub fn from_decimal_str(s: &str) -> Result<Self> {
        let s = s.trim();
        ensure!(!s.is_empty(), "empty amount");
        let parts: Vec<&str> = s.split('.').collect();
        ensure!(parts.len() <= 2, "bad decimal amount");

        let whole = parts[0].replace('_', "");
        let mut frac = if parts.len() == 2 {
            parts[1].replace('_', "")
        } else {
            String::new()
        };

        ensure!(whole.chars().all(|c| c.is_ascii_digit()), "bad whole digits");
        ensure!(frac.chars().all(|c| c.is_ascii_digit()), "bad fractional digits");
        ensure!(frac.len() <= SCALE_POW as usize, "too many fractional digits");

        while frac.len() < SCALE_POW as usize {
            frac.push('0');
        }

        let joined = format!("{whole}{frac}");
        let stripped = joined.trim_start_matches('0');
        let units = if stripped.is_empty() {
            BigUint::zero()
        } else {
            BigUint::parse_bytes(stripped.as_bytes(), 10).expect("digits were checked")
        };

        Ok(Self(units))
    }

    pub fn to_decimal_string(&self) -> String {
        let q = &self.0 / &*SCALE;
        let r = &self.0 % &*SCALE;
        let mut frac = r.to_string();
        if frac.len() < SCALE_POW as usize {
            frac = "0".repeat(SCALE_POW as usize - frac.len()) + &frac;
        }
        let frac_trim = frac.trim_end_matches('0');
        if frac_trim.is_empty() {
            q.to_string()
        } else {
            format!("{q}.{frac_trim}")
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TxKind {
    CreateAccount { new: AccountId },
    Transfer { to: AccountId, amount: Amount },
    Stake { amount: Amount },
    Unstake { amount: Amount },
    AdminAction { action_id: Hash32, approve: bool },
    Join { nonce: u64, difficulty: u32 },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SignedTx {
    pub from: AccountId,
    pub nonce: u64,
    pub kind: TxKind,
    pub public_key: PublicKeyBytes,
    pub sig: SignatureBytes,
    pub relay_node: Option<AccountId>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct TxWeight {
    pub bytes: u64,
    pub sig_checks: u64,
    pub state_reads: u64,
    pub state_writes: u64,
    pub total: u64,
}

impl TxWeight {
    pub fn new(bytes: u64, sig_checks: u64, state_reads: u64, state_writes: u64) -> Self {
        let total = bytes + (sig_checks * 50) + (state_reads * 5) + (state_writes * 20);
        Self { bytes, sig_checks, state_reads, state_writes, total }
    }
}

pub fn tx_weight(tx: &SignedTx) -> TxWeight {
    let bytes = bincode::serialize(tx).map(|b| b.len() as u64).unwrap_or(0);
    match tx.kind {
        TxKind::CreateAccount { .. } => TxWeight::new(bytes, 1, 1, 1),
        TxKind::Transfer { .. } => TxWeight::new(bytes, 1, 2, 2),
        TxKind::Stake { .. } => TxWeight::new(bytes, 1, 2, 2),
        TxKind::Unstake { .. } => TxWeight::new(bytes, 1, 2, 2),
        TxKind::AdminAction { .. } => TxWeight::new(bytes, 1, 3, 2),
        TxKind::Join { .. } => TxWeight::new(bytes, 1, 1, 1),
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlockCapacity {
    pub max_txs: usize,
    pub max_bytes: usize,
    pub max_weight: u64,
}

impl Default for BlockCapacity {
    fn default() -> Self {
        Self {
            max_txs: 2_000,
            max_bytes: 2 * 1024 * 1024,
            max_weight: 500_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlockHeader {
    pub chain_id: String,
    pub height: u64,
    pub parent_hash: Hash32,
    pub tx_root: Hash32,
    pub state_root: Hash32,
    pub proposer: AccountId,
    pub period: u64,
    pub timestamp_ms: u64,
    pub capacity: BlockCapacity,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Block {
    pub header: BlockHeader,
    pub txs: Vec<SignedTx>,
    pub proposer_sig: SignatureBytes,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Vote {
    pub voter: AccountId,
    pub height: u64,
    pub block_hash: Hash32,
    pub accept: bool,
    pub stake_units: BigUint,
    pub public_key: PublicKeyBytes,
    pub sig: SignatureBytes,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommitCertificate {
    pub height: u64,
    pub block_hash: Hash32,
    pub votes: Vec<Vote>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PeerCapacity {
    pub node_account: AccountId,
    pub sig_verify_per_sec: u64,
    pub tx_apply_per_sec: u64,
    pub block_read_mb_per_sec: u64,
    pub block_write_mb_per_sec: u64,
    pub recommended_max_block_weight: u64,
    pub observed_at_ms: u64,
    pub sig: SignatureBytes,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Wire {
    Tx(SignedTx),
    Proposal(Block),
    Vote(Vote),
    Commit(CommitCertificate),
    PeerCapacity(PeerCapacity),
    GetHeaders { from_height: u64, limit: u64 },
    Headers { headers: Vec<BlockHeader> },
    GetBlockByHeight { height: u64 },
    GetBlockByHash { hash: Hash32 },
    BlockResponse { block: Option<Block> },
}

pub fn hash_bytes(bytes: &[u8]) -> Hash32 {
    blake3::hash(bytes).into()
}

pub fn hash_bincode<T: Serialize>(value: &T) -> Hash32 {
    let bytes = bincode::serialize(value).expect("serializable protocol object");
    hash_bytes(&bytes)
}

pub fn hash_tx(tx: &SignedTx) -> Hash32 {
    hash_bincode(tx)
}

pub fn hash_header(header: &BlockHeader) -> Hash32 {
    hash_bincode(header)
}

pub fn hash_block(block: &Block) -> Hash32 {
    hash_header(&block.header)
}

pub fn merkle_root(mut leaves: Vec<Hash32>) -> Hash32 {
    if leaves.is_empty() {
        return [0u8; 32];
    }
    while leaves.len() > 1 {
        let mut next = Vec::with_capacity((leaves.len() + 1) / 2);
        for pair in leaves.chunks(2) {
            let h = if pair.len() == 2 {
                hash_bytes(&[pair[0].as_slice(), pair[1].as_slice()].concat())
            } else {
                pair[0]
            };
            next.push(h);
        }
        leaves = next;
    }
    leaves[0]
}

pub fn tx_root(txs: &[SignedTx]) -> Hash32 {
    merkle_root(txs.iter().map(hash_tx).collect())
}

pub fn hex_hash(hash: &Hash32) -> String {
    hex::encode(hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amount_roundtrip() {
        let a = Amount::from_decimal_str("0.25").unwrap();
        assert_eq!(a.to_decimal_string(), "0.25");
        let b = Amount::from_decimal_str("1.000").unwrap();
        assert_eq!(b.to_decimal_string(), "1");
    }
}
