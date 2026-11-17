pub mod cipher;
pub mod error;
pub mod keys;
pub mod portal;
pub mod sqlcipher;
pub mod vault;

pub use cipher::{decrypt_aead, encrypt_aead, open_from_peer, seal_for_peer, E2eePacket, SealedBox};
pub use error::{CryptoError, Result};
pub use keys::{
    derive_key_argon2id, generate_salt, DeviceIdentity, MasterKey, SecretString, SubKey,
    MASTER_KEY_LEN, SALT_LEN,
};
pub use portal::{chave_portal_aluno, cifrar_painel, decifrar_painel, ChavePortal, DOMINIO_PORTAL};
pub use sqlcipher::open_encrypted_database;
pub use vault::load_or_create_master_key;
#[cfg(target_os = "android")]
pub use vault::instalar_chave_android;

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn test_aead_roundtrip_and_aad_tamper_detection() {
        let master = MasterKey::generate();
        let cookie_key = master.derive_subkey("ordoc.extract.cookies.v1");

        let payload = br#"{"session_id":"sed_998877","csrf":"abc123xyz"}"#;
        let aad = b"connector:sed:professor";

        let sealed = encrypt_aead(&cookie_key, payload, aad).expect("falha ao cifrar");
        let decrypted = decrypt_aead(&cookie_key, &sealed, aad).expect("falha ao decifrar");
        assert_eq!(decrypted, payload);

        let wrong_aad = b"connector:bi:professor";
        assert!(decrypt_aead(&cookie_key, &sealed, wrong_aad).is_err());
    }

    #[test]
    fn test_e2ee_peer_sync_packet() {
        let pc_identity = DeviceIdentity::generate();
        let android_identity = DeviceIdentity::generate();

        let attendance_batch = br#"{"turma":"3B","data":"2026-11-14","presentes":[1,2,4,5]}"#;
        let aad = b"sync:attendance:v1";

        let packet = seal_for_peer(&pc_identity.public_key_bytes(), attendance_batch, aad)
            .expect("falha ao selar pacote E2EE");

        let opened = open_from_peer(&pc_identity, &packet, aad)
            .expect("PC deveria abrir o pacote destinado a ele");
        assert_eq!(opened, attendance_batch);

        assert!(open_from_peer(&android_identity, &packet, aad).is_err());
    }

    #[test]
    fn test_argon2id_backup_derivation() {
        let salt = generate_salt();
        let k1 = derive_key_argon2id(b"SenhaForteProfessor#2026", &salt).unwrap();
        let k2 = derive_key_argon2id(b"SenhaForteProfessor#2026", &salt).unwrap();
        let k3 = derive_key_argon2id(b"SenhaErrada", &salt).unwrap();

        assert_eq!(k1.as_bytes(), k2.as_bytes());
        assert_ne!(k1.as_bytes(), k3.as_bytes());
    }

    #[test]
    fn test_sqlcipher_database_isolation() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let db_path = std::env::temp_dir().join(format!("ordoc_test_{nanos}.db"));

        let valid_key = MasterKey::generate();
        let wrong_key = MasterKey::generate();

        {
            let conn = open_encrypted_database(&db_path, &valid_key)
                .expect("falha ao criar banco SQLCipher");
            conn.execute(
                "CREATE TABLE alunos (ra TEXT PRIMARY KEY, nome TEXT NOT NULL);",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO alunos (ra, nome) VALUES (?1, ?2);",
                ["109283746-SP", "Aluno Teste"],
            )
            .unwrap();
        }

        assert!(open_encrypted_database(&db_path, &wrong_key).is_err());

        {
            let conn = open_encrypted_database(&db_path, &valid_key)
                .expect("falha ao reabrir banco válido");
            let nome: String = conn
                .query_row(
                    "SELECT nome FROM alunos WHERE ra = ?1;",
                    ["109283746-SP"],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(nome, "Aluno Teste");
        }

        let _ = std::fs::remove_file(db_path);
    }
}