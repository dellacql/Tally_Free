use anyhow::Result;
use clap::{Parser, Subcommand};
use rpassword::prompt_password;
use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;
use ul_keystore as ks;

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "./keystore.json")]
    keystore: String,
    #[arg(long, default_value = "")]
    password: String,
    #[command(subcommand)]
    cmd: Cmd,
}
#[derive(Subcommand)]
enum Cmd {
    New,
    Address,
    ExportVk,
}

fn main() -> Result<()> {
    let a = Args::parse();
    match a.cmd {
        Cmd::New => {
            let kp = ks::create(&a.keystore, &a.password)?;
            println!("created. address={}", hex::encode(kp.account_id().0));
        }
        Cmd::Address => {
            let kp = ks::load(&a.keystore, &a.password)?;
            println!("{}", hex::encode(kp.account_id().0));
        }
        Cmd::ExportVk => {
            let kp = ks::load(&a.keystore, &a.password)?;
            println!("{}", hex::encode(kp.vk.as_bytes()));
        }
    }
    Ok(())
}

/// Common CLI fields for secret handling
#[derive(Debug, Parser)]
struct SecretCli {
    /// Pass password directly (discouraged; visible in history/ps)
    #[arg(long)]
    password: Option<String>,

    /// Read password from this file (trim trailing newlines)
    #[arg(long, conflicts_with = "password")]
    password_file: Option<PathBuf>,

    /// Read password from STDIN (trim trailing newlines)
    #[arg(long, conflicts_with_all = ["password", "password_file"])]
    password_stdin: bool,
}

/// Resolve the password using prompt / file / stdin / literal

#[allow(dead_code)]
fn get_password(secret: &SecretCli, prompt: &str) -> Result<String> {
    // 1) explicit --password takes precedence
    if let Some(pw) = &secret.password {
        return Ok(pw.clone());
    }

    // 2) read from file: --password-file <path>
    if let Some(path) = &secret.password_file {
        let s = fs::read_to_string(path)?;
        // strip trailing newlines
        return Ok(s.trim_end_matches(&['\r', '\n'][..]).to_owned());
    }

    // 3) read entire stdin: --password-stdin
    if secret.password_stdin {
        let mut s = String::new();
        io::stdin().read_to_string(&mut s)?;
        return Ok(s.trim_end_matches(&['\r', '\n'][..]).to_owned());
    }

    // 4) interactive, hidden input
    let s = prompt_password(prompt)?;
    Ok(s)
}
