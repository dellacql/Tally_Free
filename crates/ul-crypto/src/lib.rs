use ed25519_dalek::{SigningKey, VerifyingKey, Signature, Signer, Verifier};
use rand::rngs::OsRng;
use ul_types::AccountId;

pub struct Keypair { pub sk: SigningKey, pub vk: VerifyingKey }

impl Keypair {
    pub fn random() -> Self {
        let sk = SigningKey::generate(&mut OsRng);
        let vk = sk.verifying_key();
        Self { sk, vk }
    }
    pub fn account_id(&self) -> AccountId {
        AccountId(blake3::hash(self.vk.as_bytes()).into())
    }
    pub fn sign(&self, bytes: &[u8]) -> Vec<u8> {
        self.sk.sign(bytes).to_bytes().to_vec()
    }
}

pub fn verify_sig(vk: &VerifyingKey, msg: &[u8], sig: &[u8]) -> bool {
    match Signature::from_slice(sig) {
        Ok(s) => vk.verify(msg, &s).is_ok(),
        Err(_) => false,
    }
}
