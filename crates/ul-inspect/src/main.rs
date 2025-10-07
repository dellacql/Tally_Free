use anyhow::{Context, Result};
use clap::{Parser, Subcommand, Args, ValueEnum};
use num_bigint::BigUint;
use sled::{Db, Tree};
use std::path::Path;

/// Default tree names used by the node's sled DB.
#[derive(Debug, Clone)]
struct Trees {
    meta: String,
    balances: String,
    stake: String,
    admins: String,
    blocks: String,
}

impl Default for Trees {
    fn default() -> Self {
        Self {
            meta: "meta".into(),
            balances: "balances".into(),
            stake: "stake".into(),
            admins: "admins".into(),
            blocks: "blocks".into(),
        }
    }
}

#[derive(ValueEnum, Copy, Clone, Debug)]
enum Decode {
    /// Show value bytes as hex
    Hex,
    /// Interpret bytes as big-endian integer scaled by 1e45 (Amount units) and print decimal
    Amount1e45,
    /// Interpret bytes as big-endian integer (no scale)
    UintBe,
}

#[derive(Parser, Debug)]
#[command(name="ul-inspect", about="Unity Ledger inspector (read-only)")]
struct Cli {
    /// Path to the node database directory (the --db you used when running ul-node)
    #[arg(long, value_name="PATH")]
    db: String,

    /// Override tree names (only if you changed defaults in the node)
    #[arg(long, default_value_t=Trees::default().meta)]
    tree_meta: String,
    #[arg(long, default_value_t=Trees::default().balances)]
    tree_balances: String,
    #[arg(long, default_value_t=Trees::default().stake)]
    tree_stake: String,
    #[arg(long, default_value_t=Trees::default().admins)]
    tree_admins: String,
    #[arg(long, default_value_t=Trees::default().blocks)]
    tree_blocks: String,

    #[command(subcommand)]
    cmd: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Print overall status: height, parent, counts
    Status,
    /// List balances (keys are AccountId bytes -> hex). Use --decode for values.
    Balances(BalancesArgs),
    /// List stake entries (AccountId -> value)
    Stake(ScanArgs),
    /// List admin set (AccountId only)
    Admins(ScanAdminsArgs),
    /// List block keys/lengths (metadata-level view)
    Blocks(BlocksArgs),
    /// List all tree names present in the DB
    Trees,
}

#[derive(Args, Debug)]
struct BalancesArgs {
    /// Show at most N rows (0 = unlimited)
    #[arg(long, default_value_t = 50)]
    limit: usize,
    /// Value decoder (default = Hex)
    #[arg(long, value_enum, default_value_t=Decode::Hex)]
    decode: Decode,
    /// Print as JSON
    #[arg(long, default_value_t=false)]
    json: bool,
}

#[derive(Args, Debug)]
struct ScanArgs {
    #[arg(long, default_value_t = 50)]
    limit: usize,
    #[arg(long, value_enum, default_value_t=Decode::Hex)]
    decode: Decode,
    #[arg(long, default_value_t=false)]
    json: bool,
}

#[derive(Args, Debug)]
struct ScanAdminsArgs {
    #[arg(long, default_value_t = 50)]
    limit: usize,
    #[arg(long, default_value_t=false)]
    json: bool,
}

#[derive(Args, Debug)]
struct BlocksArgs {
    /// Show the last N keys by lexicographic order
    #[arg(long, default_value_t = 20)]
    last: usize,
    #[arg(long, default_value_t=false)]
    json: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let db = open_db(&cli.db)?;
    let trees = Trees {
        meta: cli.tree_meta,
        balances: cli.tree_balances,
        stake: cli.tree_stake,
        admins: cli.tree_admins,
        blocks: cli.tree_blocks,
    };

    match cli.cmd {
        Command::Status => cmd_status(&db, &trees),
        Command::Balances(args) => cmd_scan_kv(&db, &trees.balances, args.limit, args.decode, args.json),
        Command::Stake(args) => cmd_scan_kv(&db, &trees.stake, args.limit, args.decode, args.json),
        Command::Admins(args) => cmd_scan_admins(&db, &trees.admins, args.limit, args.json),
        Command::Blocks(args) => cmd_blocks(&db, &trees.blocks, args.last, args.json),
        Command::Trees => cmd_trees(&db),
    }
}

fn open_db(path: &str) -> Result<Db> {
    anyhow::ensure!(Path::new(path).exists(), "DB path not found: {path}");
    sled::open(path).with_context(|| format!("opening sled DB at {path}"))
}

fn open_tree(db: &Db, name: &str) -> Result<Tree> {
    db.open_tree(name).with_context(|| format!("opening tree '{name}'"))
}

