use std::collections::BTreeMap;
use tiny_keccak::{Hasher, Keccak};
use rlp::RlpStream;

/// Compute a binary Merkle root over a list of 32‑byte hashes.
/// If there is an odd number of leaves, duplicate the last hash at each level.
/// Keccak‑256 is used for hashing the concatenated children.
fn merkle_root(mut leaves: Vec<[u8; 32]>) -> [u8; 32] {
    if leaves.is_empty() {
        return [0u8; 32];
    }
    while leaves.len() > 1 {
        let mut next = Vec::with_capacity((leaves.len() + 1) / 2);
        for pair in leaves.chunks(2) {
            // Concatenate the two hashes (duplicate the last if odd)
            let mut data = Vec::with_capacity(64);
            data.extend_from_slice(&pair[0]);
            if pair.len() == 2 {
                data.extend_from_slice(&pair[1]);
            } else {
                // Duplicate the last hash
                data.extend_from_slice(&pair[0]);
            }
            // Keccak256 hash
            let mut hasher = Keccak::v256();
            hasher.update(&data);
            let mut out = [0u8; 32];
            hasher.finalize(&mut out);
            next.push(out);
        }
        leaves = next;
    }
    leaves[0]
}

/// Build a simple “state root” by hashing each account ID and its encoded state,
/// then computing the Merkle root of those hashes.  Returns the root hash.
///
/// Accounts must implement `BalanceGetter` so their balance/stake fields can be
/// encoded.  Adjust this trait or the RLP encoding as needed to match your
/// account structure.
pub fn build_state_trie<T: BalanceGetter>(
    accounts: &BTreeMap<[u8; 32], T>,
) -> [u8; 32] {
    // Collect leaf hashes: hash(key || rlp(account))
    let mut leaves: Vec<[u8; 32]> = Vec::with_capacity(accounts.len());
    for (addr, account) in accounts {
        // RLP‑encode the account (balance + stake)
        let mut stream = RlpStream::new_list(2);
        let (bal, stake) = account.get_balance_and_stake();
        stream.append(&bal);
        stream.append(&stake);
        let encoded = stream.out();
        // Hash the key/value pair using Keccak‑256
        let mut hasher = Keccak::v256();
        hasher.update(addr);
        hasher.update(&encoded);
        let mut leaf = [0u8; 32];
        hasher.finalize(&mut leaf);
        leaves.push(leaf);
    }
    // The Merkle root of all leaf hashes
    merkle_root(leaves)
}

/// A simple trait for extracting the fields needed to encode an account into RLP.
/// Implement this for your account struct wherever that struct is defined.
/// For example:
///
/// ```rust
/// use num_traits::ToPrimitive;
/// use ul_state::mpt::BalanceGetter;
///
/// impl BalanceGetter for Account {
///     fn get_balance_and_stake(&self) -> (u128, u128) {
///         (
///             self.balance1e45.to_u128().unwrap_or(0),
///             self.stake1e45.to_u128().unwrap_or(0),
///         )
///     }
/// }
/// ```
pub trait BalanceGetter {
    /// Returns (balance, stake) as two u128 values.  Adjust this to match your
    /// account structure (e.g. BigUint or other numeric type).
    fn get_balance_and_stake(&self) -> (u128, u128);
}
