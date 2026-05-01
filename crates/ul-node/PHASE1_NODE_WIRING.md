# Phase 1 `ul-node` wiring

Your current `ul-node/src/main.rs` mixes several concerns in one file: local CLI actions, mempool storage, block building, gossip handling, vote tracking, and commit storage.

Do not replace the whole node file blindly. Wire it in this order:

## 1. Add dependencies to `crates/ul-node/Cargo.toml`

```toml
ul_ledger = { package = "ul-ledger", path = "../ul-ledger" }
ul_mempool = { package = "ul-mempool", path = "../ul-mempool" }
ul_capacity = { package = "ul-capacity", path = "../ul-capacity" }
```

## 2. Replace old dev block printing

Old behavior stored only hash-as-value in the `blocks` tree on simplified commit. New behavior should read from `ul-ledger`:

```rust
use ul_ledger::Ledger;
use ul_types::{hash_block, hex_hash};

fn print_blocks(db_path: &str, last: usize) -> anyhow::Result<()> {
    let ledger = Ledger::open(db_path)?;
    let meta = ledger.meta()?;
    let from = if last == 0 || last as u64 > meta.height { 0 } else { meta.height + 1 - last as u64 };
    for block in ledger.iter_blocks(from, meta.height)? {
        let h = hash_block(&block);
        println!(
            "h={} hash={} parent={} txs={} state_root={}",
            block.header.height,
            hex_hash(&h),
            hex_hash(&block.header.parent_hash),
            block.txs.len(),
            hex_hash(&block.header.state_root),
        );
    }
    Ok(())
}
```

## 3. Add chain export commands

Add commands:

```rust
ChainShow { from: Option<u64>, to: Option<u64>, json: bool },
ChainExport { out: String, format: String, password: Option<String> },
ChainVerify,
Benchmark,
```

Implement:

```rust
fn chain_verify(db_path: &str) -> anyhow::Result<()> {
    let ledger = Ledger::open(db_path)?;
    let report = ledger.verify_chain()?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn chain_export(db_path: &str, out: &str, format: &str, password: Option<&str>) -> anyhow::Result<()> {
    let ledger = Ledger::open(db_path)?;
    match (format, password) {
        ("json", _) => ledger.export_json_file(out)?,
        ("binary", _) => ledger.export_binary_file(out)?,
        ("encrypted", Some(pw)) => ledger.export_encrypted_file(out, pw)?,
        ("encrypted", None) => anyhow::bail!("encrypted export requires password"),
        _ => anyhow::bail!("format must be json, binary, or encrypted"),
    }
    println!("exported chain to {out}");
    Ok(())
}
```

## 4. Use `ul-mempool` for block creation

```rust
let db = sled::open(&cli.db)?;
let mempool = ul_mempool::Mempool::open(&db)?;
let selected = mempool.select_for_block(BlockCapacity::default())?;
let block = ledger.build_unsigned_block(my_account, period, selected.txs, BlockCapacity::default())?;
```

## 5. Use `ul-consensus` for periods and votes

```rust
let clock = ul_consensus::PeriodClock::new_now(ul_consensus::PeriodConfig::default());
let (period, phase) = clock.period_and_phase(current_unix_ms);
```

## 6. Use `ul-capacity` for validator admission

```rust
let report = ul_capacity::run_capacity_benchmark(my_account, ul_capacity::CapacityPolicy::default())?;
println!("{}", serde_json::to_string_pretty(&report)?);
```
