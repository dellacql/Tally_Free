# Unity Ledger — Getting Started (Updated)

A minimal, fee-less blockchain prototype written in Rust.

- **Total supply:** fixed at **1.0** (no mint/burn), **45 decimal places**
- **Accounts:** created by **nodes** (wallets hold keys)
- **Transfers:** binary (A → B), **no fees**
- **Consensus:** stake-weighted, **≥72%** supermajority to commit blocks
- **Networking:** libp2p **QUIC + Gossipsub + Kademlia + AutoNAT + DCUtR**, optional **relay**
- **Security:** wallets encrypted with **Argon2id → ChaCha20-Poly1305**

> ⚠️ Learning prototype. Use for test networks only.

---

## Table of Contents

- [1. Install Rust](#1-install-rust)
- [2. Build](#2-build)
- [3. Create a Wallet (keystore) & Show Address](#3-create-a-wallet-keystore--show-address)
- [4. Initialize a Genesis Chain (dev)](#4-initialize-a-genesis-chain-dev)
- [5. Run a Node](#5-run-a-node)
- [6. (Optional) Run a Relay](#6-optional-run-a-relay)
- [7. Create an On-Chain Account](#7-create-an-on-chain-account)
- [8. Inspect Balances & Verify Supply = 1.0](#8-inspect-balances--verify-supply--10)
- [9. Secure Password Input (hidden)](#9-secure-password-input-hidden)
- [10. Troubleshooting](#10-troubleshooting)
- [11. Protocol Cheat-Sheet](#11-protocol-cheat-sheet)

---

## 1. Install Rust

All platforms: https://rustup.rs/

Verify:
```sh
rustc -V
cargo -V
```

---

## 2. Build
From the repo root:
```sh
cargo build --release
```

Binaries:
- `target/release/ul-wallet[.exe]`
- `target/release/ul-node[.exe]`
- `target/release/ul-relay[.exe]`
- `target/release/ul-inspect[.exe]`

---

## 3. Create a Wallet (keystore) & Show Address

A wallet is **your keys** (encrypted on disk). It’s not an on-chain account yet.

**Windows**
```cmd
target\release\ul-wallet.exe --keystore .\n1.json --password <YOUR_PASSWORD> new
target\release\ul-wallet.exe --keystore .\n1.json --password <YOUR_PASSWORD> address
```

**macOS/Linux**
```bash
./target/release/ul-wallet --keystore ./n1.json --password <"YOUR_PASSWORD"> new
./target/release/ul-wallet --keystore ./n1.json --password <"YOUR_PASSWORD"> address
```

Copy the printed **AccountId (hex32)**. Example:
```
875c9f77841e2c1696aa7afc773385ef9caa39d5dfba7ef57da0f82325fe0670
```

---

## 4. Initialize a Genesis Chain (dev)

Give the entire supply (1.0) to your wallet address, then write height/parent metadata.

**Windows**
```cmd
target\release\ul-node.exe --db .\n1 --keystore .\n1.json --password <YOUR_PASSWORD> genesis
```

**macOS/Linux**
```bash
./target/release/ul-node --db ./n1 --keystore ./n1.json --password <"YOUR_PASSWORD"> genesis
```

You should see:
```
ul-node starting; peer id: 12D3KooW...
wallet accountId (hex32): <YOUR_HEX32>
✔ genesis initialized at .
1
```

---

## 5. Run a Node

Run networking + consensus loop. Keep this terminal **open**.

**Windows**
```cmd
set RUST_LOG=info && target\release\ul-node.exe --db .\n1 --keystore .\n1.json --password "pw" --listen /ip4/0.0.0.0/udp/7001/quic-v1 run
```

**macOS/Linux**
```bash
RUST_LOG=info ./target/release/ul-node --db ./n1 --keystore ./n1.json --password "pw" --listen /ip4/0.0.0.0/udp/7001/quic-v1 run
```

---

## 6. (Optional) Run a Relay

Run in a **second** terminal (helps NATed peers connect):

**Windows**
```cmd
set RUST_LOG=info && target\release\ul-relay.exe
```

**macOS/Linux**
```bash
RUST_LOG=info ./target/release/ul-relay
```

Share the relay multiaddr by appending your PeerId:
```
/ip4/<IP>/udp/7000/quic-v1/p2p/<PEER_ID>
```

---

## 7. Create an On-Chain Account

> **Current behavior (safe stub):** `create-account` **logs the intent** (you’ll see a console message) but does not modify state yet. That’s by design to keep the sample stable. You can still **inspect existing balances** and your **genesis balance**.

**Windows**
```cmd
target\release\ul-node.exe --db .\n1 --keystore .\n1.json --password "pw" create-account --addr-hex32 <NEW_ACCOUNT_HEX32>
```

**macOS/Linux**
```bash
./target/release/ul-node --db ./n1 --keystore ./n1.json --password "pw" create-account --addr-hex32 <NEW_ACCOUNT_HEX32>
```

You should see:
```
create-account requested for <NEW_ACCOUNT_HEX32>
```

### DEV-ONLY local write (optional)
If you want `create-account` to **immediately** appear in your local DB for demos/tests, add this snippet to the `CreateAccount` branch of `crates/ul-node/src/main.rs` and rebuild:

```rust
// 1) parse hex
let acc = hex::decode(&args.addr_hex32)
    .map_err(|e| anyhow!("bad hex32 address: {e}"))?;
anyhow::ensure!(acc.len() == 32, "address must be 32 bytes");

// 2) insert zero balance locally
let db = sled::open(&cli.db)?;
let balances = db.open_tree("balances")?;
balances.insert(acc, Vec::<u8>::new())?;
db.flush()?;

println!("✔ created account locally in DB: {}", args.addr_hex32);
```

---

## 8. Inspect Balances & Verify Supply = 1.0

**Status**
```sh
# Windows
target\release\ul-inspect.exe --db .\n1 status
# macOS/Linux
./target/release/ul-inspect --db ./n1 status
```

**Find a specific account**
```cmd
target\release\ul-inspect.exe --db .\n1 balances --limit 0 --decode amount1e45 | findstr /i <ACCOUNT_HEX32>
```

**Exact supply check (sum of integer units = 10^45)**

*macOS/Linux (Python 3):*
```bash
./target/release/ul-inspect --db ./n1 balances --limit 0 --decode uint-be --json | python3 -c "import sys,json; rows=json.load(sys.stdin); s=sum(int(v[1]) for v in rows); print('units=',s); print('OK' if s==10**45 else 'NOT OK')"
```

*Windows (PowerShell):*
```powershell
$rows = & target\release\ul-inspect.exe --db .\n1 balances --limit 0 --decode uint-be --json | ConvertFrom-Json
$sum = [System.Numerics.BigInteger]::Zero
foreach ($r in $rows) { $sum += [System.Numerics.BigInteger]::Parse($r[1]) }
$target = [System.Numerics.BigInteger]::Pow(10,45)
"units=$sum"
if ($sum -eq $target) { "OK" } else { "NOT OK" }
```

---

## 9. Secure Password Input (hidden)

Right now examples pass `--password "pw"` for speed. For **real use**, prompt securely so the password is **not echoed** and **not visible** in history.

### Add dependency (both `ul-wallet` and `ul-node`)
`crates/ul-wallet/Cargo.toml` and `crates/ul-node/Cargo.toml`:
```toml
[dependencies]
rpassword = "7"
```

### Make `--password` optional and prompt if missing

**`ul-wallet/src/main.rs` (example)**
```rust
use rpassword::prompt_password;

#[derive(Parser)]
struct Cli {
    #[arg(long)]
    keystore: String,
    #[arg(long)]
    password: Option<String>,
    // ...
}

fn get_password(cli: &Cli) -> anyhow::Result<String> {
    if let Some(p) = &cli.password { return Ok(p.clone()); }
    let pw = prompt_password("Keystore password: ")?;
    Ok(pw)
}

// then use: let password = get_password(&cli)?;
```

**`ul-node/src/main.rs` (example)**
```rust
use rpassword::prompt_password;

#[derive(Parser)]
struct Cli {
    #[arg(long)]
    db: String,
    #[arg(long)]
    keystore: String,
    #[arg(long)]
    password: Option<String>,
    // ...
}

fn get_password(cli: &Cli) -> anyhow::Result<String> {
    if let Some(p) = &cli.password { return Ok(p.clone()); }
    let pw = prompt_password("Keystore password: ")?;
    Ok(pw)
}

// load wallet:
let pw = get_password(&cli)?;
let kp = ks::load(&cli.keystore, &pw)?;
```

**Usage after change**
```cmd
target\release\ul-wallet.exe --keystore .\n1.json new
# (prompt appears; typing is hidden)
```

You can still pass `--password "..."` in scripts/CI, but for humans it’s safer to omit and be prompted.

---

## 10. Troubleshooting

| Symptom / Log | Meaning | Fix |
|---|---|---|
| `The system cannot find the file specified (os error 2)` | Keystore file missing | Create it via `ul-wallet --keystore ... new` |
| EXE locked during build | Program still running | Ctrl+C or `taskkill /IM ul-node.exe /F` |
| No peers | NAT/firewall | Use a relay and/or open UDP 7000/7001 |
| Relay works but no `/p2p/<PeerId>` shown | Display tweak | Append `/p2p/<PeerId>` manually or modify the print in `ul-relay` |
| `create-account` doesn’t appear in balances | By design (skeleton) | Use **DEV-ONLY local write** or wait for full tx/consensus wiring |

---

## 11. Protocol Cheat-Sheet

- **Supply:** fixed 1.0 (internally 10^45 integer units).
- **Accounts:** created by nodes; wallets hold keys.
- **Transfers:** A → B only, **no fees**.
- **Stake:** lock balances to gain vote weight.
- **Consensus:** block commits at **≥72%** of active stake.
- **Epochs:** periodic rounds; top staker coordinates timing.
- **Networking:** QUIC, gossip for tx/proposals/votes/commits/epochs.
- **Safety:** on commit, nodes verify **∑ balances == 1.0**.


## Quick Commands (Windows)

### See your balance
```cmd
target\release\ul-node.exe --db .\n1 --keystore .\n1.json --password "pw" balance
```

### Create recipient
```cmd
target\release\ul-node.exe --db .\n1 --keystore .\n1.json --password "pw" create-account --addr-hex32 0b41e83c6ba339d92cbc1ce17b160ea5e93f6021eea14cbf33c7fb9368eff26d
```

### Transfer (no fees; dev-local apply)
```cmd
target\release\ul-node.exe --db .\n1 --keystore .\n1.json --password "pw" transfer --to-hex32 0b41e83c6ba339d92cbc1ce17b160ea5e93f6021eea14cbf33c7fb9368eff26d --amount 0.25
```

### Verify balances
```cmd
:: recipient ~0.25
target\release\ul-inspect.exe --db .\n1 balances --limit 0 --decode amount1e45 | findstr /i 0b41e8

:: sender ~0.75 (your 236adf... address)
target\release\ul-inspect.exe --db .\n1 balances --limit 0 --decode amount1e45 | findstr /i 236adf83
```

### See the blockchain (dev blocks)
```cmd
target\release\ul-node.exe --db .\n1 --keystore .\n1.json --password "pw" print-blocks --last 10
```

### Verify total supply = 1.0 (exact)
```powershell
$rows = & target\release\ul-inspect.exe --db .\n1 balances --limit 0 --decode uint-be --json | ConvertFrom-Json
$sum = [System.Numerics.BigInteger]::Zero
foreach ($r in $rows) { $sum += [System.Numerics.BigInteger]::Parse($r[1]) }
$target = [System.Numerics.BigInteger]::Pow(10,45)
"units=$sum"
if ($sum -eq $target) { "OK" } else { "NOT OK" }
## How‑To / FAQ

### How do I see the **genesis** blockchain account?
The **genesis** assigns the full supply (1.0 = 10^45 units) to the wallet you used when running `genesis`.

**Find your AccountId (hex32):**
```cmd
target\release\ul-wallet.exe --keystore .\n1.json --password "pw" address
```

**See its balance (Windows):**
```cmd
target\release\ul-node.exe --db .\n1 --keystore .\n1.json --password "pw" balance
```

**Inspector (Windows):**
```cmd
target\release\ul-inspect.exe --db .\n1 status
target\release\ul-inspect.exe --db .\n1 balances --limit 0 --decode amount1e45 | findstr /i <YOUR_HEX32>
```
*(macOS/Linux equivalents are in **Quick Commands (macOS/Linux)** above.)*

---

### Where do I transfer **fees**?
There are **no fees** in this prototype (by design). Transfers are A→B only, fee‑less. Nodes earn off‑chain (e.g., market‑making).  
If you want fees later, we can add: `sender debited (amount+fee)`, `receiver credited amount`, `fee_account credited fee`, while enforcing **Σ balances = 10^45**.

---

### Is the coin “kept on the wallet that creates the block”?
- **Genesis funds** go to the **AccountId** derived from the wallet used for `genesis`.
- Blocks/state live in the **DB folder** given via `--db` (e.g., `./n1`).  
- In the current **dev-local** mode, `create-account` and `transfer` update your local DB immediately so you can inspect balances and blocks.

---

### Can I create more than one (wallets/accounts/nodes/chains)?
Yes.

- **More wallets**
```cmd
target\release\ul-wallet.exe --keystore .\n2.json --password "pw" new
```
- **More on‑chain accounts (from any node)**
```cmd
target\release\ul-node.exe --db .\n1 --keystore .\n1.json --password "pw" create-account --addr-hex32 <HEX32>
```
- **More nodes on one chain** → use a different DB dir per node (e.g., `./n2`, `./n3`).
- **More chains** → run `genesis` with a brand‑new DB path (e.g., `./chainB`).  
  *(Genesis refuses to run twice on the same DB: `meta.height exists`.)*

---

### How do I **hide the password** (no echo)?
Make `--password` optional and prompt securely if missing, using `rpassword`.

**Add dependency in both `ul-wallet` and `ul-node`:**
```toml
# crates/ul-wallet/Cargo.toml
# crates/ul-node/Cargo.toml
[dependencies]
rpassword = "7"
```

**Prompt if `--password` is omitted (example for `ul-node/src/main.rs`):**
```rust
use rpassword::prompt_password;

#[derive(clap::Parser)]
struct Cli {
    #[arg(long)] db: String,
    #[arg(long)] keystore: String,
    #[arg(long)] password: Option<String>, // make optional
    // ... other args ...
}

fn get_password(cli: &Cli) -> anyhow::Result<String> {
    if let Some(p) = &cli.password { return Ok(p.clone()); }
    Ok(prompt_password("Keystore password: ")?)
}

// then
let pw = get_password(&cli)?;
let kp = ul_keystore::load(&cli.keystore, &pw)?;
```

**Usage (you will be prompted; input is hidden):**
```cmd
target\release\ul-node.exe --db .\n1 --keystore .\n1.json balance
```

---

### How do I communicate across different computers (network)?

**Option A — Use a relay (most reliable through NAT):**

1) Run relay on a reachable machine:
```cmd
set RUST_LOG=info && target\release\ul-relay.exe
```
2) Open UDP **7000** on firewall/router (Windows example):
```powershell
New-NetFirewallRule -DisplayName "UL Relay UDP 7000" -Direction Inbound -Protocol UDP -LocalPort 7000 -Action Allow
```
3) Share the address:
```
/ip4/<RELAY_IP>/udp/7000/quic-v1/p2p/<RELAY_PEER_ID>
```
4) Other nodes dial it when starting:
```cmd
set RUST_LOG=info && target\release\ul-node.exe --db .\n2 --keystore .\n2.json --password "pw" --dial "/ip4/<RELAY_IP>/udp/7000/quic-v1/p2p/<RELAY_PEER_ID>" run
```

**Option B — Direct dial (if peers are reachable on UDP 7001):**
```cmd
:: Node A (listen)
set RUST_LOG=info && target\release\ul-node.exe --db .\nA --keystore .\a.json --password "pw" --listen /ip4/0.0.0.0/udp/7001/quic-v1 run

:: Node B (dial A)
set RUST_LOG=info && target\release\ul-node.exe --db .\nB --keystore .\b.json --password "pw" --dial "/ip4/<A_PUBLIC_IP>/udp/7001/quic-v1" run
```

> The stack includes AutoNAT & DCUtR for hole punching; a relay is the most dependable path through strict NATs.

---

### What instructions are there for **node processing**? (dev-local quick tour)

**See your balance**
```cmd
target\release\ul-node.exe --db .\n1 --keystore .\n1.json --password "pw" balance
```

**Create a recipient**
```cmd
target\release\ul-node.exe --db .\n1 --keystore .\n1.json --password "pw" create-account --addr-hex32 0b41e83c6ba339d92cbc1ce17b160ea5e93f6021eea14cbf33c7fb9368eff26d
```

**Transfer (no fees; dev‑local apply)**
```cmd
target\release\ul-node.exe --db .\n1 --keystore .\n1.json --password "pw" transfer --to-hex32 0b41e83c6ba339d92cbc1ce17b160ea5e93f6021eea14cbf33c7fb9368eff26d --amount 0.25
```

**Verify balances**
```cmd
:: recipient ~0.25
target\release\ul-inspect.exe --db .\n1 balances --limit 0 --decode amount1e45 | findstr /i 0b41e8

:: sender ~0.75 (your 236adf... address)
target\release\ul-inspect.exe --db .\n1 balances --limit 0 --decode amount1e45 | findstr /i 236adf83
```

**See the blockchain (dev blocks)**
```cmd
target\release\ul-node.exe --db .\n1 --keystore .\n1.json --password "pw" print-blocks --last 10
```

**Verify total supply = 1.0 (exact)**
```powershell
$rows = & target\release\ul-inspect.exe --db .\n1 balances --limit 0 --decode uint-be --json | ConvertFrom-Json
$sum = [System.Numerics.BigInteger]::Zero
foreach ($r in $rows) { $sum += [System.Numerics.BigInteger]::Parse($r[1]) }
$target = [System.Numerics.BigInteger]::Pow(10,45)
"units=$sum"
if ($sum -eq $target) { "OK" } else { "NOT OK" }
```


## Quick Commands (macOS/Linux)

### See your balance
```bash
./target/release/ul-node --db ./n1 --keystore ./n1.json --password "pw" balance
```

### Create recipient
```bash
./target/release/ul-node --db ./n1 --keystore ./n1.json --password "pw" create-account --addr-hex32 0b41e83c6ba339d92cbc1ce17b160ea5e93f6021eea14cbf33c7fb9368eff26d
```

### Transfer (no fees; dev-local apply)
```bash
./target/release/ul-node --db ./n1 --keystore ./n1.json --password "pw" transfer --to-hex32 0b41e83c6ba339d92cbc1ce17b160ea5e93f6021eea14cbf33c7fb9368eff26d --amount 0.25
```

### Verify balances
```bash
# recipient ~0.25
./target/release/ul-inspect --db ./n1 balances --limit 0 --decode amount1e45 | grep -i 0b41e8

# sender ~0.75 (your 236adf... address)
./target/release/ul-inspect --db ./n1 balances --limit 0 --decode amount1e45 | grep -i 236adf83
```

### See the blockchain (dev blocks)
```bash
./target/release/ul-node --db ./n1 --keystore ./n1.json --password "pw" print-blocks --last 10
```

### Verify total supply = 1.0 (exact)
```bash
./target/release/ul-inspect --db ./n1 balances --limit 0 --decode uint-be --json | python3 -c "import sys,json; rows=json.load(sys.stdin); s=sum(int(v[1]) for v in rows); print('units=',s); print('OK' if s==10**45 else 'NOT OK')"