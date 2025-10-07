use anyhow::{Result, ensure, anyhow};
use ul_types::*;
use sled::Db;
use std::collections::{BTreeMap, BTreeSet};
use num_bigint::BigUint;
use num_traits::Zero;

pub struct ChainState { db: Db }

#[derive(Clone, Default)]
pub struct Snapshot {
    pub balances: BTreeMap<AccountId, Amount>,
    pub stake:    BTreeMap<AccountId, Amount>,
    pub admins:   BTreeSet<AccountId>,
}

impl ChainState {
    pub fn open(path: &str) -> Result<Self> { Ok(Self { db: sled::open(path)? }) }

    pub fn init_genesis(&self, owner: AccountId) -> Result<()> {
        if self.db.open_tree("meta")?.get("height")?.is_some() { return Ok(()); }
        self.db.open_tree("stake")?;
        let bal = self.db.open_tree("bal")?;
        bal.insert(bincode::serialize(&owner)?, bincode::serialize(&Amount(TOTAL_SUPPLY.clone()))?)?;
        let admins = self.db.open_tree("admins")?;
        admins.insert(bincode::serialize(&owner)?, bincode::serialize(&true)?)?;
        self.set_height_parent(0, [0u8;32])?;
        Ok(())
    }

    pub fn get_snapshot(&self) -> Result<Snapshot> {
        let mut ss = Snapshot::default();
        for kv in self.db.open_tree("bal")?.iter() {
            let (k, v) = kv?;
            ss.balances.insert(bincode::deserialize(&k)?, bincode::deserialize(&v)?);
        }
        for kv in self.db.open_tree("stake")?.iter() {
            let (k, v) = kv?;
            ss.stake.insert(bincode::deserialize(&k)?, bincode::deserialize(&v)?);
        }
        for kv in self.db.open_tree("admins")?.iter() {
            let (k, _v) = kv?;
            ss.admins.insert(bincode::deserialize(&k)?);
        }
        Ok(ss)
    }

    pub fn state_root(&self) -> Result<[u8;32]> {
        let ss = self.get_snapshot()?;
        let mut leaves = Vec::<[u8;32]>::new();
        for (a, amt) in ss.balances.iter() {
            leaves.push(blake3::hash(&bincode::serialize(&(a, &amt.0))?).into());
        }
        for (a, st) in ss.stake.iter() {
            leaves.push(blake3::hash(&bincode::serialize(&(a, &st.0, true))?).into());
        }
        if leaves.is_empty() { return Ok([0u8;32]); }
        Ok(merkle_root(leaves))
    }

    pub fn apply_block(&self, b: &Block) -> Result<()> {
        for tx in &b.txs { self.apply_tx(tx)?; }
        let tot = self.total_balance_units()?;
        ensure!(tot == *TOTAL_SUPPLY, "supply changed");
        let blocks = self.db.open_tree("blocks")?;
        blocks.insert(b.header.height.to_be_bytes(), bincode::serialize(&b)?)?;
        self.set_height_parent(b.header.height, b.header.parent)?;
        Ok(())
    }

    fn apply_tx(&self, tx: &SignedTx) -> Result<()> {
        use TxKind::*;
        match &tx.kind {
            CreateAccount { new } => {
                let bal = self.db.open_tree("bal")?;
                let k = bincode::serialize(new)?;
                if bal.get(&k)?.is_none() {
                    bal.insert(k, bincode::serialize(&Amount::zero())?)?;
                }
            }
            Transfer { to, amount } => {
                self.debit(&tx.from, &amount.0)?;
                self.credit(to, &amount.0)?;
            }
            Stake { amount } => {
                self.debit(&tx.from, &amount.0)?;
                self.add_stake_pending(&tx.from, &amount.0)?;
            }
            Unstake { amount } => {
                self.remove_stake_pending(&tx.from, &amount.0)?;
            }
            AdminAction { .. } => {}
            Join { .. } => {}
        }
        Ok(())
    }

    fn debit(&self, who: &AccountId, units: &BigUint) -> Result<()> {
        let bal = self.db.open_tree("bal")?;
        let k = bincode::serialize(who)?;
        let cur = bal.get(&k)?
            .map(|v| bincode::deserialize::<Amount>(&v).unwrap())
            .unwrap_or(Amount::zero());
        let new = cur.checked_sub(&Amount(units.clone()))
            .ok_or_else(|| anyhow!("insufficient funds"))?;
        bal.insert(k, bincode::serialize(&new)?)?; Ok(())
    }

    fn credit(&self, who: &AccountId, units: &BigUint) -> Result<()> {
        let bal = self.db.open_tree("bal")?;
        let k = bincode::serialize(who)?;
        let cur = bal.get(&k)?
            .map(|v| bincode::deserialize::<Amount>(&v).unwrap())
            .unwrap_or(Amount::zero());
        let new = cur.checked_add(&Amount(units.clone())).unwrap();
        bal.insert(k, bincode::serialize(&new)?)?; Ok(())
    }

    fn add_stake_pending(&self, who: &AccountId, units: &BigUint) -> Result<()> {
        let st = self.db.open_tree("stakepending")?;
        let k = bincode::serialize(who)?;
        let cur = st.get(&k)?.map(|v| bincode::deserialize::<Amount>(&v).unwrap()).unwrap_or(Amount::zero());
        st.insert(k, bincode::serialize(&Amount(cur.0 + units))?)?; Ok(())
    }
    fn remove_stake_pending(&self, who: &AccountId, units: &BigUint) -> Result<()> {
        let st = self.db.open_tree("stakepending")?;
        let k = bincode::serialize(who)?;
        let cur = st.get(&k)?.map(|v| bincode::deserialize::<Amount>(&v).unwrap()).unwrap_or(Amount::zero());
        ensure!(cur.0 >= *units, "not enough pending stake");
        st.insert(k, bincode::serialize(&Amount(cur.0 - units))?)?; Ok(())
    }

    fn total_balance_units(&self) -> Result<BigUint> {
        let mut sum = BigUint::zero();
        for kv in self.db.open_tree("bal")?.iter() {
            let (_k, v) = kv?;
            let a: Amount = bincode::deserialize(&v)?;
            sum += a.0;
        }
        Ok(sum)
    }

    fn set_height_parent(&self, h: u64, parent: [u8;32]) -> Result<()> {
        let meta = self.db.open_tree("meta")?;
        meta.insert("height", h.to_be_bytes().to_vec())?;
        meta.insert("parent", parent.to_vec())?;
        Ok(())
    }
}

fn merkle_root(mut leaves: Vec<[u8;32]>) -> [u8;32] {
    if leaves.len() == 1 { return leaves[0]; }
    while leaves.len() > 1 {
        let mut next = Vec::with_capacity((leaves.len()+1)/2);
        for pair in leaves.chunks(2) {
            let h = if pair.len() == 2 {
                blake3::hash(&[pair[0].as_slice(), pair[1].as_slice()].concat()).into()
            } else { pair[0] };
            next.push(h);
        }
        leaves = next;
    }
    leaves[0]
}
