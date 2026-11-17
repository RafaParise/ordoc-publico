//! Painel do aluno por link cifrado (pedido do professor em 16/11/2026, opção "link cifrado").
//!
//! Cada aluno recebe um link `<site>/#<arquivo>.<chave>`. O site estático guarda só `d/<arquivo>.json`, um
//! envelope AES-256-GCM. A chave vai depois do `#`: o navegador nunca a manda ao servidor, então quem hospeda
//! vê só texto cifrado, sem nome. AES-GCM aqui (e não XChaCha20) porque o navegador decifra com a WebCrypto,
//! sem biblioteca de fora.
//!
//! As chaves não ficam guardadas: saem da chave-mestra (`ordoc.portal-aluno.v1`) + id local do aluno + versão.
//! Trocar a versão (rotação) invalida todos os links antigos.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use rand_core::{OsRng, RngCore};
use zeroize::Zeroizing;

use crate::error::{CryptoError, Result};
use crate::keys::MasterKey;

/// Domínio da subchave (regra 3 do contrato).
pub const DOMINIO_PORTAL: &str = "ordoc.portal-aluno.v1";

/// Chave e nome de arquivo de um aluno numa versão dos links.
pub struct ChavePortal {
    /// Nome do arquivo no site (32 hex, não revela o aluno nem a chave).
    pub arquivo: String,
    chave: Zeroizing<[u8; 32]>,
}

impl ChavePortal {
    /// Parte do link depois do `#`: `<arquivo>.<chave em hex>`.
    pub fn fragmento(&self) -> Zeroizing<String> {
        Zeroizing::new(format!("{}.{}", self.arquivo, hex::encode(*self.chave)))
    }

    /// Chave crua (só para testes e para decifrar a prévia local).
    pub fn chave(&self) -> &[u8; 32] {
        &self.chave
    }
}

/// Deriva a chave do aluno. `aluno` é o id LOCAL (nunca vai ao site); `versao` começa em 1.
pub fn chave_portal_aluno(mestra: &MasterKey, aluno: &str, versao: u32) -> ChavePortal {
    let sub = mestra.derive_subkey(DOMINIO_PORTAL);
    let mut entrada = Zeroizing::new(Vec::with_capacity(aluno.len() + 16));
    entrada.extend_from_slice(aluno.as_bytes());
    entrada.push(0);
    entrada.extend_from_slice(&versao.to_le_bytes());
    let mut ctx_chave = Zeroizing::new(b"chave\0".to_vec());
    ctx_chave.extend_from_slice(&entrada);
    let mut ctx_arq = b"arquivo\0".to_vec();
    ctx_arq.extend_from_slice(&entrada);
    let chave = Zeroizing::new(*blake3::keyed_hash(sub.as_bytes(), &ctx_chave).as_bytes());
    let arquivo = hex::encode(&blake3::keyed_hash(sub.as_bytes(), &ctx_arq).as_bytes()[..16]);
    ChavePortal { arquivo, chave }
}

fn aad(arquivo: &str) -> Vec<u8> {
    format!("{DOMINIO_PORTAL}|{arquivo}").into_bytes()
}

/// Cifra o JSON do painel. Saída: `{"v":1,"n":"<nonce hex>","c":"<cifra hex>"}` (o que vai ao site).
pub fn cifrar_painel(chave: &ChavePortal, json: &[u8]) -> Result<String> {
    let cifra = Aes256Gcm::new_from_slice(chave.chave.as_ref()).map_err(|_| CryptoError::InvalidLength)?;
    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut nonce);
    let c = cifra
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: json, aad: &aad(&chave.arquivo) })
        .map_err(|_| CryptoError::AeadFailure)?;
    Ok(format!("{{\"v\":1,\"n\":\"{}\",\"c\":\"{}\"}}", hex::encode(nonce), hex::encode(c)))
}

/// Decifra um envelope (testes e prévia local; o site faz o mesmo em JavaScript).
pub fn decifrar_painel(chave: &ChavePortal, nonce_hex: &str, cifra_hex: &str) -> Result<Zeroizing<Vec<u8>>> {
    let nonce = hex::decode(nonce_hex).map_err(|_| CryptoError::InvalidLength)?;
    let c = hex::decode(cifra_hex).map_err(|_| CryptoError::InvalidLength)?;
    if nonce.len() != 12 {
        return Err(CryptoError::InvalidLength);
    }
    let cifra = Aes256Gcm::new_from_slice(chave.chave.as_ref()).map_err(|_| CryptoError::InvalidLength)?;
    cifra
        .decrypt(Nonce::from_slice(&nonce), Payload { msg: &c, aad: &aad(&chave.arquivo) })
        .map(Zeroizing::new)
        .map_err(|_| CryptoError::AeadFailure)
}

#[cfg(test)]
mod testes {
    use super::*;

    fn campos(env: &str) -> (String, String) {
        let pega = |k: &str| {
            let i = env.find(&format!("\"{k}\":\"")).unwrap() + k.len() + 4;
            env[i..i + env[i..].find('"').unwrap()].to_owned()
        };
        (pega("n"), pega("c"))
    }

    #[test]
    fn ida_e_volta_e_aluno_errado_nao_abre() {
        let m = MasterKey::generate();
        let a = chave_portal_aluno(&m, "aluno-1", 1);
        let env = cifrar_painel(&a, br#"{"turma":"2A"}"#).unwrap();
        let (n, c) = campos(&env);
        assert_eq!(&*decifrar_painel(&a, &n, &c).unwrap(), br#"{"turma":"2A"}"#);
        let b = chave_portal_aluno(&m, "aluno-2", 1);
        assert!(decifrar_painel(&b, &n, &c).is_err());
        assert_ne!(a.arquivo, b.arquivo);
    }

    #[test]
    fn rotacao_troca_arquivo_e_chave_e_e_deterministica() {
        let m = MasterKey::generate();
        let v1 = chave_portal_aluno(&m, "aluno-1", 1);
        let v1b = chave_portal_aluno(&m, "aluno-1", 1);
        let v2 = chave_portal_aluno(&m, "aluno-1", 2);
        assert_eq!(v1.arquivo, v1b.arquivo);
        assert_eq!(v1.chave(), v1b.chave());
        assert_ne!(v1.arquivo, v2.arquivo);
        assert_ne!(v1.chave(), v2.chave());
        assert_eq!(v1.arquivo.len(), 32);
        assert_eq!(v1.fragmento().len(), 32 + 1 + 64);
    }

    #[test]
    fn arquivo_trocado_nao_abre_por_causa_do_aad() {
        let m = MasterKey::generate();
        let a = chave_portal_aluno(&m, "aluno-1", 1);
        let env = cifrar_painel(&a, b"{}").unwrap();
        let (n, c) = campos(&env);
        let outro = ChavePortal { arquivo: "0".repeat(32), chave: Zeroizing::new(*a.chave()) };
        assert!(decifrar_painel(&outro, &n, &c).is_err());
    }
}
