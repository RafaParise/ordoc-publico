use ordoc_core::CoreError;
use ordoc_crypto::CryptoError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("Falha ao abrir o banco cifrado: {0}")]
    Cripto(#[from] CryptoError),

    #[error("Erro do banco SQLCipher: {0}")]
    Banco(#[from] rusqlite::Error),

    #[error("JSON inválido: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Dado inválido: {0}")]
    Dominio(#[from] CoreError),

    #[error("A migração {versao} do esquema falhou: {motivo}")]
    Migracao { versao: u32, motivo: String },

    #[error("O banco está na versão {encontrada} do esquema, mais nova que a suportada ({suportada}): atualize o Ordoc")]
    VersaoFutura { encontrada: u32, suportada: u32 },

    // LGPD: nenhuma variante leva RA ou nome de aluno no texto (Display) nem no Debug; o erro
    // aponta a POSIÇÃO do registro na lista, e a tela, que tem a lista, mostra o aluno localmente.
    #[error("A chamada repete um aluno (registro nº {}): cada aluno entra uma vez", .indice + 1)]
    RegistroDuplicado { indice: usize },

    #[error("O registro nº {} da chamada é de um aluno sem matrícula nesta turma", .indice + 1)]
    AlunoForaDaTurma { indice: usize },

    #[error("A turma '{0}' não está no banco: importe as turmas antes de lançar a chamada")]
    TurmaDesconhecida(String),

    #[error("Indicador da turma '{encontrada}' enviado para a turma '{esperada}'")]
    TurmaDivergente {
        esperada: String,
        encontrada: String,
    },

    #[error("O item {0} da Outbox não está reservado (IN_FLIGHT)")]
    ItemNaoReservado(i64),

    #[error("Falha ao gravar {tipo} nº {} do lote: {causa}", .indice + 1)]
    Registro {
        tipo: &'static str,
        /// Posição do registro na lista do lote (começa em 0).
        indice: usize,
        #[source]
        causa: Box<StorageError>,
    },
}

impl StorageError {
    /// O arquivo existe mas não abre com esta chave (chave errada ou arquivo que não é do Ordoc).
    pub fn chave_incorreta(&self) -> bool {
        let erro_sqlite = match self {
            Self::Cripto(CryptoError::DatabaseCipher(e)) | Self::Banco(e) => e,
            Self::Registro { causa, .. } => return causa.chave_incorreta(),
            _ => return false,
        };
        erro_sqlite.sqlite_error_code() == Some(rusqlite::ErrorCode::NotADatabase)
    }
}

pub type Result<T> = std::result::Result<T, StorageError>;
