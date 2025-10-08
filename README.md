# Tally_Free — Complete Guide (Networking & Operations)

A minimal, fee-less blockchain prototype written in Rust with **libp2p** networking (QUIC + Gossipsub + Kademlia + AutoNAT + DCUtR). The design enforces a fixed total supply of **1.0** (internally **10^45 units**, i.e., 45 decimal places). Nodes hold the state DB; wallets hold keys. Transactions are binary (A→B) with **no protocol fees**.

> ⚠️ Learning prototype for testnets. Not production-hardened.

---

## Quick Start (Windows, two-minute demo)

```cmd
:: 1) Build
cargo build --release

:: 2) Create wallet + address
target\release\ul-wallet.exe --keystore .\n1.json --password "pw" new
target\release\ul-wallet.exe --keystore .\n1.json --password "pw" address

:: 3) Init genesis into a new DB
target\release\ul-node.exe --db .\n1 --keystore .\n1.json --password "pw" genesis

:: 4) Run node (keep this window open)
set RUST_LOG=info && target\release\ul-node.exe --db .\n1 --keystore .\n1.json --password "pw" --listen /ip4/0.0.0.0/udp/7001/quic-v1 run

:: 5) In another window, create recipient + transfer 0.25
target\release\ul-node.exe --db .\n1 --keystore .\n1.json --password "pw" create-account --addr-hex32 0b41e83c6ba339d92cbc1ce17b160ea5e93f6021eea14cbf33c7fb9368eff26d
target\release\ul-node.exe --db .\n1 --keystore .\n1.json --password "pw" transfer --to-hex32 0b41e83c6ba339d92cbc1ce17b160ea5e93f6021eea14cbf33c7fb9368eff26d --amount 0.25

:: 6) Inspect balances (recipient ~0.25, sender ~0.75)
target\release\ul-inspect.exe --db .\n1 balances --limit 0 --decode amount1e45 | findstr /i 0b41e8
target\release\ul-inspect.exe --db .\n1 balances --limit 0 --decode amount1e45 | findstr /i 236adf83

:: 7) Verify total supply == 1.0 (exact, 10^45 units)
powershell -NoProfile -Command ^
  "$rows = & target\release\ul-inspect.exe --db .\n1 balances --limit 0 --decode uint-be --json | ConvertFrom-Json; " ^
+ "$sum = [System.Numerics.BigInteger]::Zero; foreach ($r in $rows) { $sum += [System.Numerics.BigInteger]::Parse($r[1]) };" ^
+ "$t = [System.Numerics.BigInteger]::Pow(10,45); if ($sum -eq $t) { 'OK' } else { 'NOT OK' }"
```

---

## Table of Contents

