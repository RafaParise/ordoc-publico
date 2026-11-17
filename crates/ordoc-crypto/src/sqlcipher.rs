use std::path::Path;
use rusqlite::Connection;
use zeroize::Zeroize;

use crate::{
    error::{CryptoError, Result},
    keys::MasterKey,
};

pub fn open_encrypted_database<P: AsRef<Path>>(
    db_path: P,
    master_key: &MasterKey,
) -> Result<Connection> {
    if let Some(parent) = db_path.as_ref().parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let conn = Connection::open(db_path)?;

    let db_key = master_key.derive_subkey("ordoc.storage.sqlcipher.v1");
    let mut hex_key = hex::encode(db_key.as_bytes());
    let mut pragma_key = format!("PRAGMA key = \"x'{hex_key}'\";");

    let apply_result = conn.execute_batch(&pragma_key);

    hex_key.zeroize();
    pragma_key.zeroize();

    apply_result?;

    conn.execute_batch(
        "
        PRAGMA cipher_memory_security = ON;
        PRAGMA journal_mode = WAL;
        PRAGMA synchronous = NORMAL;
        PRAGMA foreign_keys = ON;
        PRAGMA temp_store = MEMORY;
        ",
    )?;

    let _: i64 = conn
        .query_row("SELECT count(*) FROM sqlite_master;", [], |row| row.get(0))
        .map_err(CryptoError::DatabaseCipher)?;

    Ok(conn)
}