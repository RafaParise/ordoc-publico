//! Repositórios tipados dos modelos do ordoc-core.
//!
//! - Tudo o que vem de uma plataforma guarda o JSON bruto integral em `metadata_bruta` (regra 4).
//! - Gravação sempre por `INSERT ... ON CONFLICT DO UPDATE` (upsert). Nunca `INSERT OR REPLACE`: ele
//!   apaga a linha antes de inserir (e, pelo `ON DELETE CASCADE`, os indicadores de risco da turma).
//! - Importação nunca falha por registro órfão (sem FK entre dados importados, ver `migracoes`).
//! - Duas formas de gravar aula e frequência:
//!   - IMPORTADA (veio da plataforma, `aplicar_lote` e `salvar_aula`): substitui tudo, inclusive o JSON bruto;
//!   - LOCAL (o professor lançou no Ordoc, `registrar_chamada`): muda os campos, mas preserva o JSON
//!     bruto que veio do DC (ex.: o id do lançamento, que o conector usa na idempotência).
//! - Chamada local ainda não sincronizada, ou sincronizada depois do início da coleta, vence a
//!   importação: o DC (ou um lote lido antes do envio) não desfaz o que o professor fez.
//! - LGPD: erro de gravação aponta a posição do registro no lote, nunca o RA.

use std::collections::{HashMap, HashSet};

use ordoc_core::{
    Aluno, AtividadeSdf, Aula, CoreError, EntregaSdf, IndicadorBi, LoteCanonico, Matricula,
    OperacaoRemota, Plataforma, RegistroFrequencia, StatusPresenca, Turma,
};
use rusqlite::{params, Connection, Row, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Result, StorageError};
use crate::outbox::{self, chave_frequencia, TIPO_FREQUENCIA};
use crate::sql::{enum_da_coluna, json_da_coluna, json_texto, listar, obter};
use crate::telemetria::upsert_cursor;
use crate::BancoOrdoc;

/// O que uma importação (`aplicar_lote`) gravou.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultadoLote {
    pub registros_gravados: usize,
    /// Aulas com chamada local ainda não sincronizada: a importação não as tocou.
    pub aulas_preservadas: usize,
    pub frequencias_preservadas: usize,
}

// ------------------------------------------------------------------ gravação
// Funções livres: servem tanto à conexão quanto a uma transação (que é uma `Connection`).

fn upsert_turma(conn: &Connection, t: &Turma, agora: i64) -> Result<()> {
    t.validar()?;
    conn.prepare_cached(
        "INSERT INTO turmas (id, nome, ano_letivo, escola, codigo_sdf, metadata_bruta, atualizado_em)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (id) DO UPDATE SET
             nome = excluded.nome,
             ano_letivo = excluded.ano_letivo,
             escola = excluded.escola,
             codigo_sdf = COALESCE(excluded.codigo_sdf, turmas.codigo_sdf),
             metadata_bruta = excluded.metadata_bruta,
             atualizado_em = excluded.atualizado_em",
    )?
    .execute(params![
        t.id,
        t.nome,
        t.ano_letivo,
        t.escola,
        t.codigo_sdf,
        json_texto(&t.metadata_bruta)?,
        agora
    ])?;
    Ok(())
}

fn upsert_aluno(conn: &Connection, a: &Aluno, agora: i64) -> Result<()> {
    a.validar()?;
    conn.prepare_cached(
        "INSERT INTO alunos (ra, digito_ra, uf_ra, nome, situacao_matricula, metadata_bruta, atualizado_em)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (ra) DO UPDATE SET
             digito_ra = excluded.digito_ra,
             uf_ra = excluded.uf_ra,
             nome = excluded.nome,
             situacao_matricula = excluded.situacao_matricula,
             metadata_bruta = excluded.metadata_bruta,
             atualizado_em = excluded.atualizado_em",
    )?
    .execute(params![
        a.ra,
        a.digito_ra,
        a.uf_ra,
        a.nome,
        a.situacao_matricula,
        json_texto(&a.metadata_bruta)?,
        agora
    ])?;
    Ok(())
}

fn upsert_matricula(conn: &Connection, m: &Matricula, agora: i64) -> Result<()> {
    m.validar()?;
    conn.prepare_cached(
        "INSERT INTO matriculas (turma_id, aluno_ra, numero_chamada, situacao, metadata_bruta, atualizado_em)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (turma_id, aluno_ra) DO UPDATE SET
             numero_chamada = excluded.numero_chamada,
             situacao = excluded.situacao,
             metadata_bruta = excluded.metadata_bruta,
             atualizado_em = excluded.atualizado_em",
    )?
    .execute(params![
        m.turma_id,
        m.aluno_ra,
        m.numero_chamada,
        m.situacao,
        json_texto(&m.metadata_bruta)?,
        agora
    ])?;
    Ok(())
}

/// Aula vinda da plataforma: substitui tudo, inclusive o JSON bruto.
fn upsert_aula_importada(conn: &Connection, a: &Aula, agora: i64) -> Result<()> {
    a.validar()?;
    conn.prepare_cached(
        "INSERT INTO aulas (id, turma_id, data, disciplina, quantidade, metadata_bruta, atualizado_em)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (id) DO UPDATE SET
             turma_id = excluded.turma_id,
             data = excluded.data,
             disciplina = excluded.disciplina,
             quantidade = excluded.quantidade,
             metadata_bruta = excluded.metadata_bruta,
             atualizado_em = excluded.atualizado_em",
    )?
    .execute(params![
        a.id,
        a.turma_id,
        a.data,
        a.disciplina,
        a.quantidade,
        json_texto(&a.metadata_bruta)?,
        agora
    ])?;
    Ok(())
}

/// Aula lançada no Ordoc: atualiza os campos e preserva o JSON bruto que veio do DC.
fn upsert_aula_local(conn: &Connection, a: &Aula, agora: i64) -> Result<()> {
    a.validar()?;
    conn.prepare_cached(
        "INSERT INTO aulas (id, turma_id, data, disciplina, quantidade, metadata_bruta, atualizado_em)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (id) DO UPDATE SET
             turma_id = excluded.turma_id,
             data = excluded.data,
             disciplina = excluded.disciplina,
             quantidade = excluded.quantidade,
             atualizado_em = excluded.atualizado_em",
    )?
    .execute(params![
        a.id,
        a.turma_id,
        a.data,
        a.disciplina,
        a.quantidade,
        json_texto(&a.metadata_bruta)?,
        agora
    ])?;
    Ok(())
}

