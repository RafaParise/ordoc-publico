use chacha20poly1305::{
    aead::{Aead, AeadCore, KeyInit, OsRng, Payload},
    XChaCha20Poly1305, XNonce,
};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::{
    error::{CryptoError, Result},
    keys::{DeviceIdentity, SubKey},
};

pub const XNONCE_LEN: usize = 24;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SealedBox {
    pub nonce: [u8; XNONCE_LEN],
    pub ciphertext: Vec<u8>,
}

pub fn encrypt_aead(key: &SubKey, plaintext: &[u8], aad: &[u8]) -> Result<SealedBox> {
    let cipher = XChaCha20Poly1305::new(key.as_bytes().into());
    let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);

    let ciphertext = cipher
        .encrypt(&nonce, Payload { msg: plaintext, aad })
        .map_err(|_| CryptoError::AeadFailure)?;

    let mut nonce_arr = [0u8; XNONCE_LEN];
    nonce_arr.copy_from_slice(nonce.as_slice());

    Ok(SealedBox {
        nonce: nonce_arr,
        ciphertext,
    })
}

pub fn decrypt_aead(key: &SubKey, sealed: &SealedBox, aad: &[u8]) -> Result<Vec<u8>> {
    let cipher = XChaCha20Poly1305::new(key.as_bytes().into());
    let nonce = XNonce::from_slice(&sealed.nonce);

    cipher
        .decrypt(
            nonce,
            Payload {
                msg: &sealed.ciphertext,
                aad,
            },
        )
        .map_err(|_| CryptoError::AeadFailure)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct E2eePacket {
    pub ephemeral_sender_pub: [u8; 32],
    pub envelope: SealedBox,
}

pub fn seal_for_peer(
    recipient_public: &[u8; 32],
    plaintext: &[u8],
    aad: &[u8],
) -> Result<E2eePacket> {
    let ephemeral = DeviceIdentity::generate();
    let shared_key = ephemeral.derive_shared_channel(recipient_public);
    let envelope = encrypt_aead(&shared_key, plaintext, aad)?;

    Ok(E2eePacket {
        ephemeral_sender_pub: ephemeral.public_key_bytes(),
        envelope,
    })
}

pub fn open_from_peer(
    recipient_identity: &DeviceIdentity,
    packet: &E2eePacket,
    aad: &[u8],
) -> Result<Vec<u8>> {
    let shared_key = recipient_identity.derive_shared_channel(&packet.ephemeral_sender_pub);
    let mut plaintext = decrypt_aead(&shared_key, &packet.envelope, aad)?;

    if plaintext.is_empty() {
        plaintext.zeroize();
        return Err(CryptoError::InvalidLength);
    }

    Ok(plaintext)
}