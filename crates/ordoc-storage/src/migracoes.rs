//! Migrações do esquema do banco local, idempotentes.
//!
//! - A versão aplicada fica no cabeçalho do próprio arquivo (`PRAGMA user_version`).
//! - Cada migração roda numa transação `IMMEDIATE`: entra inteira ou não entra (queda de energia no
//!   meio não deixa o banco pela metade). Se outra conexão migrou enquanto esta esperava a trava, a
//!   versão é conferida de novo dentro da transação e nada é aplicado duas vezes.
//! - `CREATE ... IF NOT EXISTS` em tudo: rodar o SQL de novo não quebra.
//! - Banco de uma versão MAIS NOVA do Ordoc é recusado sem ser tocado.
//! - Tabelas `STRICT` (o SQLite recusa tipo errado) e `CHECK (json_valid(...))` nas colunas de JSON.
//!
//! Para mudar o esquema: acrescente uma `Migracao` com a próxima versão (nunca edite uma publicada)
//! e suba `VERSAO_ESQUEMA`.

use rusqlite::{Connection, TransactionBehavior};

use crate::error::{Result, StorageError};

pub(crate) struct Migracao {
    pub(crate) versao: u32,
    pub(crate) nome: &'static str,
    pub(crate) sql: &'static str,
}

/// Versão do esquema que este código conhece.
pub const VERSAO_ESQUEMA: u32 = 1;

pub(crate) const MIGRACOES: &[Migracao] = &[Migracao {
    versao: 1,
    nome: "esquema_inicial",
    sql: ESQUEMA_V1,
}];