/// Frequência vinda da plataforma: substitui tudo.
fn upsert_frequencia_importada(
    conn: &Connection,
    f: &RegistroFrequencia,
    agora: i64,
) -> Result<()> {
    f.validar()?;
    conn.prepare_cached(
        "INSERT INTO frequencias (aula_id, aluno_ra, status, metadata_bruta, atualizado_em)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (aula_id, aluno_ra) DO UPDATE SET
             status = excluded.status,
             metadata_bruta = excluded.metadata_bruta,
             atualizado_em = excluded.atualizado_em",
    )?
    .execute(params![
        f.aula_id,
        f.aluno_ra,
        f.status.as_str(),
        json_texto(&f.metadata_bruta)?,
        agora
    ])?;
    Ok(())
}

/// Frequência lançada no Ordoc: muda o status e preserva o JSON bruto do DC.
fn upsert_frequencia_local(conn: &Connection, f: &RegistroFrequencia, agora: i64) -> Result<()> {
    f.validar()?;
    conn.prepare_cached(
        "INSERT INTO frequencias (aula_id, aluno_ra, status, metadata_bruta, atualizado_em)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (aula_id, aluno_ra) DO UPDATE SET
             status = excluded.status,
             atualizado_em = excluded.atualizado_em",
    )?
    .execute(params![
        f.aula_id,
        f.aluno_ra,
        f.status.as_str(),
        json_texto(&f.metadata_bruta)?,
        agora
    ])?;
    Ok(())
}

fn upsert_atividade(conn: &Connection, a: &AtividadeSdf, agora: i64) -> Result<()> {
    a.validar()?;
    conn.prepare_cached(
        "INSERT INTO atividades_sdf (id, turma_codigo_sdf, titulo, prazo_iso, total_alunos,
                                     total_entregues, metadata_bruta, atualizado_em)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT (id) DO UPDATE SET
             turma_codigo_sdf = excluded.turma_codigo_sdf,
             titulo = excluded.titulo,
             prazo_iso = excluded.prazo_iso,
             total_alunos = excluded.total_alunos,
             total_entregues = excluded.total_entregues,
             metadata_bruta = excluded.metadata_bruta,
             atualizado_em = excluded.atualizado_em",
    )?
    .execute(params![
        a.id,
        a.turma_codigo_sdf,
        a.titulo,
        a.prazo_iso,
        a.total_alunos,
        a.total_entregues,
        json_texto(&a.metadata_bruta)?,
        agora
    ])?;
    Ok(())
}

fn upsert_entrega(conn: &Connection, e: &EntregaSdf, agora: i64) -> Result<()> {
    e.validar()?;
    conn.prepare_cached(
        "INSERT INTO entregas_sdf (atividade_id, aluno_ra, entregue, entregue_em, metadata_bruta, atualizado_em)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (atividade_id, aluno_ra) DO UPDATE SET
             entregue = excluded.entregue,
             entregue_em = excluded.entregue_em,
             metadata_bruta = excluded.metadata_bruta,
             atualizado_em = excluded.atualizado_em",
    )?
    .execute(params![
        e.atividade_id,
        e.aluno_ra,
        e.entregue,
        e.entregue_em,
        json_texto(&e.metadata_bruta)?,
        agora
    ])?;
    Ok(())
}

fn inserir_indicador_bi(conn: &Connection, i: &IndicadorBi) -> Result<()> {
    i.validar()?;
    conn.prepare_cached(
        "INSERT INTO indicadores_bi (painel, turma_id, aluno_ra, coletado_em, metadata_bruta)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?
    .execute(params![
        i.painel,
        i.turma_id,
        i.aluno_ra,
        i.coletado_em_ms,
        json_texto(&i.metadata_bruta)?
    ])?;
    Ok(())
}

/// A chamada local desta aula deve ser protegida da importação? Sim se ela ainda não foi confirmada
/// pelo DC (pendente, em envio ou com falha) OU se mudou/foi enviada depois de `inicio_coleta`
/// (o lote pode ter sido lido antes de o DC receber a chamada).
fn chamada_local_protegida<'a>(
    conn: &Connection,
    cache: &mut HashMap<&'a str, bool>,
    aula_id: &'a str,
    inicio_coleta: i64,
) -> Result<bool> {
    if let Some(&protegida) = cache.get(aula_id) {
        return Ok(protegida);
    }
    let protegida: bool = conn
        .prepare_cached(
            "SELECT EXISTS (SELECT 1 FROM outbox WHERE chave_idempotencia = ?1
                            AND (status <> 'SYNCED' OR atualizado_em >= ?2))",
        )?
        .query_row(params![chave_frequencia(aula_id), inicio_coleta], |r| {
            r.get(0)
        })?;
    cache.insert(aula_id, protegida);
    Ok(protegida)
}

/// Põe a aula na lista das que precisam ser buscadas de novo pelo id (ver `aulas_a_recoletar`).
pub(crate) fn marcar_recoleta(
    conn: &Connection,
    aula_id: &str,
    plataforma: Plataforma,
    agora: i64,
) -> Result<()> {
    conn.prepare_cached(
        "INSERT INTO aulas_a_recoletar (aula_id, plataforma, marcado_em) VALUES (?1, ?2, ?3)
         ON CONFLICT (aula_id) DO NOTHING",
    )?
    .execute(params![aula_id, plataforma.as_str(), agora])?;
    Ok(())
}

/// Diz qual registro do lote falhou pela posição na lista (nunca pelo RA: LGPD).
fn com_contexto<T>(resultado: Result<T>, tipo: &'static str, indice: usize) -> Result<T> {
    resultado.map_err(|causa| StorageError::Registro {
        tipo,
        indice,
        causa: Box::new(causa),
    })
}

// ------------------------------------------------------------------ leitura

fn turma_da_linha(row: &Row<'_>) -> rusqlite::Result<Turma> {
    Ok(Turma {
        id: row.get(0)?,
        nome: row.get(1)?,
        ano_letivo: row.get(2)?,
        escola: row.get(3)?,
        codigo_sdf: row.get(4)?,
        metadata_bruta: json_da_coluna(row, 5)?,
    })
}

fn aluno_da_linha(row: &Row<'_>) -> rusqlite::Result<Aluno> {
    Ok(Aluno {
        ra: row.get(0)?,
        digito_ra: row.get(1)?,
        uf_ra: row.get(2)?,
        nome: row.get(3)?,
        situacao_matricula: row.get(4)?,
        metadata_bruta: json_da_coluna(row, 5)?,
    })
}

fn matricula_da_linha(row: &Row<'_>) -> rusqlite::Result<Matricula> {
    Ok(Matricula {
        turma_id: row.get(0)?,
        aluno_ra: row.get(1)?,
        numero_chamada: row.get(2)?,
        situacao: row.get(3)?,
        metadata_bruta: json_da_coluna(row, 4)?,
    })
}

