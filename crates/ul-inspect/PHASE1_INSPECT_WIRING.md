# Phase 1 `ul-inspect` wiring

`ul-inspect` should become the read-only chain explorer.

Suggested commands:

```bash
ul-inspect --db ./n1 status
ul-inspect --db ./n1 balances --limit 100 --json
ul-inspect --db ./n1 blocks --from 0 --to latest --json
ul-inspect --db ./n1 verify-chain
```

Core code:

```rust
let ledger = ul_ledger::Ledger::open(&cli.db)?;
let meta = ledger.meta()?;
println!("height={} head={}", meta.height, ul_types::hex_hash(&meta.head_hash));
```
