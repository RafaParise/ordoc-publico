use zeroize::Zeroize;

use crate::{
    error::{CryptoError, Result},
    keys::{MasterKey, MASTER_KEY_LEN},
};

const SERVICE_NAME: &str = "com.ordoc.app.vault";
const ACCOUNT_NAME: &str = "ordoc_master_key_v1";

#[cfg(not(target_os = "android"))]
pub fn load_or_create_master_key() -> Result<MasterKey> {
    let entry = keyring::Entry::new(SERVICE_NAME, ACCOUNT_NAME)
        .map_err(|e| CryptoError::KeyStoreError(e.to_string()))?;

    match entry.get_password() {
        Ok(mut hex_secret) => {
            let mut decoded = hex::decode(hex_secret.trim())
                .map_err(|e| CryptoError::KeyStoreError(e.to_string()))?;
            hex_secret.zeroize();

            if decoded.len() != MASTER_KEY_LEN {
                decoded.zeroize();
                return Err(CryptoError::InvalidLength);
            }

            let mut key_bytes = [0u8; MASTER_KEY_LEN];
            key_bytes.copy_from_slice(&decoded);
            decoded.zeroize();

            Ok(MasterKey::from_bytes(key_bytes))
        }
        Err(keyring::Error::NoEntry) => {
            let master = MasterKey::generate();
            let mut hex_secret = hex::encode(master.as_bytes());
            let save_res = entry.set_password(&hex_secret);
            hex_secret.zeroize();

            save_res.map_err(|e| CryptoError::KeyStoreError(e.to_string()))?;
            Ok(master)
        }
        Err(e) => Err(CryptoError::KeyStoreError(e.to_string())),
    }
}

/// Android: a chave-mestra fica embrulhada (AES-GCM) por uma chave do Android Keystore que nunca sai do TEE
/// (StrongBox quando o aparelho tem). O lado Kotlin (`Cofre.kt`, chamado pelo host Tauri por JNI ao abrir o
/// app) desembrulha e entrega os 32 bytes aqui uma vez; eles ficam só na memória do processo.
#[cfg(target_os = "android")]
static CHAVE_ANDROID: std::sync::OnceLock<zeroize::Zeroizing<[u8; MASTER_KEY_LEN]>> = std::sync::OnceLock::new();

/// Recebe a chave desembrulhada pelo Android Keystore. Zera a cópia recebida em qualquer caso.
#[cfg(target_os = "android")]
pub fn instalar_chave_android(bytes: &mut [u8]) -> Result<()> {
    if bytes.len() != MASTER_KEY_LEN {
        bytes.zeroize();
        return Err(CryptoError::InvalidLength);
    }
    let mut chave = zeroize::Zeroizing::new([0u8; MASTER_KEY_LEN]);
    chave.copy_from_slice(bytes);
    bytes.zeroize();
    // já instalada: fica a primeira (a mesma do Keystore)
    let _ = CHAVE_ANDROID.set(chave);
    Ok(())
}

#[cfg(target_os = "android")]
pub fn load_or_create_master_key() -> Result<MasterKey> {
    CHAVE_ANDROID
        .get()
        .map(|c| MasterKey::from_bytes(**c))
        .ok_or_else(|| CryptoError::KeyStoreError("cofre do Android ainda não aberto (reabra o Ordoc)".into()))
}