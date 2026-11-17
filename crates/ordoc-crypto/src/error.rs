use thiserror::Error;

#[derive(Debug, Error)]
pub enum CryptoError {
    #[error("Falha na operação criptográfica AEAD (dados corrompidos ou chave inválida)")]
    AeadFailure,

    #[error("Tamanho de payload ou nonce inválido")]
    InvalidLength,

    #[error("Falha na derivação de chave Argon2id: {0}")]
    KeyDerivation(String),

    #[error("Falha ao acessar o cofre de hardware do sistema operacional: {0}")]
    KeyStoreError(String),

    #[error("Falha ao inicializar ou destravar banco SQLCipher: {0}")]
    DatabaseCipher(#[from] rusqlite::Error),

    #[error("Erro de I/O no armazenamento seguro: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, CryptoError>;