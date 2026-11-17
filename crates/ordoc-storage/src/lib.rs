//! ordoc-storage: o banco local cifrado do Ordoc (SQLCipher) e a fila Outbox.
//!
//! - Abre o banco com `ordoc_crypto::open_encrypted_database` (a chave do banco é derivada da chave
//!   mestra lá dentro, no domínio `ordoc.storage.sqlcipher.v1`, regra 3 do regras do projeto).
//! - Aplica as migrações (`migracoes`): idempotentes, controladas por `PRAGMA user_version`.
//! - Repositórios tipados dos modelos do `ordoc-core` (`repositorios`, `riscos`, `telemetria`),
//!   sempre guardando o JSON bruto em `metadata_bruta` (regra 4).
//! - Fila Outbox offline-first (`outbox`): o que o professor salva vai primeiro para o banco como
//!   `PENDING_SYNC`, na mesma transação do dado, e libera a interface na hora.
//!
//! Privacidade (LGPD, alunos menores): `StorageError` nunca leva RA nem nome de aluno no texto nem
//! no `Debug` (aponta a posição do registro). Já `outbox.ultimo_erro` e `telemetria_conectores.detalhe`
//! guardam o texto que a plataforma devolveu (sem a consulta das URLs) e PODEM conter dado de aluno:
//! ficam só no banco cifrado e não vão para a Bancada-Sync nem para log sem um filtro.
//!
//! `BancoOrdoc` é `Send` mas não `Sync` (como a `rusqlite::Connection`): use um por thread, ou um
//! `Mutex`, e no Tokio chame dentro de `spawn_blocking`. Duas conexões no mesmo arquivo funcionam
//! (WAL + transações `IMMEDIATE` + espera de 5 s quando o outro está gravando).

#![forbid(unsafe_code)]
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)
)]

mod error;
mod migracoes;
mod outbox;
mod repositorios;
mod riscos;
mod sql;
mod telemetria;

use std::fmt;
use std::path::Path;
use std::time::Duration;

use ordoc_crypto::{open_encrypted_database, MasterKey};
use rusqlite::Connection;

pub use error::{Result, StorageError};
pub use migracoes::VERSAO_ESQUEMA;
pub use outbox::{
    chave_frequencia, ContagemOutbox, ItemOutbox, MAX_TENTATIVAS_PADRAO, TIPO_FREQUENCIA,
};
pub use repositorios::ResultadoLote;
pub use riscos::{ResultadoRiscos, RiscoRegistrado};
pub use telemetria::EstadoConector;

/// Fuso do professor em minutos (Brasília, UTC−3, sem horário de verão desde o Decreto 9.772/2019).
/// Usado para saber quando vence o prazo de uma atividade que não diz o fuso.
pub const FUSO_PADRAO_MINUTOS: i32 = -180;

/// O banco local do Ordoc.
pub struct BancoOrdoc {
    conn: Connection,
    relogio: Box<dyn Fn() -> i64 + Send>,
    fuso_minutos: i32,
}

impl BancoOrdoc {
    /// Abre (ou cria) o banco cifrado e leva o esquema até `VERSAO_ESQUEMA`.
    /// Chave errada: `StorageError::chave_incorreta()` responde `true`.
    pub fn abrir<P: AsRef<Path>>(caminho: P, chave_mestra: &MasterKey) -> Result<Self> {
        let mut conn = open_encrypted_database(caminho, chave_mestra)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        migracoes::aplicar_migracoes(&mut conn)?;
        Ok(Self {
            conn,
            relogio: Box::new(ordoc_core::agora_ms),
            fuso_minutos: FUSO_PADRAO_MINUTOS,
        })
    }

    pub fn versao_esquema(&self) -> Result<u32> {
        migracoes::versao_atual(&self.conn)
    }

    /// Troca o relógio (milissegundos desde 1970). Serve aos testes e a simulações.
    pub fn definir_relogio(&mut self, relogio: impl Fn() -> i64 + Send + 'static) {
        self.relogio = Box::new(relogio);
    }

    /// A hora do relógio do banco (ms desde 1970). Use para o `inicio_coleta_ms` do `aplicar_lote`,
    /// para ficar no mesmo relógio da Outbox (inclusive com `definir_relogio`).
    pub fn agora_ms(&self) -> i64 {
        self.agora()
    }

    /// Troca o fuso do professor (minutos em relação a UTC, ex.: -240 no Amazonas). Limitado a ±14 h.
    pub fn definir_fuso_minutos(&mut self, minutos: i32) {
        self.fuso_minutos = minutos.clamp(-14 * 60, 14 * 60);
    }

    fn agora(&self) -> i64 {
        (self.relogio)()
    }
}

impl fmt::Debug for BancoOrdoc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BancoOrdoc")
            .field("caminho", &self.conn.path())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
pub(crate) mod apoio {
    //! Ajudantes dos testes. Todos os dados são fictícios.

    use std::path::PathBuf;
    use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
    use std::sync::Arc;

    use ordoc_core::{Aluno, Aula, Matricula, RegistroFrequencia, StatusPresenca, Turma};
    use ordoc_crypto::MasterKey;
    use serde_json::json;

    use crate::BancoOrdoc;

    /// 14/11/2026 00:00 UTC.
    pub(crate) const INICIO: i64 = 1_794_614_400_000;
    pub(crate) const MINUTO: i64 = 60_000;
    pub(crate) const DIA: i64 = 86_400_000;