fn aula_da_linha(row: &Row<'_>) -> rusqlite::Result<Aula> {
    Ok(Aula {
        id: row.get(0)?,
        turma_id: row.get(1)?,
        data: row.get(2)?,
        disciplina: row.get(3)?,
        quantidade: row.get(4)?,
        metadata_bruta: json_da_coluna(row, 5)?,
    })
}

fn frequencia_da_linha(row: &Row<'_>) -> rusqlite::Result<RegistroFrequencia> {
    Ok(RegistroFrequencia {
        aula_id: row.get(0)?,
        aluno_ra: row.get(1)?,
        status: enum_da_coluna(row, 2, StatusPresenca::parse)?,
        metadata_bruta: json_da_coluna(row, 3)?,
    })
}

fn atividade_da_linha(row: &Row<'_>) -> rusqlite::Result<AtividadeSdf> {
    Ok(AtividadeSdf {
        id: row.get(0)?,
        turma_codigo_sdf: row.get(1)?,
        titulo: row.get(2)?,
        prazo_iso: row.get(3)?,
        total_alunos: row.get(4)?,
        total_entregues: row.get(5)?,
        metadata_bruta: json_da_coluna(row, 6)?,
    })
}

fn entrega_da_linha(row: &Row<'_>) -> rusqlite::Result<EntregaSdf> {
    Ok(EntregaSdf {
        atividade_id: row.get(0)?,
        aluno_ra: row.get(1)?,
        entregue: row.get(2)?,
        entregue_em: row.get(3)?,
        metadata_bruta: json_da_coluna(row, 4)?,
    })
}

fn indicador_bi_da_linha(row: &Row<'_>) -> rusqlite::Result<IndicadorBi> {
    Ok(IndicadorBi {
        painel: row.get(0)?,
        turma_id: row.get(1)?,
        aluno_ra: row.get(2)?,
        coletado_em_ms: row.get(3)?,
        metadata_bruta: json_da_coluna(row, 4)?,
    })
}

/// O que vai para o conector do DC numa chamada: a aula e os registros como estão no banco
/// (com o JSON bruto do DC preservado), em ordem de RA.
#[derive(Serialize)]
struct PayloadChamada<'a> {
    aula: &'a Aula,
    registros: Vec<RegistroPayload<'a>>,
}

#[derive(Serialize)]
struct RegistroPayload<'a> {
    aluno_ra: &'a str,
    status: StatusPresenca,
    metadata_bruta: &'a Value,
}

fn payload_chamada(conn: &Connection, aula_id: &str) -> Result<Value> {
    let aula = conn
        .prepare_cached(
            "SELECT id, turma_id, data, disciplina, quantidade, metadata_bruta FROM aulas WHERE id = ?1",
        )?
        .query_row([aula_id], aula_da_linha)?;
    let registros = listar(
        conn,
        "SELECT aula_id, aluno_ra, status, metadata_bruta FROM frequencias
         WHERE aula_id = ?1 ORDER BY aluno_ra",
        [aula_id],
        frequencia_da_linha,
    )?;
    let payload = PayloadChamada {
        aula: &aula,
        registros: registros
            .iter()
            .map(|r| RegistroPayload {
                aluno_ra: &r.aluno_ra,
                status: r.status,
                metadata_bruta: &r.metadata_bruta,
            })
            .collect(),
    };
    Ok(serde_json::to_value(payload)?)
}

impl BancoOrdoc {
    // -------------------------------------------------------------- turmas e alunos

    pub fn salvar_turma(&self, turma: &Turma) -> Result<()> {
        upsert_turma(&self.conn, turma, self.agora())
    }

    /// Liga (ou desliga, com `None`) a turma ao código dela na Sala do Futuro.
    /// A importação do DC nunca apaga esta associação (só a troca se trouxer outro código).
    pub fn associar_codigo_sdf(&self, turma_id: &str, codigo_sdf: Option<&str>) -> Result<bool> {
        let n = self
            .conn
            .prepare_cached("UPDATE turmas SET codigo_sdf = ?2, atualizado_em = ?3 WHERE id = ?1")?
            .execute(params![turma_id, codigo_sdf, self.agora()])?;
        Ok(n > 0)
    }

    pub fn obter_turma(&self, id: &str) -> Result<Option<Turma>> {
        obter(
            &self.conn,
            "SELECT id, nome, ano_letivo, escola, codigo_sdf, metadata_bruta FROM turmas WHERE id = ?1",
            [id],
            turma_da_linha,
        )
    }

    pub fn listar_turmas(&self) -> Result<Vec<Turma>> {
        listar(
            &self.conn,
            "SELECT id, nome, ano_letivo, escola, codigo_sdf, metadata_bruta FROM turmas
             ORDER BY ano_letivo DESC, nome, id",
            [],
            turma_da_linha,
        )
    }

    pub fn salvar_aluno(&self, aluno: &Aluno) -> Result<()> {
        upsert_aluno(&self.conn, aluno, self.agora())
    }

    pub fn obter_aluno(&self, ra: &str) -> Result<Option<Aluno>> {
        obter(
            &self.conn,
            "SELECT ra, digito_ra, uf_ra, nome, situacao_matricula, metadata_bruta FROM alunos WHERE ra = ?1",
            [ra],
            aluno_da_linha,
        )
    }

    pub fn salvar_matricula(&self, matricula: &Matricula) -> Result<()> {
        upsert_matricula(&self.conn, matricula, self.agora())
    }

    /// A lista de chamada da turma, na ordem dos números de chamada.
    pub fn listar_matriculas(&self, turma_id: &str) -> Result<Vec<Matricula>> {
        listar(
            &self.conn,
            "SELECT turma_id, aluno_ra, numero_chamada, situacao, metadata_bruta FROM matriculas
             WHERE turma_id = ?1 ORDER BY numero_chamada IS NULL, numero_chamada, aluno_ra",
            [turma_id],
            matricula_da_linha,
        )
    }

    // -------------------------------------------------------------- aulas e chamada

    /// Grava uma aula VINDA DA PLATAFORMA (substitui tudo, inclusive `metadata_bruta`).
    /// Aula lançada pelo professor entra por `registrar_chamada`.
    pub fn salvar_aula(&self, aula: &Aula) -> Result<()> {
        upsert_aula_importada(&self.conn, aula, self.agora())
    }

    pub fn obter_aula(&self, id: &str) -> Result<Option<Aula>> {
        obter(
            &self.conn,
            "SELECT id, turma_id, data, disciplina, quantidade, metadata_bruta FROM aulas WHERE id = ?1",
            [id],
            aula_da_linha,
        )
    }

    pub fn listar_aulas(&self, turma_id: &str) -> Result<Vec<Aula>> {
        listar(
            &self.conn,
            "SELECT id, turma_id, data, disciplina, quantidade, metadata_bruta FROM aulas
             WHERE turma_id = ?1 ORDER BY data, id",
            [turma_id],
            aula_da_linha,
        )
    }