- [0. Prerequisites](#0-prerequisites)
- [1. Build (all OS)](#1-build-all-os)
- [2. Binaries](#2-binaries)
- [3. Wallet (Keystore) & Address](#3-wallet-keystore--address)
- [4. Initialize a Dev Genesis](#4-initialize-a-dev-genesis)
- [5. Run a Node (Single Machine)](#5-run-a-node-single-machine)
- [6. Networking Between Different Computers](#6-networking-between-different-computers)
  - [6A. Two-Node Quick Demo (with Relay)](#6a-two-node-quick-demo-with-relay)
  - [6B. Direct Dial (without Relay)](#6b-direct-dial-without-relay)
- [7. Accounts, Transfers, Balances](#7-accounts-transfers-balances)
- [8. Inspect State & Verify Total Supply](#8-inspect-state--verify-total-supply)
- [9. Hidden Password Prompt (safer input)](#9-hidden-password-prompt-safer-input)
- [10. FAQ](#10-faq)
- [11. Troubleshooting](#11-troubleshooting)
- [12. Appendix: Ports, Multiaddrs, Notes](#12-appendix-ports-multiaddrs-notes)

---

## 0. Prerequisites

- Install Rust: <https://rustup.rs/>
- Verify:
  ```sh
  rustc -V
  cargo -V
  ```

> Windows users: run commands in **Developer PowerShell** or **cmd**.

---

## 1. Build (all OS)

From the repo root:

```sh
cargo build --release
```

---

## 2. Binaries

After building:

- `target/release/ul-wallet[.exe]`   — manage keystore, print address
- `target/release/ul-node[.exe]`     — node (DB, tx submission, network loop)
- `target/release/ul-relay[.exe]`    — optional libp2p relay for NAT traversal
- `target/release/ul-inspect[.exe]`  — read-only inspection of the DB

---

## 3. Wallet (Keystore) & Address

A wallet is your **encrypted key** on disk. It is **not** an on-chain account by itself.

**Windows**
```cmd
target\release\ul-wallet.exe --keystore .\n1.json --password "pw" new
target\release\ul-wallet.exe --keystore .\n1.json --password "pw" address
```

**macOS/Linux**
```bash
./target/release/ul-wallet --keystore ./n1.json --password "pw" new
./target/release/ul-wallet --keystore ./n1.json --password "pw" address
```

Copy the 64-hex AccountId (32 bytes hex).

---

## 4. Initialize a Dev Genesis

Initialize a DB directory and assign **all supply (1.0)** to your wallet’s AccountId.

**Windows**
```cmd
target\release\ul-node.exe --db .\n1 --keystore .\n1.json --password "pw" genesis
```

**macOS/Linux**
```bash
./target/release/ul-node --db ./n1 --keystore ./n1.json --password "pw" genesis
```

Expected log includes:
```
ul-node starting; peer id: 12D3KooW...
wallet accountId (hex32): <YOUR_HEX32>
✔ genesis initialized at .\n1
```

> Re-running `genesis` on the same DB will refuse with `meta.height exists`.

---

## 5. Run a Node (Single Machine)

Run the node network loop. Keep this terminal **open**.

**Windows**
```cmd
set RUST_LOG=info && target\release\ul-node.exe --db .\n1 --keystore .\n1.json --password "pw" --listen /ip4/0.0.0.0/udp/7001/quic-v1 run
```

**macOS/Linux**
```bash
RUST_LOG=info ./target/release/ul-node --db ./n1 --keystore ./n1.json --password "pw" --listen /ip4/0.0.0.0/udp/7001/quic-v1 run
```

---

## 6. Networking Between Different Computers

There are two paths:

- **Use a relay** (best through NATs)
- **Direct dial** (works if the listener is reachable on UDP 7001)

### 6A. Two-Node Quick Demo (with Relay)

**Terminal A — Relay (reachable host)**
```cmd
set RUST_LOG=info && target\release\ul-relay.exe
```

You’ll see:
```
relay peerId: <RELAY_PEER_ID>
listening on: /ip4/0.0.0.0/udp/7000/quic-v1
addr: /ip4/<HOST_IP>/udp/7000/quic-v1
```

**Open firewall/port on the relay machine (Windows example):**
```powershell
New-NetFirewallRule -DisplayName "UL Relay UDP 7000" -Direction Inbound -Protocol UDP -LocalPort 7000 -Action Allow
```

**Construct the relay multiaddr** (share with peers):
```
/ip4/<RELAY_IP>/udp/7000/quic-v1/p2p/<RELAY_PEER_ID>
```

**Terminal B — Node #1**
```cmd
set RUST_LOG=info && target\release\ul-node.exe --db .\nA --keystore .\a.json --password "pw" --listen /ip4/0.0.0.0/udp/7001/quic-v1 --dial "/ip4/<RELAY_IP>/udp/7000/quic-v1/p2p/<RELAY_PEER_ID>" run
```

**Terminal C — Node #2** (on another computer)
```cmd
set RUST_LOG=info && target\release\ul-node.exe --db .\nB --keystore .\b.json --password "pw" --listen /ip4/0.0.0.0/udp/7001/quic-v1 --dial "/ip4/<RELAY_IP>/udp/7000/quic-v1/p2p/<RELAY_PEER_ID>" run
```

> As both nodes dial the same relay, they can discover and exchange traffic. Keep all three terminals running.

### 6B. Direct Dial (without Relay)

**On Node A (publicly reachable)**
```cmd
set RUST_LOG=info && target\release\ul-node.exe --db .\nA --keystore .\a.json --password "pw" --listen /ip4/0.0.0.0/udp/7001/quic-v1 run
```
Open firewall/port **7001/UDP** on A.

**On Node B (another machine)**
```cmd
set RUST_LOG=info && target\release\ul-node.exe --db .\nB --keystore .\b.json --password "pw" --dial "/ip4/<A_PUBLIC_IP>/udp/7001/quic-v1" run
```

> If direct dial fails due to strict NAT/firewall, use the relay flow (6A).

---

## 7. Accounts, Transfers, Balances

> Current CLI is **dev-friendly**. Accounts and transfers can be submitted locally and inspected immediately for learning and demos.

### See your balance (human decimal)
```cmd
target\release\ul-node.exe --db .\n1 --keystore .\n1.json --password "pw" balance
```

### Create a recipient account (by hex32)
```cmd
target\release\ul-node.exe --db .\n1 --keystore .\n1.json --password "pw" create-account --addr-hex32 0b41e83c6ba339d92cbc1ce17b160ea5e93f6021eea14cbf33c7fb9368eff26d
```

### Transfer (no fees; dev-local apply)
```cmd
target\release\ul-node.exe --db .\n1 --keystore .\n1.json --password "pw" transfer --to-hex32 0b41e83c6ba339d92cbc1ce17b160ea5e93f6021eea14cbf33c7fb9368eff26d --amount 0.25
```

> Internally, amounts are 45-decimal fixed-point (10^45 units = 1.0).

---

## 8. Inspect State & Verify Total Supply

### Node status / trees / height
```cmd
target\release\ul-inspect.exe --db .\n1 status
```

### Find balances for a specific account
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

### Verify total supply == 1.0 (exact integer sum)

**Windows (PowerShell):**
```powershell
$rows = & target\release\ul-inspect.exe --db .\n1 balances --limit 0 --decode uint-be --json | ConvertFrom-Json
$sum = [System.Numerics.BigInteger]::Zero
foreach ($r in $rows) { $sum += [System.Numerics.BigInteger]::Parse($r[1]) }
$target = [System.Numerics.BigInteger]::Pow(10,45)
"units=$sum"
if ($sum -eq $target) { "OK" } else { "NOT OK" }
```

**macOS/Linux (Python 3):**
```bash
./target/release/ul-inspect --db ./n1 balances --limit 0 --decode uint-be --json \
| python3 -c 'import sys,json; rows=json.load(sys.stdin); s=sum(int(v[1]) for v in rows); print("units=",s); print("OK" if s==10**45 else "NOT OK")'
```

---

## 9. Hidden Password Prompt (safer input)

Right now examples pass `--password "pw"` for speed. To avoid echoing or storing passwords in shell history:

1) Add to **both** `crates/ul-wallet/Cargo.toml` and `crates/ul-node/Cargo.toml`:
   ```toml
   [dependencies]
   rpassword = "7"
   ```

2) Make `--password` optional and prompt if omitted (example sketch):
   ```rust
   use rpassword::prompt_password;

   #[derive(clap::Parser)]
   struct Cli {
       #[arg(long)] keystore: String,
       #[arg(long)] password: Option<String>,
       // ...
   }
   fn get_password(cli: &Cli) -> anyhow::Result<String> {
       Ok(cli.password.clone().unwrap_or_else(|| prompt_password("Keystore password: ").unwrap()))
   }
   ```

Usage then becomes:
```cmd
target\release\ul-node.exe --db .\n1 --keystore .\n1.json balance
# (you'll be securely prompted)
```

---

## 10. FAQ

**Q: Where is the “genesis account”?**  
A: It’s the **AccountId** (hex32) of the wallet that ran `genesis`. The full supply (10^45 units) is stored under that key in the `balances` tree of your DB.

**Q: Where do fees go?**  
A: There are **no protocol fees** in this prototype. Nodes can earn off-chain (e.g., market making). If fees are added later, the supply check (`Σ balances == 10^45`) still holds with a fee sink account.

**Q: Can I create multiple wallets/nodes/chains?**  
A: Yes. Use different `--keystore` files and different `--db` directories. To start a **new chain**, run `genesis` with a fresh DB path.

**Q: How do nodes find each other?**  
A: Either **dial a relay** address (`/ip4/<relay>/udp/7000/quic-v1/p2p/<id>`) or **direct dial** a listening node (`/ip4/<ip>/udp/7001/quic-v1`). libp2p’s AutoNAT and DCUtR help, but relays are most reliable through strict NATs.

---

## 11. Troubleshooting

| Symptom / Message | Cause | Fix |
|---|---|---|
| `The system cannot find the file specified (os error 2)` | Missing keystore or DB path | Create wallet via `ul-wallet ... new`; check `--db` path |
| `genesis refused: meta.height exists` | DB already initialized | Use a new `--db` dir or delete the old |
| EXE locked during build | Process still running | Ctrl+C the app or `taskkill /IM ul-node.exe /F` |
| No peers | NAT/firewall | Use a relay and/or open UDP 7000/7001 |
| Relay prints no `/p2p/<PeerId>` suffix | Cosmetic output | Append `/p2p/<PeerId>` manually (relay prints the PeerId separately) |
| Account/transfer not visible | Waiting for inclusion | In dev, you can inspect local DB immediately; for multi-node consensus, ensure both nodes are peered (relay/direct) |

---

## 12. Appendix: Ports, Multiaddrs, Notes

- **Ports**
  - Relay: UDP **7000**
  - Nodes (example here): UDP **7001**
- **Multiaddrs**
  - Relay listening addr: `/ip4/<ip>/udp/7000/quic-v1`
  - Relay shareable (add PeerId): `/ip4/<ip>/udp/7000/quic-v1/p2p/<id>`
  - Node listening addr: `/ip4/0.0.0.0/udp/7001/quic-v1`
  - Direct dial target: `/ip4/<public-ip>/udp/7001/quic-v1`
- **Supply invariant**: Σ balances = **10^45** (exact), i.e. 1.0.
- **No fees**: A→B transfers only (fee-less).
- **Stake/consensus (roadmap)**: stake-weighted, supermajority (**≥72%**) to finalize blocks. Safety check: total supply must match **10^45** units each commit.
