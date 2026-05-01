use num_bigint::BigUint;
use num_traits::Zero;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};

pub const SCALE_POW: u32 = 45;
pub static SCALE: Lazy<BigUint> = Lazy::new(|| BigUint::from(10u32).pow(SCALE_POW));
pub static TOTAL_SUPPLY: Lazy<BigUint> = Lazy::new(|| SCALE.clone()); // 1.0 coin

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Amount(pub BigUint);

impl Amount {
    pub fn zero() -> Self {
        Self(BigUint::zero())
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
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AccountId(pub [u8; 32]);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TxKind {
    CreateAccount { new: AccountId },
    Transfer { to: AccountId, amount: Amount },
    Stake { amount: Amount },
    Unstake { amount: Amount },
    AdminAction { action_id: [u8; 32], approve: bool },
    Join { nonce: u64, difficulty: u32 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedTx {
    pub from: AccountId,
    pub kind: TxKind,
    pub sig: Vec<u8>, // ed25519
    pub relay_node: Option<AccountId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockHeader {
    pub parent: [u8; 32],
    pub height: u64,
    pub tx_root: [u8; 32],
    pub state_root: [u8; 32],
    pub proposer: AccountId,
    pub epoch_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Block {
    pub header: BlockHeader,
    pub txs: Vec<SignedTx>,
    pub proposer_sig: Vec<u8>, // ed25519 over header
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vote {
    pub voter: AccountId,
    pub height: u64,
    pub block_hash: [u8; 32],
    pub stake_units: BigUint, // snapshot
    pub sig: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Wire {
    Tx(SignedTx),
    Proposal(Block),
    Vote(Vote),
    Commit { height: u64, block_hash: [u8; 32] },
    EpochStart { height: u64, unix_ms: u64 },
}

pub fn hash_block(b: &Block) -> [u8; 32] {
    blake3::hash(&bincode::serialize(&b.header).unwrap()).into()
}
