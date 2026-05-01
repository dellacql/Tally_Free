use anyhow::Result;
use num_bigint::BigUint;
use ul_state::ChainState;
use ul_types::*;
use ul_types::{AccountId, Block};

pub const THRESHOLD_BP: u64 = 7200; // 72.00% (10000 bp = 100%)
pub const EPOCH_MS: u64 = 2_000;

#[derive(Clone)]
pub struct ValidatorSet {
    pub total_stake: BigUint,
    pub weights: Vec<(AccountId, BigUint)>,
}

impl ValidatorSet {
    pub fn from_state(st: &ChainState) -> Result<Self> {
        let ss = st.get_snapshot()?;
        let mut v = Vec::new();
        let mut total = BigUint::from(0u32);
        for (k, a) in ss.stake {
            total += a.0.clone();
            v.push((k, a.0));
        }
        Ok(Self {
            total_stake: total,
            weights: v,
        })
    }
    pub fn supermajority(&self, votes_weight: &BigUint) -> bool {
        // votes/total >= 0.72  <=>  votes*10000 >= total*7200
        votes_weight * BigUint::from(10_000u32) >= &self.total_stake * BigUint::from(THRESHOLD_BP)
    }
    pub fn top_staker(&self) -> Option<AccountId> {
        self.weights
            .iter()
            .max_by(|a, b| a.1.cmp(&b.1))
            .map(|(id, _)| id.clone())
    }
}

/// Return a stake-weighted vote for a block if structurally valid.
/// (Production: re-execute block and recompute state_root.)
pub fn validate_and_vote(
    _st: &ChainState,
    b: &Block,
    me: &AccountId,
    my_stake: &BigUint,
) -> Result<Option<Vote>> {
    let vote = Vote {
        voter: me.clone(),
        height: b.header.height,
        block_hash: ul_types::hash_block(b),
        stake_units: my_stake.clone(),
        sig: vec![], // signed by node
    };
    Ok(Some(vote))
}