    pub fn frequencias_da_aula(&self, aula_id: &str) -> Result<Vec<RegistroFrequencia>> {
        listar(
            &self.conn,
            "SELECT aula_id, aluno_ra, status, metadata_bruta FROM frequencias
             WHERE aula_id = ?1 ORDER BY aluno_ra",
            [aula_id],
            frequencia_da_linha,
        )
    }

    /// A chamada lançada pelo professor (offline-first): numa transação só, grava a aula e os
    /// `registros` e põe o envio ao DC na Outbox como `PENDING_SYNC` (chave
    /// `dc:frequencia:<aula_id>`). Devolve o id do item. Chamar só quando o professor salva: salvar
    /// de novo rearma o envio mesmo que nada tenha mudado.
    /// - A turma precisa estar no banco e cada aluno, matriculado nela (erro pela posição na lista).
    /// - Quem não veio em `registros` fica como estava: o Ordoc não apaga lançamento (o JSON bruto
    ///   do DC é preservado, regra 4, e uma remoção não teria como chegar ao DC).
    /// - O JSON bruto que já veio do DC (aula e registros) é preservado.
    pub fn registrar_chamada(
        &mut self,
        aula: &Aula,
        registros: &[RegistroFrequencia],
    ) -> Result<i64> {
        aula.validar()?;
        let mut presentes_na_lista = HashSet::new();
        for (indice, r) in registros.iter().enumerate() {
            r.validar()?;
            if r.aula_id != aula.id {
                return Err(CoreError::AulaDivergente {
                    aula: aula.id.clone(),
                    registro: r.aula_id.clone(),
                }
                .into());
            }
            if !presentes_na_lista.insert(r.aluno_ra.as_str()) {
                return Err(StorageError::RegistroDuplicado { indice });
            }
        }

        let agora = self.agora();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let turma_existe: bool = tx
            .prepare_cached("SELECT EXISTS (SELECT 1 FROM turmas WHERE id = ?1)")?
            .query_row([&aula.turma_id], |r| r.get(0))?;
        if !turma_existe {
            return Err(StorageError::TurmaDesconhecida(aula.turma_id.clone()));
        }
        for (indice, r) in registros.iter().enumerate() {
            let matriculado: bool = tx
                .prepare_cached(
                    "SELECT EXISTS (SELECT 1 FROM matriculas WHERE turma_id = ?1 AND aluno_ra = ?2)",
                )?
                .query_row(params![aula.turma_id, r.aluno_ra], |row| row.get(0))?;
            if !matriculado {
                return Err(StorageError::AlunoForaDaTurma { indice });
            }
        }
        upsert_aula_local(&tx, aula, agora)?;
        for r in registros {
            upsert_frequencia_local(&tx, r, agora)?;
        }
        let operacao = OperacaoRemota {
            chave_idempotencia: chave_frequencia(&aula.id),
            plataforma: Plataforma::DiarioClasse,
            tipo: TIPO_FREQUENCIA.to_string(),
            payload: payload_chamada(&tx, &aula.id)?,
        };
        let id = outbox::enfileirar(&tx, &operacao, agora)?;
        tx.commit()?;
        Ok(id)
    }

    // -------------------------------------------------------------- Sala do Futuro

    pub fn salvar_atividade_sdf(&self, atividade: &AtividadeSdf) -> Result<()> {
        upsert_atividade(&self.conn, atividade, self.agora())
    }

    pub fn listar_atividades_sdf(&self, turma_codigo_sdf: &str) -> Result<Vec<AtividadeSdf>> {
        listar(
            &self.conn,
            "SELECT id, turma_codigo_sdf, titulo, prazo_iso, total_alunos, total_entregues, metadata_bruta
             FROM atividades_sdf WHERE turma_codigo_sdf = ?1
             ORDER BY prazo_iso IS NULL, prazo_iso, id",
            [turma_codigo_sdf],
            atividade_da_linha,
        )
    }

    pub fn salvar_entrega_sdf(&self, entrega: &EntregaSdf) -> Result<()> {
        upsert_entrega(&self.conn, entrega, self.agora())
    }

    pub fn entregas_da_atividade(&self, atividade_id: &str) -> Result<Vec<EntregaSdf>> {
        listar(
            &self.conn,
            "SELECT atividade_id, aluno_ra, entregue, entregue_em, metadata_bruta FROM entregas_sdf
             WHERE atividade_id = ?1 ORDER BY aluno_ra",
            [atividade_id],
            entrega_da_linha,
        )
    }

    // -------------------------------------------------------------- Painéis de BI

