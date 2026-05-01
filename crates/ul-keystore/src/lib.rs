use anyhow::{anyhow, Result};
use argon2::Argon2;
use chacha20poly1305::{aead::Aead, ChaCha20Poly1305, KeyInit};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use std::path::Path;
use ul_crypto::Keypair;
use zeroize::Zeroize;

#[derive(Serialize, Deserialize)]
struct Enc {
    salt: Vec<u8>,
    nonce: Vec<u8>,
    cipher: Vec<u8>,
    vk: Vec<u8>, // public for address display
}

pub fn create(path: &str, password: &str) -> Result<Keypair> {
    let kp = Keypair::random();
    save(path, password, &kp)?;
    Ok(kp)
}

pub fn save(path: &str, password: &str, kp: &Keypair) -> Result<()> {
    // 16B salt, 12B nonce
    let mut salt = [0u8; 16];
    OsRng.fill_bytes(&mut salt);
    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut nonce);
    let mut key_bytes = [0u8; 32];

    // Argon2id KDF -> 32B key
    Argon2::default()
        .hash_password_into(password.as_bytes(), &salt, &mut key_bytes)
        .map_err(|e| anyhow!("argon2: {e}"))?;

    let cipher = ChaCha20Poly1305::new((&key_bytes).into());
    let enc_bytes = cipher
        .encrypt(
            chacha20poly1305::Nonce::from_slice(&nonce),
            kp.sk.to_bytes().as_slice(),
        )
        .map_err(|e| anyhow!("encrypt: {e}"))?;

    let file = Enc {
        salt: salt.to_vec(),
        nonce: nonce.to_vec(),
        cipher: enc_bytes,
        vk: kp.vk.as_bytes().to_vec(),
    };
    std::fs::write(path, serde_json::to_vec_pretty(&file)?)?;
    key_bytes.zeroize();
    Ok(())
}

pub fn load(path: &str, password: &str) -> Result<Keypair> {
    let buf = std::fs::read(path)?;
    let enc: Enc = serde_json::from_slice(&buf)?;
    let mut key_bytes = [0u8; 32];

    Argon2::default()
        .hash_password_into(password.as_bytes(), &enc.salt, &mut key_bytes)
        .map_err(|e| anyhow!("argon2: {e}"))?;

    let cipher = ChaCha20Poly1305::new((&key_bytes).into());
    let plain = cipher
        .decrypt(
            chacha20poly1305::Nonce::from_slice(&enc.nonce),
            enc.cipher.as_ref(),
        )
        .map_err(|_| anyhow!("keystore: bad password or corrupted file"))?;

    let sk = ed25519_dalek::SigningKey::from_bytes(
        plain
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("bad key length"))?,
    );
    let vk = sk.verifying_key();
    key_bytes.zeroize();
    Ok(Keypair { sk, vk })
}
/// Load an existing key from `path`, or create a new one, save it, and return it.
pub fn load_or_create(path: &str, password: &str) -> Result<Keypair> {
    if Path::new(path).exists() {
        load(path, password)
    } else {
        let kp = Keypair::random(); // <-- was Keypair::generate()
        save(path, password, &kp)?;
        Ok(kp)
    }
}