    static CONTADOR: AtomicU64 = AtomicU64::new(0);

    /// Caminho temporário único; apaga o banco e os arquivos do WAL ao sair.
    pub(crate) struct ArquivoTemporario(pub(crate) PathBuf);

    impl ArquivoTemporario {
        pub(crate) fn novo() -> Self {
            let n = CONTADOR.fetch_add(1, Ordering::Relaxed);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            Self(std::env::temp_dir().join(format!(
                "ordoc_storage_teste_{}_{nanos}_{n}.db",
                std::process::id()
            )))
        }
    }

    impl Drop for ArquivoTemporario {
        fn drop(&mut self) {
            for sufixo in ["", "-wal", "-shm"] {
                let mut nome = self.0.clone().into_os_string();
                nome.push(sufixo);
                let _ = std::fs::remove_file(PathBuf::from(nome));
            }
        }
    }

    /// Banco num arquivo temporário, com relógio controlado. Os campos caem na ordem em que foram
    /// declarados: o banco fecha antes de os arquivos serem apagados.
    pub(crate) struct BancoTeste {
        pub(crate) banco: BancoOrdoc,
        pub(crate) relogio: Arc<AtomicI64>,
        chave: [u8; 32],
        pub(crate) arquivo: ArquivoTemporario,
    }

    impl BancoTeste {
        pub(crate) fn novo() -> Self {
            let arquivo = ArquivoTemporario::novo();
            let chave = MasterKey::generate();
            let bytes = *chave.as_bytes();
            let mut banco = BancoOrdoc::abrir(&arquivo.0, &chave).unwrap();
            let relogio = Arc::new(AtomicI64::new(INICIO));
            let r = Arc::clone(&relogio);
            banco.definir_relogio(move || r.load(Ordering::SeqCst));
            Self {
                banco,
                relogio,
                chave: bytes,
                arquivo,
            }
        }

        pub(crate) fn chave(&self) -> MasterKey {
            MasterKey::from_bytes(self.chave)
        }

        pub(crate) fn avancar(&self, ms: i64) {
            self.relogio.fetch_add(ms, Ordering::SeqCst);
        }

        pub(crate) fn agora(&self) -> i64 {
            self.relogio.load(Ordering::SeqCst)
        }

        /// Turma com alunos fictícios matriculados (números de chamada 1, 2, 3...).
        pub(crate) fn preparar_turma(
            &self,
            turma_id: &str,
            codigo_sdf: Option<&str>,
            ras: &[&str],
        ) {
            self.banco
                .salvar_turma(&turma(turma_id, codigo_sdf))
                .unwrap();
            for (i, ra) in ras.iter().enumerate() {
                self.banco.salvar_aluno(&aluno(ra)).unwrap();
                let numero = u32::try_from(i + 1).unwrap();
                self.banco
                    .salvar_matricula(&matricula(turma_id, ra, numero))
                    .unwrap();
            }
        }
    }

    pub(crate) fn turma(id: &str, codigo_sdf: Option<&str>) -> Turma {
        Turma {
            id: id.into(),
            nome: format!("Turma {id}"),
            ano_letivo: 2026,
            escola: Some("Escola Ficticia".into()),
            codigo_sdf: codigo_sdf.map(Into::into),
            metadata_bruta: json!({"origem": "teste", "extra": {"lista": [1, 2, 3]}}),
        }
    }

    pub(crate) fn aluno(ra: &str) -> Aluno {
        Aluno {
            ra: ra.into(),
            digito_ra: Some("0".into()),
            uf_ra: Some("SP".into()),
            nome: format!("Aluno Ficticio {ra}"),
            situacao_matricula: "ATIVO".into(),
            metadata_bruta: json!({"campoNovoDaSed": true, "numeros": [7, 8]}),
        }
    }

    pub(crate) fn matricula(turma_id: &str, ra: &str, numero: u32) -> Matricula {
        Matricula {
            turma_id: turma_id.into(),
            aluno_ra: ra.into(),
            numero_chamada: Some(numero),
            situacao: "ATIVO".into(),
            metadata_bruta: json!({"codigoMatricula": numero}),
        }
    }

    pub(crate) fn aula(id: &str, turma_id: &str, data: &str, quantidade: u32) -> Aula {
        Aula {
            id: id.into(),
            turma_id: turma_id.into(),
            data: data.into(),
            disciplina: Some("Programacao".into()),
            quantidade,
            metadata_bruta: json!({"idAulaDc": id}),
        }
    }

    pub(crate) fn presenca(aula_id: &str, ra: &str, status: StatusPresenca) -> RegistroFrequencia {
        RegistroFrequencia {
            aula_id: aula_id.into(),
            aluno_ra: ra.into(),
            status,
            metadata_bruta: json!({}),
        }
    }
}

#[cfg(test)]
mod testes {
    use super::apoio::BancoTeste;
    use super::BancoOrdoc;

    #[test]
    fn banco_pode_ir_para_outra_thread() {
        fn exige_send<T: Send>() {}
        exige_send::<BancoOrdoc>();
    }

    #[test]
    fn debug_mostra_so_o_caminho() {
        let t = BancoTeste::novo();
        let texto = format!("{:?}", t.banco);
        assert!(texto.starts_with("BancoOrdoc { caminho: "), "{texto}");
    }
}