    /// Acrescenta as linhas de uma coleta de painel (numa transação só). Devolve quantas gravou.
    pub fn salvar_indicadores_bi(&mut self, linhas: &[IndicadorBi]) -> Result<usize> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        for linha in linhas {
            inserir_indicador_bi(&tx, linha)?;
        }
        tx.commit()?;
        Ok(linhas.len())
    }

    /// As linhas da coleta mais recente do painel.
    pub fn indicadores_bi_mais_recentes(&self, painel: &str) -> Result<Vec<IndicadorBi>> {
        listar(
            &self.conn,
            "SELECT painel, turma_id, aluno_ra, coletado_em, metadata_bruta FROM indicadores_bi
             WHERE painel = ?1
               AND coletado_em = (SELECT MAX(coletado_em) FROM indicadores_bi WHERE painel = ?1)
             ORDER BY id",
            [painel],
            indicador_bi_da_linha,
        )
    }

    /// Apaga as coletas de painel anteriores a `antes_de_ms` (histórico velho).
    pub fn apagar_indicadores_bi_anteriores(&self, antes_de_ms: i64) -> Result<usize> {
        Ok(self
            .conn
            .prepare_cached("DELETE FROM indicadores_bi WHERE coletado_em < ?1")?
            .execute([antes_de_ms])?)
    }

    // -------------------------------------------------------------- importação

    /// Grava tudo o que uma coleta trouxe, numa transação só: ou entra o lote inteiro (com o novo
    /// cursor da sincronização diferencial), ou nada entra e o cursor não anda. Registro órfão
    /// (aluno, turma ou atividade que não veio) é guardado e não trava o lote.
    ///
    /// `inicio_coleta_ms`: a hora (`BancoOrdoc::agora_ms`) em que a coleta COMEÇOU, antes do
    /// primeiro pedido à plataforma. Aula com chamada local ainda não sincronizada, ou que mudou/foi
    /// enviada depois desse instante, não é tocada (nem a aula nem a frequência) e entra em
    /// `aulas_a_recoletar`: como o cursor anda, a coleta diferencial não a traria de novo.
    pub fn aplicar_lote(
        &mut self,
        plataforma: Plataforma,
        lote: &LoteCanonico,
        inicio_coleta_ms: i64,
    ) -> Result<ResultadoLote> {
        let agora = self.agora();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut resultado = ResultadoLote::default();
        let mut protegidas: HashMap<&str, bool> = HashMap::new();
        let mut adiadas: HashSet<&str> = HashSet::new();
        let mut importadas: HashSet<&str> = HashSet::new();

        for (n, t) in lote.turmas.iter().enumerate() {
            com_contexto(upsert_turma(&tx, t, agora), "turma", n)?;
            resultado.registros_gravados += 1;
        }
        for (n, a) in lote.alunos.iter().enumerate() {
            com_contexto(upsert_aluno(&tx, a, agora), "aluno", n)?;
            resultado.registros_gravados += 1;
        }
        for (n, m) in lote.matriculas.iter().enumerate() {
            com_contexto(upsert_matricula(&tx, m, agora), "matrícula", n)?;
            resultado.registros_gravados += 1;
        }
        for (n, a) in lote.aulas.iter().enumerate() {
            if chamada_local_protegida(&tx, &mut protegidas, &a.id, inicio_coleta_ms)? {
                resultado.aulas_preservadas += 1;
                if adiadas.insert(&a.id) {
                    marcar_recoleta(&tx, &a.id, plataforma, agora)?;
                }
                continue;
            }
            com_contexto(upsert_aula_importada(&tx, a, agora), "aula", n)?;
            importadas.insert(&a.id);
            resultado.registros_gravados += 1;
        }
        for (n, f) in lote.frequencias.iter().enumerate() {
            if chamada_local_protegida(&tx, &mut protegidas, &f.aula_id, inicio_coleta_ms)? {
                resultado.frequencias_preservadas += 1;
                if adiadas.insert(&f.aula_id) {
                    marcar_recoleta(&tx, &f.aula_id, plataforma, agora)?;
                }
                continue;
            }
            com_contexto(upsert_frequencia_importada(&tx, f, agora), "frequência", n)?;
            importadas.insert(&f.aula_id);
            resultado.registros_gravados += 1;
        }
        for (n, a) in lote.atividades_sdf.iter().enumerate() {
            com_contexto(upsert_atividade(&tx, a, agora), "atividade", n)?;
            resultado.registros_gravados += 1;
        }
        for (n, e) in lote.entregas_sdf.iter().enumerate() {
            com_contexto(upsert_entrega(&tx, e, agora), "entrega", n)?;
            resultado.registros_gravados += 1;
        }
        for (n, i) in lote.indicadores_bi.iter().enumerate() {
            com_contexto(inserir_indicador_bi(&tx, i), "indicador de BI", n)?;
            resultado.registros_gravados += 1;
        }
        for aula_id in &importadas {
            tx.prepare_cached("DELETE FROM aulas_a_recoletar WHERE aula_id = ?1")?
                .execute([aula_id])?;
        }
        if let Some(cursor) = &lote.cursor {
            upsert_cursor(&tx, plataforma, cursor, agora)?;
        }
        tx.commit()?;
        Ok(resultado)
    }

    /// Aulas que o worker deve buscar de novo pelo id (a importação as pulou, ou o professor
    /// descartou a chamada local), já sem proteção: sem item de chamada pendente, em envio ou com
    /// falha. Busque cada uma e passe num lote para `aplicar_lote`; a marca some quando ela entra.
    pub fn aulas_a_recoletar(&self, plataforma: Plataforma) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT r.aula_id FROM aulas_a_recoletar r
             WHERE r.plataforma = ?1
               AND NOT EXISTS (SELECT 1 FROM outbox o
                               WHERE o.chave_idempotencia = ?2 || r.aula_id
                                 AND o.status <> 'SYNCED')
             ORDER BY r.marcado_em, r.aula_id",
        )?;
        let ids = stmt
            .query_map(params![plataforma.as_str(), chave_frequencia("")], |r| {
                r.get(0)
            })?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        Ok(ids)
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::apoio::{aluno, aula, matricula, presenca, turma, BancoTeste, DIA, MINUTO};
    use ordoc_core::ErroConector;
    use ordoc_core::StatusPresenca::{Falta, FaltaJustificada, Presente};
    use ordoc_core::{StatusSincronizacao, RA_AUSENTE};
    use serde_json::json;

    #[test]
    fn metadata_bruta_sobrevive_a_ida_e_volta() {
        let mut t = BancoTeste::novo();
        t.preparar_turma("T1", Some("3B_SDF"), &["000000001"]);

        assert_eq!(
            t.banco.obter_turma("T1").unwrap().unwrap(),
            turma("T1", Some("3B_SDF"))
        );
        assert_eq!(
            t.banco.obter_aluno("000000001").unwrap().unwrap(),
            aluno("000000001")
        );
        assert_eq!(
            t.banco.listar_matriculas("T1").unwrap(),
            vec![matricula("T1", "000000001", 1)]
        );

        let mut a = aula("A1", "T1", "2026-11-10", 2);
        a.metadata_bruta =
            json!({"idAulaDc": 77, "conteudo": {"texto": "Laços", "habilidades": ["EM13"]}});
        t.banco.salvar_aula(&a).unwrap();
        assert_eq!(t.banco.obter_aula("A1").unwrap().unwrap(), a);

        let atividade = AtividadeSdf {
            id: "task_1".into(),
            turma_codigo_sdf: "3B_SDF".into(),
            titulo: "Arrays".into(),
            prazo_iso: Some("2026-11-20T23:59:00Z".into()),
            total_alunos: 30,
            total_entregues: 12,
            metadata_bruta: json!({"node": {"id": "task_1", "rubrica": [1, 2]}, "campoNovo": null}),
        };
        t.banco.salvar_atividade_sdf(&atividade).unwrap();
        assert_eq!(
            t.banco.listar_atividades_sdf("3B_SDF").unwrap(),
            vec![atividade]
        );

        let entrega = EntregaSdf {
            atividade_id: "task_1".into(),
            aluno_ra: "000000001".into(),
            entregue: true,
            entregue_em: Some("2026-11-12T10:00:00Z".into()),
            metadata_bruta: json!({"nota": 9.5}),
        };
        t.banco.salvar_entrega_sdf(&entrega).unwrap();
        assert_eq!(
            t.banco.entregas_da_atividade("task_1").unwrap(),
            vec![entrega]
        );

        let painel = IndicadorBi {
            painel: "frequencia_mensal".into(),
            turma_id: Some("T1".into()),
            aluno_ra: None,
            coletado_em_ms: t.agora(),
            metadata_bruta: json!({"turma": "3B", "frequencia_media": 92.5, "coluna_nova": [1, 2]}),
        };
        assert_eq!(
            t.banco
                .salvar_indicadores_bi(std::slice::from_ref(&painel))
                .unwrap(),
            1
        );
        assert_eq!(
            t.banco
                .indicadores_bi_mais_recentes("frequencia_mensal")
                .unwrap(),
            vec![painel]
        );
    }

    #[test]
    fn importacao_do_dc_nao_apaga_a_associacao_com_a_sdf() {
        let t = BancoTeste::novo();
        t.banco.salvar_turma(&turma("T1", None)).unwrap();
        assert!(t.banco.associar_codigo_sdf("T1", Some("3B_SDF")).unwrap());
        // O DC não conhece o código da SDF: regravar a turma não pode apagar a associação.
        t.banco.salvar_turma(&turma("T1", None)).unwrap();
        assert_eq!(
            t.banco
                .obter_turma("T1")
                .unwrap()
                .unwrap()
                .codigo_sdf
                .as_deref(),
            Some("3B_SDF")
        );
        assert!(t.banco.associar_codigo_sdf("T1", None).unwrap());
        assert_eq!(t.banco.obter_turma("T1").unwrap().unwrap().codigo_sdf, None);
        assert!(!t
            .banco
            .associar_codigo_sdf("T-INEXISTENTE", Some("x"))
            .unwrap());
    }

    #[test]
    fn regravar_turma_e_aula_nao_apaga_as_chamadas_em_cascata() {
        let mut t = BancoTeste::novo();
        t.preparar_turma("T1", None, &["000000001"]);
        let a = aula("A1", "T1", "2026-11-10", 1);
        t.banco
            .registrar_chamada(&a, &[presenca("A1", "000000001", Falta)])
            .unwrap();
        // INSERT OR REPLACE apagaria a turma e, em cascata, a aula e a chamada.
        t.banco.salvar_turma(&turma("T1", None)).unwrap();
        t.banco.salvar_aluno(&aluno("000000001")).unwrap();
        t.banco.salvar_aula(&a).unwrap();
        assert_eq!(t.banco.frequencias_da_aula("A1").unwrap().len(), 1);
    }

    #[test]
    fn ra_ausente_nunca_vira_chave() {
        let t = BancoTeste::novo();
        let mut sem_ra = aluno(RA_AUSENTE);
        assert!(matches!(
            t.banco.salvar_aluno(&sem_ra),
            Err(StorageError::Dominio(CoreError::RaAusente))
        ));
        sem_ra.ra = String::new();
        assert!(t.banco.salvar_aluno(&sem_ra).is_err());
    }

    #[test]
    fn chamada_grava_local_enfileira_e_preserva_o_json_do_dc() {
        let mut t = BancoTeste::novo();
        t.preparar_turma("T1", None, &["000000001", "000000002", "000000003"]);
        // A aula e um lançamento já vieram do DC, com o id do lançamento no JSON bruto.
        t.banco
            .salvar_aula(&aula("A1", "T1", "2026-11-10", 2))
            .unwrap();
        let mut veio_do_dc = presenca("A1", "000000001", Presente);
        veio_do_dc.metadata_bruta = json!({"idLancamento": 991});
        let lote = LoteCanonico {
            frequencias: vec![veio_do_dc],
            ..Default::default()
        };
        t.banco
            .aplicar_lote(Plataforma::DiarioClasse, &lote, t.agora())
            .unwrap();

        // O professor faz a chamada no Ordoc (a aula chega sem o JSON do DC).
        let mut local = aula("A1", "T1", "2026-11-10", 2);
        local.metadata_bruta = json!({});
        let chamada = vec![
            presenca("A1", "000000003", Falta),
            presenca("A1", "000000001", Falta),
            presenca("A1", "000000002", FaltaJustificada),
        ];
        let id = t.banco.registrar_chamada(&local, &chamada).unwrap();

        let item = t.banco.obter_item_outbox(id).unwrap().unwrap();
        assert_eq!(item.status, StatusSincronizacao::PendingSync);
        assert_eq!(item.operacao.chave_idempotencia, "dc:frequencia:A1");
        let p = &item.operacao.payload;
        assert_eq!(
            p["aula"]["metadata_bruta"]["idAulaDc"], "A1",
            "JSON da aula preservado"
        );
        let ras: Vec<&str> = p["registros"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["aluno_ra"].as_str().unwrap())
            .collect();
        assert_eq!(
            ras,
            vec!["000000001", "000000002", "000000003"],
            "em ordem de RA"
        );
        assert_eq!(p["registros"][0]["status"], "FALTA");
        assert_eq!(p["registros"][0]["metadata_bruta"]["idLancamento"], 991);

        // Salvar a mesma chamada de novo não cria item nem nova versão.
        assert_eq!(t.banco.registrar_chamada(&local, &chamada).unwrap(), id);
        assert_eq!(t.banco.obter_item_outbox(id).unwrap().unwrap().versao, 1);

        // Lista menor: quem não veio fica como estava (nada é apagado), e a versão sobe.
        let menor = vec![
            presenca("A1", "000000001", Presente),
            presenca("A1", "000000002", Falta),
        ];
        assert_eq!(t.banco.registrar_chamada(&local, &menor).unwrap(), id);
        let freq = t.banco.frequencias_da_aula("A1").unwrap();
        assert_eq!(freq.len(), 3);
        assert_eq!(freq[2].status, Falta, "RA3 ficou como estava");
        assert_eq!(
            freq[0].metadata_bruta["idLancamento"], 991,
            "JSON do DC preservado"
        );
        let item = t.banco.obter_item_outbox(id).unwrap().unwrap();
        assert_eq!(item.versao, 2);
        assert_eq!(
            item.operacao.payload["registros"].as_array().unwrap().len(),
            3
        );
        assert_eq!(t.banco.contar_outbox().unwrap().pendentes, 1);
    }

    #[test]
    fn chamada_invalida_nao_grava_nada() {
        let mut t = BancoTeste::novo();
        t.preparar_turma("T1", None, &["000000001"]);
        let a = aula("A1", "T1", "2026-11-10", 1);

        let repetida = vec![
            presenca("A1", "000000001", Falta),
            presenca("A1", "000000001", Presente),
        ];
        let erro = t.banco.registrar_chamada(&a, &repetida).unwrap_err();
        assert!(
            matches!(erro, StorageError::RegistroDuplicado { indice: 1 }),
            "{erro}"
        );
        assert!(
            !erro.to_string().contains("000000001"),
            "LGPD: sem RA no texto"
        );
        assert!(
            !format!("{erro:?}").contains("000000001"),
            "LGPD: sem RA no Debug"
        );
        assert!(matches!(
            t.banco
                .registrar_chamada(&a, &[presenca("A2", "000000001", Falta)]),
            Err(StorageError::Dominio(CoreError::AulaDivergente { .. }))
        ));
        // Aluno sem matrícula na turma: recusado pela posição, sem citar o RA.
        let erro = t
            .banco
            .registrar_chamada(
                &a,
                &[
                    presenca("A1", "000000001", Falta),
                    presenca("A1", "000000099", Falta),
                ],
            )
            .unwrap_err();
        assert!(
            matches!(erro, StorageError::AlunoForaDaTurma { indice: 1 }),
            "{erro}"
        );
        assert!(!format!("{erro} {erro:?}").contains("000000099"));
        // Turma que não está no banco.
        assert!(matches!(
            t.banco
                .registrar_chamada(&aula("A9", "T-X", "2026-11-10", 1), &[]),
            Err(StorageError::TurmaDesconhecida(id)) if id == "T-X"
        ));
        assert!(
            t.banco.obter_aula("A1").unwrap().is_none(),
            "a aula também não ficou"
        );
        assert_eq!(t.banco.contar_outbox().unwrap().pendentes, 0);
    }

    #[test]
    fn importacao_nao_desfaz_chamada_local_ainda_nao_sincronizada() {
        let mut t = BancoTeste::novo();
        t.preparar_turma("T1", None, &["000000001"]);
        let a1 = aula("A1", "T1", "2026-11-10", 1);
        let id = t
            .banco
            .registrar_chamada(&a1, &[presenca("A1", "000000001", Falta)])
            .unwrap();

        let mut a1_do_dc = aula("A1", "T1", "2026-11-10", 2);
        a1_do_dc.disciplina = Some("Outra".into());
        let lote = LoteCanonico {
            aulas: vec![a1_do_dc, aula("A2", "T1", "2026-11-11", 1)],
            frequencias: vec![
                presenca("A1", "000000001", Presente),
                presenca("A2", "000000001", Presente),
            ],
            ..Default::default()
        };
        let r = t
            .banco
            .aplicar_lote(Plataforma::DiarioClasse, &lote, t.agora())
            .unwrap();
        assert_eq!(
            r,
            ResultadoLote {
                registros_gravados: 2,
                aulas_preservadas: 1,
                frequencias_preservadas: 1
            }
        );
        assert_eq!(t.banco.frequencias_da_aula("A1").unwrap()[0].status, Falta);
        assert_eq!(t.banco.obter_aula("A1").unwrap().unwrap().quantidade, 1);
        assert_eq!(
            t.banco.frequencias_da_aula("A2").unwrap()[0].status,
            Presente
        );

        // Depois que o DC confirmou a chamada, uma coleta que COMEÇOU depois volta a valer para A1.
        t.banco.reservar_lote(10).unwrap();
        t.banco.marcar_sincronizado(id, 1).unwrap();
        t.avancar(MINUTO);
        let r = t
            .banco
            .aplicar_lote(Plataforma::DiarioClasse, &lote, t.agora())
            .unwrap();
        assert_eq!(r.aulas_preservadas, 0);
        assert_eq!(
            t.banco.frequencias_da_aula("A1").unwrap()[0].status,
            Presente
        );
    }

    #[test]
    fn lote_e_atomico_e_o_cursor_anda_junto() {
        let mut t = BancoTeste::novo();
        let ruim = LoteCanonico {
            turmas: vec![turma("T1", Some("3B_SDF"))],
            entregas_sdf: vec![EntregaSdf {
                atividade_id: "task_1".into(),
                aluno_ra: RA_AUSENTE.into(),
                entregue: false,
                entregue_em: None,
                metadata_bruta: json!({}),
            }],
            cursor: Some("2026-11-14T00:00:00Z".into()),
            ..Default::default()
        };
        let erro = t
            .banco
            .aplicar_lote(Plataforma::SalaDoFuturo, &ruim, t.agora())
            .unwrap_err();
        assert!(
            matches!(
                &erro,
                StorageError::Registro {
                    tipo: "entrega",
                    indice: 0,
                    ..
                }
            ),
            "{erro}"
        );
        assert!(
            t.banco.obter_turma("T1").unwrap().is_none(),
            "nada do lote entrou"
        );
        assert_eq!(
            t.banco.obter_cursor(Plataforma::SalaDoFuturo).unwrap(),
            None
        );

        let bom = LoteCanonico {
            turmas: vec![turma("T1", Some("3B_SDF"))],
            cursor: Some("2026-11-14T00:00:00Z".into()),
            ..Default::default()
        };
        t.banco
            .aplicar_lote(Plataforma::SalaDoFuturo, &bom, t.agora())
            .unwrap();
        assert_eq!(
            t.banco
                .obter_cursor(Plataforma::SalaDoFuturo)
                .unwrap()
                .as_deref(),
            Some("2026-11-14T00:00:00Z")
        );
    }

    /// Envia a chamada pendente do item (reserva + confirma a versão atual).
    fn sincronizar(t: &BancoTeste, id: i64) {
        let versao = t.banco.obter_item_outbox(id).unwrap().unwrap().versao;
        t.banco.reservar_lote(10).unwrap();
        t.banco.marcar_sincronizado(id, versao).unwrap();
    }

    fn lote_a1(status: StatusPresenca) -> LoteCanonico {
        let mut f = presenca("A1", "000000001", status);
        f.metadata_bruta = json!({"idLancamento": 991});
        LoteCanonico {
            frequencias: vec![f],
            ..Default::default()
        }
    }

    #[test]
    fn mesma_chamada_depois_de_importacao_divergente_e_reenviada() {
        let mut t = BancoTeste::novo();
        t.preparar_turma("T1", None, &["000000001"]);
        let a1 = aula("A1", "T1", "2026-11-10", 1);
        let chamada = [presenca("A1", "000000001", Falta)];
        let id = t.banco.registrar_chamada(&a1, &chamada).unwrap();
        sincronizar(&t, id);
        // Alguém muda para PRESENTE no DC; a coleta seguinte importa (o item estava SYNCED).
        t.avancar(MINUTO);
        let r = t
            .banco
            .aplicar_lote(Plataforma::DiarioClasse, &lote_a1(Presente), t.agora())
            .unwrap();
        assert_eq!(r.frequencias_preservadas, 0);
        // O professor volta para FALTA: payload igual ao último enviado, mas precisa sair de novo.
        assert_eq!(t.banco.registrar_chamada(&a1, &chamada).unwrap(), id);
        let item = t.banco.obter_item_outbox(id).unwrap().unwrap();
        assert_eq!(
            (item.status, item.versao),
            (StatusSincronizacao::PendingSync, 2)
        );
    }

    #[test]
    fn lote_lido_antes_do_envio_nao_desfaz_a_chamada() {
        let mut t = BancoTeste::novo();
        t.preparar_turma("T1", None, &["000000001"]);
        let inicio_coleta = t.agora(); // o coletor começa a ler o DC aqui
        t.avancar(MINUTO);
        let a1 = aula("A1", "T1", "2026-11-10", 1);
        let id = t
            .banco
            .registrar_chamada(&a1, &[presenca("A1", "000000001", Falta)])
            .unwrap();
        t.avancar(MINUTO);
        sincronizar(&t, id); // o worker envia e o item fica SYNCED
        t.avancar(MINUTO);
        // Chega o lote lido no início (ainda com PRESENTE): não pode desfazer a chamada.
        let r = t
            .banco
            .aplicar_lote(Plataforma::DiarioClasse, &lote_a1(Presente), inicio_coleta)
            .unwrap();
        assert_eq!(r.frequencias_preservadas, 1);
        assert_eq!(t.banco.frequencias_da_aula("A1").unwrap()[0].status, Falta);
        // O cursor andou: A1 fica marcada para ser buscada de novo pelo id.
        let dc = Plataforma::DiarioClasse;
        assert_eq!(t.banco.aulas_a_recoletar(dc).unwrap(), vec!["A1"]);
        // Uma coleta que começou depois do envio vale (e tira a marca).
        let r = t
            .banco
            .aplicar_lote(Plataforma::DiarioClasse, &lote_a1(Presente), t.agora())
            .unwrap();
        assert_eq!(r.frequencias_preservadas, 0);
        assert_eq!(
            t.banco.frequencias_da_aula("A1").unwrap()[0].status,
            Presente
        );
        assert!(t.banco.aulas_a_recoletar(dc).unwrap().is_empty());
    }

    #[test]
    fn chamada_recusada_bloqueia_a_importacao_ate_o_professor_descartar() {
        let mut t = BancoTeste::novo();
        t.preparar_turma("T1", None, &["000000001"]);
        let a1 = aula("A1", "T1", "2026-11-10", 1);
        let id = t
            .banco
            .registrar_chamada(&a1, &[presenca("A1", "000000001", Falta)])
            .unwrap();
        t.banco.reservar_lote(10).unwrap();
        let recusa = ErroConector::Recusada("aula já fechada".into());
        assert_eq!(
            t.banco.marcar_falha(id, 1, &recusa, 0, 8).unwrap(),
            StatusSincronizacao::Failed
        );
        t.avancar(MINUTO);
        let r = t
            .banco
            .aplicar_lote(Plataforma::DiarioClasse, &lote_a1(Presente), t.agora())
            .unwrap();
        assert_eq!(r.frequencias_preservadas, 1, "FAILED ainda protege");
        let dc = Plataforma::DiarioClasse;
        assert!(
            t.banco.aulas_a_recoletar(dc).unwrap().is_empty(),
            "ainda protegida: não adianta buscar"
        );
        // O professor aceita a versão do DC: A1 deve ser buscada de novo.
        assert!(t.banco.descartar_item_outbox(id).unwrap());
        assert_eq!(t.banco.aulas_a_recoletar(dc).unwrap(), vec!["A1"]);
        let r = t
            .banco
            .aplicar_lote(Plataforma::DiarioClasse, &lote_a1(Presente), t.agora())
            .unwrap();
        assert_eq!(r.frequencias_preservadas, 0);
        assert_eq!(
            t.banco.frequencias_da_aula("A1").unwrap()[0].status,
            Presente
        );
        assert!(t.banco.aulas_a_recoletar(dc).unwrap().is_empty());
    }

    #[test]
    fn descartar_chamada_sem_lote_no_meio_tambem_pede_recoleta() {
        let mut t = BancoTeste::novo();
        t.preparar_turma("T1", None, &["000000001"]);
        let id = t
            .banco
            .registrar_chamada(
                &aula("A1", "T1", "2026-11-10", 1),
                &[presenca("A1", "000000001", Falta)],
            )
            .unwrap();
        assert!(t.banco.descartar_item_outbox(id).unwrap());
        assert_eq!(
            t.banco.aulas_a_recoletar(Plataforma::DiarioClasse).unwrap(),
            vec!["A1"]
        );
        assert_eq!(t.banco.agora_ms(), t.agora(), "mesmo relógio da Outbox");
    }

    #[test]
    fn lote_com_registro_orfao_entra_e_o_cursor_anda() {
        let mut t = BancoTeste::novo();
        let lote = LoteCanonico {
            matriculas: vec![matricula("T-SUMIU", "000000077", 1)],
            aulas: vec![aula("A7", "T-SUMIU", "2026-11-10", 1)],
            frequencias: vec![
                presenca("A7", "000000077", Falta),
                presenca("A-SUMIU", "000000078", Presente),
            ],
            entregas_sdf: vec![EntregaSdf {
                atividade_id: "task_sumiu".into(),
                aluno_ra: "000000077".into(),
                entregue: true,
                entregue_em: None,
                metadata_bruta: json!({"x": 1}),
            }],
            cursor: Some("c-1".into()),
            ..Default::default()
        };
        let r = t
            .banco
            .aplicar_lote(Plataforma::DiarioClasse, &lote, t.agora())
            .unwrap();
        assert_eq!(r.registros_gravados, 5);
        assert_eq!(
            t.banco
                .obter_cursor(Plataforma::DiarioClasse)
                .unwrap()
                .as_deref(),
            Some("c-1")
        );
        assert_eq!(t.banco.frequencias_da_aula("A-SUMIU").unwrap().len(), 1);
        assert_eq!(
            t.banco.entregas_da_atividade("task_sumiu").unwrap().len(),
            1
        );
    }

    #[test]
    fn bi_guarda_historico_e_devolve_a_coleta_mais_recente() {
        let mut t = BancoTeste::novo();
        let linha = |coletado: i64, valor: f64| IndicadorBi {
            painel: "risco".into(),
            turma_id: None,
            aluno_ra: None,
            coletado_em_ms: coletado,
            metadata_bruta: json!({"valor": valor}),
        };
        let ontem = t.agora() - DIA;
        t.banco
            .salvar_indicadores_bi(&[linha(ontem, 1.0), linha(ontem, 2.0)])
            .unwrap();
        t.banco
            .salvar_indicadores_bi(&[linha(t.agora(), 3.0)])
            .unwrap();
        let recentes = t.banco.indicadores_bi_mais_recentes("risco").unwrap();
        assert_eq!(recentes.len(), 1);
        assert_eq!(recentes[0].valor("valor"), Some(&json!(3.0)));
        assert_eq!(
            t.banco.apagar_indicadores_bi_anteriores(t.agora()).unwrap(),
            2
        );
        assert!(t
            .banco
            .indicadores_bi_mais_recentes("vazio")
            .unwrap()
            .is_empty());
    }
}