// Sem chave estrangeira entre dados IMPORTADOS (matrículas, aulas, frequências, entregas): a
// plataforma pode mandar um registro cujo "pai" não veio (aluno transferido, coleta parcial) e, como
// o lote é tudo ou nada e o cursor só anda no commit, uma FK travaria a sincronização diferencial
// para sempre. O órfão é guardado (regra 4) e fica fora das contas até o pai chegar. A integridade
// do que o professor cria (`registrar_chamada`) é conferida no código. A v1 ainda não foi publicada
// (nenhum banco real existe), por isso foi editada em vez de ganhar uma v2.
const ESQUEMA_V1: &str = r#"
CREATE TABLE IF NOT EXISTS turmas (
    id             TEXT    NOT NULL PRIMARY KEY,
    nome           TEXT    NOT NULL,
    ano_letivo     INTEGER NOT NULL,
    escola         TEXT,
    codigo_sdf     TEXT,
    metadata_bruta TEXT    NOT NULL CHECK (json_valid(metadata_bruta)),
    atualizado_em  INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS idx_turmas_codigo_sdf ON turmas (codigo_sdf);

CREATE TABLE IF NOT EXISTS alunos (
    ra                 TEXT    NOT NULL PRIMARY KEY,
    digito_ra          TEXT,
    uf_ra              TEXT,
    nome               TEXT    NOT NULL,
    situacao_matricula TEXT    NOT NULL,
    metadata_bruta     TEXT    NOT NULL CHECK (json_valid(metadata_bruta)),
    atualizado_em      INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS matriculas (
    turma_id       TEXT    NOT NULL,
    aluno_ra       TEXT    NOT NULL,
    numero_chamada INTEGER,
    situacao       TEXT    NOT NULL,
    metadata_bruta TEXT    NOT NULL CHECK (json_valid(metadata_bruta)),
    atualizado_em  INTEGER NOT NULL,
    PRIMARY KEY (turma_id, aluno_ra)
) STRICT;
CREATE INDEX IF NOT EXISTS idx_matriculas_aluno ON matriculas (aluno_ra);

CREATE TABLE IF NOT EXISTS aulas (
    id             TEXT    NOT NULL PRIMARY KEY,
    turma_id       TEXT    NOT NULL,
    data           TEXT    NOT NULL CHECK (data GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]'),
    disciplina     TEXT,
    quantidade     INTEGER NOT NULL CHECK (quantidade >= 1),
    metadata_bruta TEXT    NOT NULL CHECK (json_valid(metadata_bruta)),
    atualizado_em  INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS idx_aulas_turma_data ON aulas (turma_id, data);

CREATE TABLE IF NOT EXISTS frequencias (
    aula_id        TEXT    NOT NULL,
    aluno_ra       TEXT    NOT NULL,
    status         TEXT    NOT NULL CHECK (status IN ('PRESENTE', 'FALTA', 'FALTA_JUSTIFICADA')),
    metadata_bruta TEXT    NOT NULL CHECK (json_valid(metadata_bruta)),
    atualizado_em  INTEGER NOT NULL,
    PRIMARY KEY (aula_id, aluno_ra)
) STRICT;
CREATE INDEX IF NOT EXISTS idx_frequencias_aluno ON frequencias (aluno_ra);

CREATE TABLE IF NOT EXISTS atividades_sdf (
    id               TEXT    NOT NULL PRIMARY KEY,
    turma_codigo_sdf TEXT    NOT NULL,
    titulo           TEXT    NOT NULL,
    prazo_iso        TEXT,
    total_alunos     INTEGER NOT NULL CHECK (total_alunos >= 0),
    total_entregues  INTEGER NOT NULL CHECK (total_entregues >= 0),
    metadata_bruta   TEXT    NOT NULL CHECK (json_valid(metadata_bruta)),
    atualizado_em    INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS idx_atividades_sdf_turma ON atividades_sdf (turma_codigo_sdf);

-- A Sala do Futuro pode trazer um RA antes de o DC sincronizar: guardado mesmo assim.
CREATE TABLE IF NOT EXISTS entregas_sdf (
    atividade_id   TEXT    NOT NULL,
    aluno_ra       TEXT    NOT NULL,
    entregue       INTEGER NOT NULL CHECK (entregue IN (0, 1)),
    entregue_em    TEXT,
    metadata_bruta TEXT    NOT NULL CHECK (json_valid(metadata_bruta)),
    atualizado_em  INTEGER NOT NULL,
    PRIMARY KEY (atividade_id, aluno_ra)
) STRICT;
CREATE INDEX IF NOT EXISTS idx_entregas_sdf_aluno ON entregas_sdf (aluno_ra);

-- Histórico das coletas dos painéis: cada coleta acrescenta linhas (a mais recente é a que vale).
CREATE TABLE IF NOT EXISTS indicadores_bi (
    id             INTEGER NOT NULL PRIMARY KEY,
    painel         TEXT    NOT NULL,
    turma_id       TEXT,
    aluno_ra       TEXT,
    coletado_em    INTEGER NOT NULL,
    metadata_bruta TEXT    NOT NULL CHECK (json_valid(metadata_bruta))
) STRICT;
CREATE INDEX IF NOT EXISTS idx_indicadores_bi_painel ON indicadores_bi (painel, coletado_em);

CREATE TABLE IF NOT EXISTS indicadores_risco (
    regra         TEXT    NOT NULL,
    turma_id      TEXT    NOT NULL,
    aluno_ra      TEXT    NOT NULL,
    severidade    TEXT    NOT NULL CHECK (severidade IN ('ATENCAO', 'CRITICO')),
    mensagem      TEXT    NOT NULL,
    evidencias    TEXT    NOT NULL CHECK (json_valid(evidencias)),
    detectado_em  INTEGER NOT NULL,
    atualizado_em INTEGER NOT NULL,
    resolvido_em  INTEGER,
    PRIMARY KEY (regra, turma_id, aluno_ra)
) STRICT;
CREATE INDEX IF NOT EXISTS idx_indicadores_risco_ativos
    ON indicadores_risco (turma_id) WHERE resolvido_em IS NULL;

-- Aulas que a importação pulou (chamada local protegida) ou cuja chamada local foi descartada: a
-- coleta diferencial não as traz de novo (o cursor andou), então o worker as busca pelo id quando a
-- proteção acabar (`aulas_a_recoletar`). A marca some quando a aula volta a ser importada.
CREATE TABLE IF NOT EXISTS aulas_a_recoletar (
    aula_id    TEXT    NOT NULL PRIMARY KEY,
    plataforma TEXT    NOT NULL CHECK (plataforma IN ('dc', 'sdf', 'bi')),
    marcado_em INTEGER NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS telemetria_conectores (
    plataforma    TEXT    NOT NULL PRIMARY KEY CHECK (plataforma IN ('dc', 'sdf', 'bi')),
    status        TEXT    NOT NULL
                  CHECK (status IN ('OK', 'SESSAO_EXPIRADA', 'LAYOUT_ALTERADO', 'TIMEOUT', 'FALHA')),
    detalhe       TEXT,
    registrado_em INTEGER NOT NULL,
    ultimo_ok_em  INTEGER
) STRICT;

CREATE TABLE IF NOT EXISTS cursores_sync (
    plataforma    TEXT    NOT NULL PRIMARY KEY CHECK (plataforma IN ('dc', 'sdf', 'bi')),
    valor         TEXT    NOT NULL,
    atualizado_em INTEGER NOT NULL
) STRICT;

-- Fila Outbox (offline-first): o que o professor salva entra aqui como PENDING_SYNC na mesma
-- transação do dado, e o worker de sincronização envia depois.
CREATE TABLE IF NOT EXISTS outbox (
    id                   INTEGER NOT NULL PRIMARY KEY,
    chave_idempotencia   TEXT    NOT NULL UNIQUE,
    plataforma           TEXT    NOT NULL CHECK (plataforma IN ('dc', 'sdf', 'bi')),
    tipo                 TEXT    NOT NULL,
    payload              TEXT    NOT NULL CHECK (json_valid(payload)),
    status               TEXT    NOT NULL DEFAULT 'PENDING_SYNC'
                         CHECK (status IN ('PENDING_SYNC', 'IN_FLIGHT', 'SYNCED', 'FAILED')),
    versao               INTEGER NOT NULL DEFAULT 1 CHECK (versao >= 1),
    tentativas           INTEGER NOT NULL DEFAULT 0 CHECK (tentativas >= 0),
    proxima_tentativa_em INTEGER NOT NULL,
    ultimo_erro          TEXT,
    criado_em            INTEGER NOT NULL,
    atualizado_em        INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS idx_outbox_fila ON outbox (status, proxima_tentativa_em);
"#;

pub(crate) fn versao_atual(conn: &Connection) -> Result<u32> {
    let v: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    u32::try_from(v).map_err(|_| StorageError::Migracao {
        versao: 0,
        motivo: format!("user_version inválido no arquivo: {v}"),
    })
}

/// Leva o banco até `VERSAO_ESQUEMA`. Pode ser chamada quantas vezes quiser.
pub(crate) fn aplicar_migracoes(conn: &mut Connection) -> Result<u32> {
    let futura = |encontrada| StorageError::VersaoFutura {
        encontrada,
        suportada: VERSAO_ESQUEMA,
    };
    let atual = versao_atual(conn)?;
    if atual > VERSAO_ESQUEMA {
        return Err(futura(atual));
    }
    for m in MIGRACOES.iter().filter(|m| m.versao > atual) {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if versao_atual(&tx)? >= m.versao {
            continue; // outra conexão já aplicou; a transação vazia é desfeita ao sair
        }
        tx.execute_batch(m.sql)
            .map_err(|e| StorageError::Migracao {
                versao: m.versao,
                motivo: format!("{}: {e}", m.nome),
            })?;
        tx.pragma_update(None, "user_version", m.versao)?;
        tx.commit()?;
    }
    let final_ = versao_atual(conn)?;
    if final_ > VERSAO_ESQUEMA {
        return Err(futura(final_));
    }
    Ok(final_)
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::apoio::{turma, BancoTeste};
    use crate::BancoOrdoc;
    use ordoc_crypto::MasterKey;

    #[test]
    fn migracoes_em_ordem_e_a_ultima_e_a_versao_do_esquema() {
        let versoes: Vec<u32> = MIGRACOES.iter().map(|m| m.versao).collect();
        let esperado: Vec<u32> = (1..=VERSAO_ESQUEMA).collect();
        assert_eq!(versoes, esperado);
    }

    #[test]
    fn migracao_e_idempotente() {
        let mut t = BancoTeste::novo();
        assert_eq!(t.banco.versao_esquema().unwrap(), VERSAO_ESQUEMA);
        // De novo na mesma conexão: nada muda.
        assert_eq!(
            aplicar_migracoes(&mut t.banco.conn).unwrap(),
            VERSAO_ESQUEMA
        );
        // O SQL da v1 de novo (IF NOT EXISTS) também não quebra nem apaga dado.
        t.banco.salvar_turma(&turma("T1", None)).unwrap();
        t.banco.conn.execute_batch(MIGRACOES[0].sql).unwrap();
        // Outra conexão no mesmo arquivo, com a mesma chave: mesma versão, dado preservado.
        let outra = BancoOrdoc::abrir(&t.arquivo.0, &t.chave()).unwrap();
        assert_eq!(outra.versao_esquema().unwrap(), VERSAO_ESQUEMA);
        assert!(outra.obter_turma("T1").unwrap().is_some());
    }

    #[test]
    fn banco_de_versao_mais_nova_e_recusado_sem_ser_tocado() {
        let t = BancoTeste::novo();
        t.banco
            .conn
            .pragma_update(None, "user_version", VERSAO_ESQUEMA + 1)
            .unwrap();
        let erro = BancoOrdoc::abrir(&t.arquivo.0, &t.chave()).unwrap_err();
        assert!(
            matches!(erro, StorageError::VersaoFutura { encontrada, suportada }
                if encontrada == VERSAO_ESQUEMA + 1 && suportada == VERSAO_ESQUEMA),
            "{erro}"
        );
        assert_eq!(versao_atual(&t.banco.conn).unwrap(), VERSAO_ESQUEMA + 1);
    }

    #[test]
    fn chave_errada_nao_abre() {
        let t = BancoTeste::novo();
        let erro = BancoOrdoc::abrir(&t.arquivo.0, &MasterKey::generate()).unwrap_err();
        assert!(erro.chave_incorreta(), "{erro}");
    }

    #[test]
    fn tabelas_recusam_json_invalido_e_tipo_errado() {
        let t = BancoTeste::novo();
        let sql = "INSERT INTO turmas (id, nome, ano_letivo, metadata_bruta, atualizado_em)
                   VALUES (?1, 'x', ?2, ?3, 0)";
        assert!(t
            .banco
            .conn
            .execute(sql, rusqlite::params!["T8", 2026, "{nao e json"])
            .is_err());
        assert!(
            t.banco
                .conn
                .execute(sql, rusqlite::params!["T9", "dois mil", "{}"])
                .is_err(),
            "STRICT recusa texto em coluna INTEGER"
        );
    }
}
