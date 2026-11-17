//! ia-nucleo: núcleo genérico de IA do Ordoc e do Univo (documentação interna, seção 4).
//!
//! - `higiene`: `Higienizador` (CPF, RG, e-mail, telefone, RA/números longos e nomes de uma lista recebida
//!   em tempo de execução viram marcadores estáveis) e `MapaMarcadores` para restaurar no fim, só no aparelho.
//! - `motor`: trait `Motor`, `MotorOpenAiCompat` (`/v1/chat/completions`, rustls) e `MotorFalso` (testes).
//! - `roteador`: `Roteador` com a política online/offline da seção 4.1. Texto que vai a motor online
//!   SEMPRE passa pelo `Higienizador` antes.
//! - `esquema`: validação da saída JSON com serde e mensagem de correção para a nova tentativa.
//! - `memoria`, `licao`, `receita`: tabelas `ia_*` com FTS5 sobre uma `rusqlite::Connection` de quem chama;
//!   toda gravação passa pelo `Higienizador` em modo recusa.
//!
//! Sem domínio educacional e sem depender de nenhum outro crate do Ordoc.

#![forbid(unsafe_code)]
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)
)]

pub mod error;
pub mod esquema;
pub mod higiene;
pub mod licao;
pub mod memoria;
pub mod motor;
pub mod receita;
pub mod roteador;

pub use error::{ErroIa, Result};
pub use esquema::{extrair_json, mensagem_correcao, validar_saida, EsquemaSaida, FalhaEsquema};
pub use higiene::{dobrar, Categoria, Higienizador, MapaMarcadores};
pub use licao::{Grau, Licao, OrigemLicao};
pub use memoria::{
    chave_cache, hash_entradas, Achado, LicaoGravada, MemoriaIa, Norma, RegistroExecucao,
    ResultadoExecucao, TextoModelo, Veredito,
};
pub use motor::{
    Alcance, ConfigOpenAiCompat, FuturoIa, Geracao, Mensagem, Motor, MotorFalso, MotorOpenAiCompat,
    Papel, PedidoGeracao, PerfilMotor, RespostaFalsa,
};
pub use receita::{importar_pasta_receitas, RelatorioImportacao, Receita};
pub use roteador::{EstadoMotores, Politica, Resposta, Roteador, TipoTarefa};
