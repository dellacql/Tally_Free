# Tally Free testnet guide

Tally Free is a fixed-supply blockchain.

The chain begins with one genesis block. The genesis account owns exactly `1.0` token. Internally, `1.0` is represented as `10^45` indivisible units.

A node does not create genesis unless it is intentionally creating the original chain. Normal joining nodes request the chain from peers, install the received genesis block, verify block continuity, and then participate.

Transfers never create or destroy units. A sender signs a transfer giving units to another address. The receiver does not need to accept the transfer. If the sender signature, nonce, and balance are valid, the network can commit the transfer.

## Build

```powershell
cargo build
cargo test