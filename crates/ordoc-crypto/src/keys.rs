use argon2::{Algorithm, Argon2, Params, Version};
use rand_core::{OsRng, RngCore};
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::error::{CryptoError, Result};

pub const MASTER_KEY_LEN: usize = 32;
pub const SALT_LEN: usize = 16;

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct MasterKey([u8; MASTER_KEY_LEN]);

impl MasterKey {
    pub fn generate() -> Self {
        let mut bytes = [0u8; MASTER_KEY_LEN];
        OsRng.fill_bytes(&mut bytes);
        Self(bytes)
    }

    pub fn from_bytes(bytes: [u8; MASTER_KEY_LEN]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; MASTER_KEY_LEN] {
        &self.0
    }

    pub fn derive_subkey(&self, context: &str) -> SubKey {
        let derived = blake3::derive_key(context, &self.0);
        SubKey(derived)
    }
}

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct SubKey([u8; MASTER_KEY_LEN]);

impl SubKey {
    pub fn as_bytes(&self) -> &[u8; MASTER_KEY_LEN] {
        &self.0
    }
}

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(secret: String) -> Self {
        Self(secret)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

pub fn generate_salt() -> [u8; SALT_LEN] {
    let mut salt = [0u8; SALT_LEN];
    OsRng.fill_bytes(&mut salt);
    salt
}

pub fn derive_key_argon2id(passphrase: &[u8], salt: &[u8; SALT_LEN]) -> Result<SubKey> {
    let params = Params::new(65_536, 3, 2, Some(MASTER_KEY_LEN))
        .map_err(|e| CryptoError::KeyDerivation(e.to_string()))?;

    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut out = [0u8; MASTER_KEY_LEN];

    argon2
        .hash_password_into(passphrase, salt, &mut out)
        .map_err(|e| CryptoError::KeyDerivation(e.to_string()))?;

    Ok(SubKey(out))
}

pub struct DeviceIdentity {
    secret: StaticSecret,
    public: PublicKey,
}

impl DeviceIdentity {
    pub fn generate() -> Self {
        let secret = StaticSecret::random_from_rng(OsRng);
        let public = PublicKey::from(&secret);
        Self { secret, public }
    }

    pub fn from_secret_bytes(bytes: [u8; 32]) -> Self {
        let secret = StaticSecret::from(bytes);
        let public = PublicKey::from(&secret);
        Self { secret, public }
    }

    pub fn public_key_bytes(&self) -> [u8; 32] {
        self.public.to_bytes()
    }

    pub fn export_secret_bytes(&self) -> SubKey {
        SubKey(self.secret.to_bytes())
    }

    pub fn derive_shared_channel(&self, peer_public_bytes: &[u8; 32]) -> SubKey {
        let peer_pub = PublicKey::from(*peer_public_bytes);
        let shared = self.secret.diffie_hellman(&peer_pub);
        let channel_key = blake3::derive_key("ordoc.e2ee.p2p.channel.v1", shared.as_bytes());
        SubKey(channel_key)
    }
}