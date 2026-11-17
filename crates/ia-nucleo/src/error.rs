//! Erros do núcleo de IA.
//!
//! Regra (desenho, seção 7, item 5): nenhum erro leva texto do usuário nem resposta do modelo, nem no
//! `Display` nem no `Debug`. Por isso os erros do rusqlite e do serde_json NÃO são guardados inteiros
//! (a mensagem deles pode trazer trecho do texto: "syntax error near ..." no FTS5, "invalid type:
//! string \"...\"" no serde). Guardamos só o tipo, o código e a posição.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ErroIa {
    /// O motor respondeu com um status HTTP de erro. `motor` é o id configurado do perfil.
    #[error("o motor {motor} respondeu com HTTP {status}")]
    Motor { motor: String, status: u16 },

    /// O motor não respondeu dentro do tempo limite da chamada.
    #[error("tempo esgotado no motor {motor}")]
    TempoEsgotado { motor: String },

    /// Nenhum motor permitido para a tarefa está de pé ou todos falharam.
    #[error("nenhum motor disponível para esta tarefa")]
    SemMotorDisponivel,

    /// Todos os motores tentados devolveram saída fora do esquema.
    #[error("a saída do modelo ficou fora do esquema depois de {tentativas} tentativa(s)")]
    SaidaForaDoEsquema { tentativas: u32 },

    /// Falha do SQLite/SQLCipher. Só o tipo do erro (sem a mensagem, que pode ter texto da consulta).
    #[error("falha na memória local da IA ({tipo})")]
    Memoria { tipo: String },

    /// Falha de transporte HTTP, já sanitizada (categoria fixa).
    #[error("falha de rede com o motor: {0}")]
    Http(&'static str),

    /// A resposta do motor chegou, mas não tem o formato da API (`choices[0].message.content`).
    #[error("resposta do motor em formato inesperado: {0}")]
    RespostaInvalida(&'static str),

    /// O texto tem dado sensível (a categoria vai no erro; o valor, nunca).
    #[error("gravação recusada: o texto contém dado sensível ({0})")]
    DadoSensivelBloqueado(&'static str),

    /// Falha de (de)serialização JSON: só a categoria e a posição.
    #[error("JSON inválido ({categoria}, linha {linha}, coluna {coluna})")]
    Serializacao {
        categoria: &'static str,
        linha: usize,
        coluna: usize,
    },

    /// Receita em Markdown fora do formato (o motivo é fixo, nunca um trecho do arquivo).
    #[error("receita inválida: {0}")]
    ReceitaInvalida(&'static str),

    /// Lição fora do formato "ERRO: ... | CERTO: ...".
    #[error("lição inválida: {0}")]
    LicaoInvalida(&'static str),

    /// Valor inválido para gravar (ex.: hash fora do formato, limite zero).
    #[error("valor inválido: {0}")]
    ValorInvalido(&'static str),

    /// Configuração inválida (ex.: motor local fora de 127.0.0.1).
    #[error("configuração inválida: {0}")]
    Configuracao(&'static str),

    /// Falha de E/S (só o tipo; o caminho não entra).
    #[error("falha de E/S ({0:?})")]
    Io(std::io::ErrorKind),
}

pub type Result<T> = std::result::Result<T, ErroIa>;

impl From<rusqlite::Error> for ErroIa {
    fn from(e: rusqlite::Error) -> Self {
        use rusqlite::Error as E;
        let tipo = match &e {
            E::SqliteFailure(f, _) => format!("{:?}", f.code),
            E::QueryReturnedNoRows => "QueryReturnedNoRows".to_owned(),
            E::InvalidColumnType(..) => "InvalidColumnType".to_owned(),
            E::InvalidColumnIndex(_) => "InvalidColumnIndex".to_owned(),
            E::InvalidColumnName(_) => "InvalidColumnName".to_owned(),
            E::InvalidParameterCount(..) => "InvalidParameterCount".to_owned(),
            E::FromSqlConversionFailure(..) => "FromSqlConversionFailure".to_owned(),
            E::ToSqlConversionFailure(_) => "ToSqlConversionFailure".to_owned(),
            E::IntegralValueOutOfRange(..) => "IntegralValueOutOfRange".to_owned(),
            E::Utf8Error(_) => "Utf8Error".to_owned(),
            E::NulError(_) => "NulError".to_owned(),
            E::ExecuteReturnedResults => "ExecuteReturnedResults".to_owned(),
            _ => "Outro".to_owned(),
        };
        ErroIa::Memoria { tipo }
    }
}

impl From<serde_json::Error> for ErroIa {
    fn from(e: serde_json::Error) -> Self {
        use serde_json::error::Category;
        let categoria = match e.classify() {
            Category::Io => "io",
            Category::Syntax => "sintaxe",
            Category::Data => "dados",
            Category::Eof => "fim inesperado",
        };
        ErroIa::Serializacao {
            categoria,
            linha: e.line(),
            coluna: e.column(),
        }
    }
}

impl From<std::io::Error> for ErroIa {
    fn from(e: std::io::Error) -> Self {
        ErroIa::Io(e.kind())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn erro_do_serde_nao_leva_o_texto_do_modelo() {
        let segredo = "Fulana Inventada";
        let falha = serde_json::from_str::<u32>(&format!("\"{segredo}\"")).unwrap_err();
        let erro: ErroIa = falha.into();
        assert!(!erro.to_string().contains(segredo));
        assert!(!format!("{erro:?}").contains(segredo));
    }

    #[test]
    fn erro_do_sqlite_nao_leva_o_texto_da_consulta() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let falha = conn
            .execute_batch("SELECT * FROM tabela_que_nao_existe_Fulana;")
            .unwrap_err();
        let erro: ErroIa = falha.into();
        assert!(!erro.to_string().contains("Fulana"));
        assert!(!format!("{erro:?}").contains("Fulana"));
    }
}