fn cmd_status(db: &Db, trees: &Trees) -> Result<()> {
    let meta = open_tree(db, &trees.meta)?;
    let balances = db.open_tree(&trees.balances)?;
    let stake = db.open_tree(&trees.stake)?;
    let admins = db.open_tree(&trees.admins)?;
    let blocks = db.open_tree(&trees.blocks)?;

    let height = meta.get("height")?
        .and_then(|ivec| {
            let bytes: [u8;8] = ivec.as_ref().try_into().ok()?;
            Some(u64::from_be_bytes(bytes))
        });

    let parent_hex = meta.get("parent")?
        .map(|ivec| hex::encode(ivec.as_ref()));

    println!("== Status ==");
    println!("DB: <opened>");
    println!("Height: {}", height.map(|h| h.to_string()).unwrap_or("unknown".into()));
    println!("Parent: {}", parent_hex.unwrap_or("<none>".into()));
    println!("Trees:");
    println!("  {}: {} entries", trees.balances, count_entries(&balances));
    println!("  {}: {} entries", trees.stake,    count_entries(&stake));
    println!("  {}: {} entries", trees.admins,   count_entries(&admins));
    println!("  {}: {} entries", trees.blocks,   count_entries(&blocks));
    Ok(())
}

fn cmd_trees(db: &Db) -> Result<()> {
    println!("== Trees present ==");
    for name in db.tree_names() {
        // sled returns Vec<IVec>; IVec implements AsRef<[u8]>
        println!("- {}", String::from_utf8_lossy(name.as_ref()));
    }
    Ok(())
}

fn count_entries(tree: &Tree) -> usize {
    tree.iter().count()
}

fn cmd_scan_kv(db: &Db, tree_name: &str, limit: usize, decode: Decode, json: bool) -> Result<()> {
    let t = open_tree(db, tree_name)?;
    let mut rows = Vec::new();
    for item in t.iter() {
        let (k, v) = item?;
        let key_hex = hex32_or_hex(k.as_ref());
        let val_str = decode_value(v.as_ref(), decode);
        rows.push((key_hex, val_str));
        if limit != 0 && rows.len() >= limit { break; }
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        println!("== Tree: {tree_name} (showing {}{}) ==", rows.len(), if limit==0 { "" } else { " (limited)" });
        for (k, v) in rows {
            println!("{k}  ->  {v}");
        }
    }
    Ok(())
}

fn cmd_scan_admins(db: &Db, tree_name: &str, limit: usize, json: bool) -> Result<()> {
    let t = open_tree(db, tree_name)?;
    let mut rows = Vec::new();
    for item in t.iter() {
        let (k, _v) = item?;
        rows.push(hex32_or_hex(k.as_ref()));
        if limit != 0 && rows.len() >= limit { break; }
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        println!("== Admins: {} (showing {}{}) ==", tree_name, rows.len(), if limit==0 { "" } else { " (limited)" });
        for k in rows { println!("{k}"); }
    }
    Ok(())
}

fn cmd_blocks(db: &Db, tree_name: &str, last: usize, json: bool) -> Result<()> {
    let t = open_tree(db, tree_name)?;
    let mut keys: Vec<Vec<u8>> = t.iter().map(|kv| kv.unwrap().0.to_vec()).collect();
    keys.sort();
    if last > 0 && keys.len() > last {
        keys = keys[keys.len()-last..].to_vec();
    }
    if json {
        let as_hex: Vec<String> = keys.iter().map(|k| hex::encode(k)).collect();
        println!("{}", serde_json::to_string_pretty(&as_hex)?);
    } else {
        println!("== Blocks (last {last}) from tree '{tree_name}' ==");
        for k in keys {
            println!("{}", hex::encode(k));
        }
    }
    Ok(())
}

fn hex32_or_hex(bytes: &[u8]) -> String {
    if bytes.len() == 32 { hex::encode(bytes) } else { hex::encode(bytes) }
}

fn decode_value(bytes: &[u8], mode: Decode) -> String {
    match mode {
        Decode::Hex => format!("0x{}", hex::encode(bytes)),
        Decode::UintBe => {
            let n = BigUint::from_bytes_be(bytes);
            n.to_string()
        }
        Decode::Amount1e45 => {
            let n = BigUint::from_bytes_be(bytes);
            fmt_amount_1e45(&n)
        }
    }
}

fn fmt_amount_1e45(n: &BigUint) -> String {
    let scale = pow10(45);
    let (q, r) = (n / &scale, n % &scale);

    let int_part = q.to_string();
    let mut frac = r.to_string();
    if frac.len() < 45 {
        let mut s = String::with_capacity(45);
        for _ in 0..(45 - frac.len()) { s.push('0'); }
        s.push_str(&frac);
        frac = s;
    }
    let frac_trim = frac.trim_end_matches('0').to_string();
    if frac_trim.is_empty() {
        int_part
    } else {
        format!("{int_part}.{frac_trim}")
    }
}

fn pow10(exp: usize) -> BigUint {
    let ten = BigUint::from(10u32);
    let mut n = BigUint::from(1u32);
    for _ in 0..exp { n *= &ten; }
    n
}
