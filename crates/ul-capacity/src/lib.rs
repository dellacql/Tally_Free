use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use ul_types::{AccountId, PeerCapacity};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapacityPolicy {
    pub min_hashes_per_sec: u64,
    pub min_tx_apply_per_sec: u64,
    pub min_block_read_mb_per_sec: u64,
    pub min_block_write_mb_per_sec: u64,
    pub min_recommended_block_weight: u64,
}

impl Default for CapacityPolicy {
    fn default() -> Self {
        Self {
            min_hashes_per_sec: 50_000,
            min_tx_apply_per_sec: 10_000,
            min_block_read_mb_per_sec: 50,
            min_block_write_mb_per_sec: 20,
            min_recommended_block_weight: 100_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapacityReport {
    pub node_account: AccountId,
    pub hashes_per_sec: u64,
    pub synthetic_tx_apply_per_sec: u64,
    pub block_read_mb_per_sec: u64,
    pub block_write_mb_per_sec: u64,
    pub recommended_max_block_weight: u64,
    pub recommended_validator: bool,
    pub observed_at_ms: u64,
}

impl CapacityReport {
    pub fn to_peer_capacity_unsigned(&self) -> PeerCapacity {
        PeerCapacity {
            node_account: self.node_account.clone(),
            sig_verify_per_sec: self.hashes_per_sec,
            tx_apply_per_sec: self.synthetic_tx_apply_per_sec,
            block_read_mb_per_sec: self.block_read_mb_per_sec,
            block_write_mb_per_sec: self.block_write_mb_per_sec,
            recommended_max_block_weight: self.recommended_max_block_weight,
            observed_at_ms: self.observed_at_ms,
            sig: vec![],
        }
    }
}

pub fn run_capacity_benchmark(
    node_account: AccountId,
    policy: CapacityPolicy,
) -> Result<CapacityReport> {
    let hashes_per_sec = bench_hashes(Duration::from_millis(500));
    let synthetic_tx_apply_per_sec = hashes_per_sec / 5;

    // Conservative placeholders until disk benchmarks are wired into ul-node with a DB path.
    let block_read_mb_per_sec = 100;
    let block_write_mb_per_sec = 50;

    let recommended_max_block_weight = (synthetic_tx_apply_per_sec / 2).max(1_000);
    let recommended_validator = hashes_per_sec >= policy.min_hashes_per_sec
        && synthetic_tx_apply_per_sec >= policy.min_tx_apply_per_sec
        && block_read_mb_per_sec >= policy.min_block_read_mb_per_sec
        && block_write_mb_per_sec >= policy.min_block_write_mb_per_sec
        && recommended_max_block_weight >= policy.min_recommended_block_weight;

    Ok(CapacityReport {
        node_account,
        hashes_per_sec,
        synthetic_tx_apply_per_sec,
        block_read_mb_per_sec,
        block_write_mb_per_sec,
        recommended_max_block_weight,
        recommended_validator,
        observed_at_ms: now_ms(),
    })
}

fn bench_hashes(duration: Duration) -> u64 {
    let start = Instant::now();
    let mut count = 0u64;
    let mut buf = [0u8; 64];
    while start.elapsed() < duration {
        buf[0..8].copy_from_slice(&count.to_be_bytes());
        let h = blake3::hash(&buf);
        buf[8..40].copy_from_slice(h.as_bytes());
        count += 1;
    }
    let elapsed = start.elapsed().as_secs_f64();
    (count as f64 / elapsed) as u64
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
