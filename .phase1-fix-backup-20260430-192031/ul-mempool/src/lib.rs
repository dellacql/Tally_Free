use anyhow::{Result, ensure};
use sled::Tree;
use ul_types::*;

const TREE_MEMPOOL: &str = "mempool_v1";

#[derive(Clone)]
pub struct Mempool {
    tree: Tree,
}

#[derive(Debug, Clone)]
pub struct SelectedTxs {
    pub txs: Vec<SignedTx>,
    pub total_bytes: usize,
    pub total_weight: u64,
}

impl Mempool {
    pub fn open(db: &sled::Db) -> Result<Self> {
        Ok(Self {
            tree: db.open_tree(TREE_MEMPOOL)?,
        })
    }

    pub fn insert(&self, tx: &SignedTx) -> Result<Hash32> {
        validate_basic(tx)?;
        let id = hash_tx(tx);
        self.tree.insert(id.to_vec(), bincode::serialize(tx)?)?;
        Ok(id)
    }

    pub fn contains(&self, id: Hash32) -> Result<bool> {
        Ok(self.tree.contains_key(id.to_vec())?)
    }

    pub fn remove(&self, id: Hash32) -> Result<()> {
        self.tree.remove(id.to_vec())?;
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.tree.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tree.is_empty()
    }

    pub fn select_for_block(&self, capacity: BlockCapacity) -> Result<SelectedTxs> {
        let mut txs = Vec::new();
        let mut total_bytes = 0usize;
        let mut total_weight = 0u64;

        for row in self.tree.iter() {
            let (_, bytes) = row?;
            let tx: SignedTx = bincode::deserialize(&bytes)?;
            let weight = tx_weight(&tx);
            let next_bytes = total_bytes + bytes.len();
            let next_weight = total_weight + weight.total;

            if txs.len() + 1 > capacity.max_txs {
                break;
            }
            if next_bytes > capacity.max_bytes {
                break;
            }
            if next_weight > capacity.max_weight {
                break;
            }

            txs.push(tx);
            total_bytes = next_bytes;
            total_weight = next_weight;
        }

        Ok(SelectedTxs {
            txs,
            total_bytes,
            total_weight,
        })
    }

    pub fn remove_included(&self, txs: &[SignedTx]) -> Result<()> {
        for tx in txs {
            self.tree.remove(hash_tx(tx).to_vec())?;
        }
        Ok(())
    }
}

fn validate_basic(tx: &SignedTx) -> Result<()> {
    ensure!(
        AccountId::from_public_key_bytes(&tx.public_key) == tx.from,
        "from account does not match public key"
    );
    ensure!(!tx.sig.is_empty(), "missing tx signature");
    Ok(())
}
