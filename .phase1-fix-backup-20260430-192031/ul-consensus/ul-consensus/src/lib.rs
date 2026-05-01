use anyhow::{ensure, Result};
use num_bigint::BigUint;
use num_traits::Zero;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};
use ul_ledger::Ledger;
use ul_types::*;

pub const THRESHOLD_BP: u64 = 7_200;
pub const BASIS_POINTS: u64 = 10_000;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PeriodPhase {
    CollectTx,
    ProposeBlock,
    VoteBlock,
    CommitBlock,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct PeriodConfig {
    pub collect_ms: u64,
    pub propose_ms: u64,
    pub vote_ms: u64,
    pub commit_ms: u64,
    pub capacity: BlockCapacity,
}

impl Default for PeriodConfig {
    fn default() -> Self {
        Self {
            collect_ms: 5_000,
            propose_ms: 1_000,
            vote_ms: 4_000,
            commit_ms: 1_000,
            capacity: BlockCapacity::default(),
        }
    }
}

impl PeriodConfig {
    pub fn period_ms(&self) -> u64 {
        self.collect_ms + self.propose_ms + self.vote_ms + self.commit_ms
    }

    pub fn phase_at_elapsed(&self, elapsed_ms: u64) -> PeriodPhase {
        if elapsed_ms < self.collect_ms {
            PeriodPhase::CollectTx
        } else if elapsed_ms < self.collect_ms + self.propose_ms {
            PeriodPhase::ProposeBlock
        } else if elapsed_ms < self.collect_ms + self.propose_ms + self.vote_ms {
            PeriodPhase::VoteBlock
        } else {
            PeriodPhase::CommitBlock
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct PeriodClock {
    pub genesis_unix_ms: u64,
    pub config: PeriodConfig,
}

impl PeriodClock {
    pub fn new_now(config: PeriodConfig) -> Self {
        Self { genesis_unix_ms: now_ms(), config }
    }

    pub fn period_and_phase(&self, unix_ms: u64) -> (u64, PeriodPhase) {
        let elapsed = unix_ms.saturating_sub(self.genesis_unix_ms);
        let period = elapsed / self.config.period_ms();
        let inside = elapsed % self.config.period_ms();
        (period, self.config.phase_at_elapsed(inside))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorSet {
    pub total_stake: BigUint,
    pub weights: BTreeMap<AccountId, BigUint>,
}

impl ValidatorSet {
    pub fn from_ledger(ledger: &Ledger) -> Result<Self> {
        let ss = ledger.snapshot()?;
        let mut weights = BTreeMap::new();
        let mut total = BigUint::zero();
        for (account, amount) in ss.stake {
            if amount.0 > BigUint::zero() {
                total += amount.0.clone();
                weights.insert(account, amount.0);
            }
        }
        Ok(Self { total_stake: total, weights })
    }

    pub fn weight_of(&self, who: &AccountId) -> BigUint {
        self.weights.get(who).cloned().unwrap_or_else(BigUint::zero)
    }

    pub fn has_supermajority(&self, weight: &BigUint) -> bool {
        if self.total_stake.is_zero() {
            return false;
        }
        weight * BigUint::from(BASIS_POINTS) >= &self.total_stake * BigUint::from(THRESHOLD_BP)
    }

    pub fn deterministic_proposer(&self, period: u64) -> Option<AccountId> {
        if self.weights.is_empty() {
            return None;
        }
        let mut candidates: Vec<_> = self.weights.keys().cloned().collect();
        candidates.sort();
        let seed = hash_bincode(&(b"proposer", period));
        let idx = u64::from_be_bytes(seed[0..8].try_into().unwrap()) as usize % candidates.len();
        Some(candidates[idx].clone())
    }
}

#[derive(Debug, Clone)]
pub struct QuorumTracker {
    pub height: u64,
    pub block_hash: Hash32,
    pub yes_weight: BigUint,
    pub no_weight: BigUint,
    pub voters: BTreeSet<AccountId>,
    pub votes: Vec<Vote>,
}

impl QuorumTracker {
    pub fn new(height: u64, block_hash: Hash32) -> Self {
        Self {
            height,
            block_hash,
            yes_weight: BigUint::zero(),
            no_weight: BigUint::zero(),
            voters: BTreeSet::new(),
            votes: Vec::new(),
        }
    }

    pub fn add_vote(&mut self, validators: &ValidatorSet, vote: Vote) -> Result<bool> {
        ensure!(vote.height == self.height, "vote height mismatch");
        ensure!(vote.block_hash == self.block_hash, "vote hash mismatch");
        ensure!(self.voters.insert(vote.voter.clone()), "duplicate vote");
        let local_weight = validators.weight_of(&vote.voter);
        ensure!(local_weight == vote.stake_units, "declared stake does not match local validator set");
        if vote.accept {
            self.yes_weight += local_weight;
        } else {
            self.no_weight += local_weight;
        }
        self.votes.push(vote);
        Ok(validators.has_supermajority(&self.yes_weight))
    }

    pub fn certificate(&self) -> CommitCertificate {
        CommitCertificate {
            height: self.height,
            block_hash: self.block_hash,
            votes: self.votes.iter().filter(|v| v.accept).cloned().collect(),
        }
    }
}

pub fn validate_proposal(ledger: &Ledger, block: &Block, validators: &ValidatorSet) -> Result<()> {
    let meta = ledger.meta()?;
    ensure!(block.header.height == meta.height + 1, "proposal height does not extend head");
    ensure!(block.header.parent_hash == meta.head_hash, "proposal parent does not match head");
    ensure!(validators.weights.contains_key(&block.header.proposer), "proposer is not a validator");
    ledger.validate_block_shape(block)?;
    Ok(())
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
    fn phases_advance() {
        let cfg = PeriodConfig { collect_ms: 10, propose_ms: 10, vote_ms: 10, commit_ms: 10, capacity: BlockCapacity::default() };
        assert_eq!(cfg.phase_at_elapsed(0), PeriodPhase::CollectTx);
        assert_eq!(cfg.phase_at_elapsed(10), PeriodPhase::ProposeBlock);
        assert_eq!(cfg.phase_at_elapsed(20), PeriodPhase::VoteBlock);
        assert_eq!(cfg.phase_at_elapsed(30), PeriodPhase::CommitBlock);
    }
}
