//! Fila Outbox (offline-first).
//!
//! Ciclo de um item: `PENDING_SYNC` -> (`reservar_lote`) `IN_FLIGHT` -> `SYNCED`, ou de volta a
//! `PENDING_SYNC` com nova hora de tentativa, ou `FAILED` (precisa do professor).
//!
//! - A `chave_idempotencia` é única: salvar de novo a mesma chamada atualiza o item (não duplica).
//!   Conteúdo igual a um item `PENDING_SYNC` ou `IN_FLIGHT` não muda nada (ele já vai sair). Salvar
//!   de novo um item `SYNCED` (ou `FAILED`) o rearma MESMO com o conteúdo igual: a plataforma pode
//!   ter sido mudada por outra pessoa depois do envio, e quem decide se precisa gravar é o conector,
//!   que consulta antes (`ResultadoEnvio::JaAplicado`). Chame só em ação explícita do professor.
//! - `descartar_item_outbox`: o professor desiste de um envio (ex.: o DC recusou e ele aceita a
//!   versão de lá); a próxima importação volta a valer para aquele dado.
//! - `versao` sobe a cada alteração. Se o professor edita a chamada enquanto o worker a envia, o
//!   item continua `IN_FLIGHT` com a versão nova; quando o envio da versão velha termina, o item
//!   volta a `PENDING_SYNC` (em vez de `SYNCED`) e a versão nova sai em seguida, nunca em paralelo.
//! - `recuperar_em_voo` devolve à fila o que ficou `IN_FLIGHT` numa queda do aplicativo: chamar
//!   uma vez ao abrir, antes de ligar o worker.

use ordoc_core::{
    sanitizar_detalhe, AcaoAposErro, CoreError, ErroConector, OperacaoRemota, Plataforma,
    StatusSincronizacao,
};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::error::{Result, StorageError};
use crate::sql::{enum_da_coluna, json_da_coluna, json_texto, listar, obter};
use crate::BancoOrdoc;

/// Tipo das operações de chamada do Diário de Classe.
pub const TIPO_FREQUENCIA: &str = "frequencia";

/// Tentativas antes de um erro passageiro virar `FAILED`.
pub const MAX_TENTATIVAS_PADRAO: u32 = 8;

/// Chave de idempotência da chamada de uma aula: `dc:frequencia:<aula_id>`.
pub fn chave_frequencia(aula_id: &str) -> String {
    format!(
        "{}:{}:{}",
        Plataforma::DiarioClasse.as_str(),
        TIPO_FREQUENCIA,
        aula_id
    )
}

/// Um item da fila. O `Debug` não mostra o payload (tem RA dos alunos: LGPD).
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemOutbox {
    pub id: i64,
    pub operacao: OperacaoRemota,
    pub status: StatusSincronizacao,
    /// Sobe a cada alteração do conteúdo; o worker devolve a versão que enviou.
    pub versao: i64,
    pub tentativas: u32,
    pub proxima_tentativa_em_ms: i64,
    /// Último erro, já sanitizado (sem a consulta das URLs).
    pub ultimo_erro: Option<String>,
    pub criado_em_ms: i64,
    pub atualizado_em_ms: i64,
}

impl std::fmt::Debug for ItemOutbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ItemOutbox")
            .field("id", &self.id)
            .field("chave_idempotencia", &self.operacao.chave_idempotencia)
            .field("plataforma", &self.operacao.plataforma)
            .field("tipo", &self.operacao.tipo)
            .field("status", &self.status)
            .field("versao", &self.versao)
            .field("tentativas", &self.tentativas)
            .field("proxima_tentativa_em_ms", &self.proxima_tentativa_em_ms)
            .field("ultimo_erro", &self.ultimo_erro)
            .finish_non_exhaustive()
    }
}

/// Quantos itens há em cada estado (para a tela de Pendências).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContagemOutbox {
    pub pendentes: u64,
    pub em_voo: u64,
    pub sincronizados: u64,
    pub falhas: u64,
}

macro_rules! colunas_outbox {
    () => {
        "id, chave_idempotencia, plataforma, tipo, payload, status, versao, tentativas, \
         proxima_tentativa_em, ultimo_erro, criado_em, atualizado_em"
    };
}

fn item_da_linha(row: &Row<'_>) -> rusqlite::Result<ItemOutbox> {
    Ok(ItemOutbox {
        id: row.get(0)?,
        operacao: OperacaoRemota {
            chave_idempotencia: row.get(1)?,
            plataforma: enum_da_coluna(row, 2, Plataforma::parse)?,
            tipo: row.get(3)?,
            payload: json_da_coluna(row, 4)?,
        },
        status: enum_da_coluna(row, 5, StatusSincronizacao::parse)?,
        versao: row.get(6)?,
        tentativas: row.get(7)?,
        proxima_tentativa_em_ms: row.get(8)?,
        ultimo_erro: row.get(9)?,
        criado_em_ms: row.get(10)?,
        atualizado_em_ms: row.get(11)?,
    })
}

const SQL_ENFILEIRAR: &str = "
INSERT INTO outbox (chave_idempotencia, plataforma, tipo, payload, status, versao, tentativas,
                    proxima_tentativa_em, ultimo_erro, criado_em, atualizado_em)
VALUES (?1, ?2, ?3, ?4, 'PENDING_SYNC', 1, 0, ?5, NULL, ?5, ?5)
ON CONFLICT (chave_idempotencia) DO UPDATE SET
    plataforma = excluded.plataforma,
    tipo = excluded.tipo,
    payload = excluded.payload,
    status = CASE WHEN outbox.status = 'IN_FLIGHT' THEN 'IN_FLIGHT' ELSE 'PENDING_SYNC' END,
    versao = outbox.versao + 1,
    tentativas = 0,
    proxima_tentativa_em = excluded.proxima_tentativa_em,
    ultimo_erro = NULL,
    atualizado_em = excluded.atualizado_em
WHERE outbox.payload <> excluded.payload
   OR outbox.plataforma <> excluded.plataforma
   OR outbox.tipo <> excluded.tipo
   OR outbox.status IN ('SYNCED', 'FAILED')
RETURNING id";

/// Põe (ou atualiza) uma operação na fila, dentro da conexão ou transação recebida.
pub(crate) fn enfileirar(conn: &Connection, op: &OperacaoRemota, agora: i64) -> Result<i64> {
    if op.chave_idempotencia.trim().is_empty() {
        return Err(CoreError::IdentificadorAusente("operacao.chave_idempotencia").into());
    }
    if op.tipo.trim().is_empty() {
        return Err(CoreError::IdentificadorAusente("operacao.tipo").into());
    }
    let payload = json_texto(&op.payload)?;
    let id: Option<i64> = conn
        .prepare_cached(SQL_ENFILEIRAR)?
        .query_row(
            params![
                op.chave_idempotencia,
                op.plataforma.as_str(),
                op.tipo,
                payload,
                agora
            ],
            |r| r.get(0),
        )
        .optional()?;
    match id {
        Some(id) => Ok(id),
        // Mesmo conteúdo de um item que não precisa mudar: devolve o que já existe.
        None => Ok(conn
            .prepare_cached("SELECT id FROM outbox WHERE chave_idempotencia = ?1")?
            .query_row([&op.chave_idempotencia], |r| r.get(0))?),
    }
}

fn status_devolvido(status: Option<String>, id: i64) -> Result<StatusSincronizacao> {
    let texto = status.ok_or(StorageError::ItemNaoReservado(id))?;
    Ok(StatusSincronizacao::parse(&texto)?)
}

impl BancoOrdoc {
    /// Põe uma operação na fila (ex.: uma nota). Para a chamada use `registrar_chamada`, que grava
    /// a frequência e enfileira na mesma transação.
    pub fn enfileirar(&self, operacao: &OperacaoRemota) -> Result<i64> {
        enfileirar(&self.conn, operacao, self.agora())
    }

    /// Reserva até `limite` itens vencidos (`PENDING_SYNC` com a hora de tentativa já passada),
    /// marcando-os `IN_FLIGHT` num único comando. Ordem: o mais atrasado primeiro.
    pub fn reservar_lote(&self, limite: u32) -> Result<Vec<ItemOutbox>> {
        let mut itens = listar(
            &self.conn,
            concat!(
                "UPDATE outbox SET status = 'IN_FLIGHT', atualizado_em = ?1 WHERE id IN (",
                "SELECT id FROM outbox WHERE status = 'PENDING_SYNC' AND proxima_tentativa_em <= ?1 ",
                "ORDER BY proxima_tentativa_em, id LIMIT ?2) RETURNING ",
                colunas_outbox!()
            ),
            params![self.agora(), limite],
            item_da_linha,
        )?;
        itens.sort_by(|a, b| {
            a.proxima_tentativa_em_ms
                .cmp(&b.proxima_tentativa_em_ms)
                .then(a.id.cmp(&b.id))
        });
        Ok(itens)
    }

    /// O envio da `versao_enviada` deu certo. Devolve `SYNCED`, ou `PENDING_SYNC` se o professor
    /// mudou o item durante o envio (a versão nova ainda precisa sair).
    pub fn marcar_sincronizado(&self, id: i64, versao_enviada: i64) -> Result<StatusSincronizacao> {
        let agora = self.agora();
        let status: Option<String> = self
            .conn
            .prepare_cached(
                "UPDATE outbox SET
                     status = CASE WHEN versao = ?2 THEN 'SYNCED' ELSE 'PENDING_SYNC' END,
                     tentativas = CASE WHEN versao = ?2 THEN tentativas ELSE 0 END,
                     proxima_tentativa_em = ?3,
                     ultimo_erro = NULL,
                     atualizado_em = ?3
                 WHERE id = ?1 AND status = 'IN_FLIGHT'
                 RETURNING status",
            )?
            .query_row(params![id, versao_enviada, agora], |r| r.get(0))
            .optional()?;
        status_devolvido(status, id)
    }

    /// O envio falhou. Decide pelo `ErroConector::acao()`:
    /// - `TentarDeNovo`: gasta uma tentativa e volta à fila daqui a `atraso_ms` (o backoff é do
    ///   worker); esgotadas as `max_tentativas`, vira `FAILED`;
    /// - `AguardarLogin`: volta à fila sem gastar tentativa (o worker pausa a plataforma);
    /// - `Desistir`: `FAILED` na hora.
    ///
    /// Se o item mudou durante o envio, a falha era da versão velha: volta à fila zerado.
    pub fn marcar_falha(
        &self,
        id: i64,
        versao_enviada: i64,
        erro: &ErroConector,
        atraso_ms: i64,
        max_tentativas: u32,
    ) -> Result<StatusSincronizacao> {
        let agora = self.agora();
        let proxima = agora.saturating_add(atraso_ms.max(0));
        let (gasta, desiste) = match erro.acao() {
            AcaoAposErro::TentarDeNovo => (1_u32, false),
            AcaoAposErro::AguardarLogin => (0, false),
            AcaoAposErro::Desistir => (1, true),
        };
        let status: Option<String> = self
            .conn
            .prepare_cached(
                "UPDATE outbox SET
                     status = CASE
                         WHEN versao <> ?2 THEN 'PENDING_SYNC'
                         WHEN ?3 THEN 'FAILED'
                         WHEN tentativas + ?4 >= ?5 THEN 'FAILED'
                         ELSE 'PENDING_SYNC' END,
                     tentativas = CASE WHEN versao <> ?2 THEN 0 ELSE tentativas + ?4 END,
                     proxima_tentativa_em = CASE WHEN versao <> ?2 THEN ?6 ELSE ?7 END,
                     ultimo_erro = ?8,
                     atualizado_em = ?6
                 WHERE id = ?1 AND status = 'IN_FLIGHT'
                 RETURNING status",
            )?
            .query_row(
                params![
                    id,
                    versao_enviada,
                    desiste,
                    gasta,
                    max_tentativas.max(1),
                    agora,
                    proxima,
                    sanitizar_detalhe(&erro.to_string())
                ],
                |r| r.get(0),
            )
            .optional()?;
        status_devolvido(status, id)
    }

    /// Devolve à fila o que ficou `IN_FLIGHT` (o aplicativo caiu no meio de um envio).
    /// Chamar ao abrir, ANTES de ligar o worker; nunca com um envio em andamento.
    pub fn recuperar_em_voo(&self) -> Result<usize> {
        Ok(self
            .conn
            .prepare_cached(
                "UPDATE outbox SET status = 'PENDING_SYNC', atualizado_em = ?1
                 WHERE status = 'IN_FLIGHT'",
            )?
            .execute([self.agora()])?)
    }

    /// O professor pediu para tentar de novo o que falhou.
    pub fn reabrir_falhas(&self) -> Result<usize> {
        Ok(self
            .conn
            .prepare_cached(
                "UPDATE outbox SET status = 'PENDING_SYNC', tentativas = 0,
                     proxima_tentativa_em = ?1, atualizado_em = ?1
                 WHERE status = 'FAILED'",
            )?
            .execute([self.agora()])?)
    }

    /// O professor desistiu do envio (tela de Pendências: "usar a versão do sistema oficial").
    /// Apaga o item `PENDING_SYNC` ou `FAILED`; se for uma chamada, a aula entra em
    /// `aulas_a_recoletar` (a versão do DC precisa ser buscada de novo: a coleta diferencial não a
    /// traria). Devolve `false` se o item não existe, já foi sincronizado ou está sendo enviado
    /// (`IN_FLIGHT`: espere o envio terminar e tente de novo).
    pub fn descartar_item_outbox(&mut self, id: i64) -> Result<bool> {
        let agora = self.agora();
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let chave: Option<String> = tx
            .prepare_cached(
                "DELETE FROM outbox WHERE id = ?1 AND status IN ('PENDING_SYNC', 'FAILED')
                 RETURNING chave_idempotencia",
            )?
            .query_row([id], |r| r.get(0))
            .optional()?;
        let Some(chave) = chave else {
            return Ok(false);
        };
        if let Some(aula_id) = chave.strip_prefix(&chave_frequencia("")) {
            crate::repositorios::marcar_recoleta(&tx, aula_id, Plataforma::DiarioClasse, agora)?;
        }
        tx.commit()?;
        Ok(true)
    }

    pub fn obter_item_outbox(&self, id: i64) -> Result<Option<ItemOutbox>> {
        obter(
            &self.conn,
            concat!("SELECT ", colunas_outbox!(), " FROM outbox WHERE id = ?1"),
            [id],
            item_da_linha,
        )
    }

    /// Itens da fila, dos mais antigos aos mais novos (`None` = todos os estados).
    pub fn listar_outbox(&self, status: Option<StatusSincronizacao>) -> Result<Vec<ItemOutbox>> {
        listar(
            &self.conn,
            concat!(
                "SELECT ",
                colunas_outbox!(),
                " FROM outbox WHERE (?1 IS NULL OR status = ?1) ORDER BY criado_em, id"
            ),
            [status.map(StatusSincronizacao::as_str)],
            item_da_linha,
        )
    }

    pub fn contar_outbox(&self) -> Result<ContagemOutbox> {
        let mut contagem = ContagemOutbox::default();
        let mut stmt = self
            .conn
            .prepare_cached("SELECT status, COUNT(*) FROM outbox GROUP BY status")?;
        let linhas = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (status, n) in linhas {
            let n = u64::try_from(n).unwrap_or(0);
            match StatusSincronizacao::parse(&status)? {
                StatusSincronizacao::PendingSync => contagem.pendentes = n,
                StatusSincronizacao::InFlight => contagem.em_voo = n,
                StatusSincronizacao::Synced => contagem.sincronizados = n,
                StatusSincronizacao::Failed => contagem.falhas = n,
            }
        }
        Ok(contagem)
    }

    /// Apaga os itens já sincronizados antes de `antes_de_ms` (o dado continua nas tabelas).
    /// Use um limite bem anterior ao início da coleta mais antiga em andamento (ex.: 1 dia): o item
    /// `SYNCED` recente é o que protege a chamada de um lote lido antes do envio (`aplicar_lote`).
    pub fn limpar_sincronizados(&self, antes_de_ms: i64) -> Result<usize> {
        Ok(self
            .conn
            .prepare_cached("DELETE FROM outbox WHERE status = 'SYNCED' AND atualizado_em < ?1")?
            .execute([antes_de_ms])?)
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::apoio::{BancoTeste, MINUTO};
    use ordoc_core::StatusSincronizacao::{Failed, InFlight, PendingSync, Synced};
    use serde_json::json;

    fn nota(chave: &str, valor: u32) -> OperacaoRemota {
        OperacaoRemota {
            chave_idempotencia: chave.into(),
            plataforma: Plataforma::DiarioClasse,
            tipo: "nota".into(),
            payload: json!({"valor": valor}),
        }
    }

    fn timeout() -> ErroConector {
        ErroConector::Timeout("sem resposta".into())
    }

    #[test]
    fn ciclo_completo_com_backoff() {
        let t = BancoTeste::novo();
        let id = t.banco.enfileirar(&nota("dc:nota:1", 7)).unwrap();
        assert_eq!(t.banco.contar_outbox().unwrap().pendentes, 1);

        let lote = t.banco.reservar_lote(10).unwrap();
        assert_eq!(lote.len(), 1);
        assert_eq!(
            (lote[0].id, lote[0].status, lote[0].versao),
            (id, InFlight, 1)
        );
        assert!(
            t.banco.reservar_lote(10).unwrap().is_empty(),
            "nada reservado duas vezes"
        );

        // Timeout: volta à fila daqui a 5 minutos, com 1 tentativa gasta.
        let status = t
            .banco
            .marcar_falha(id, 1, &timeout(), 5 * MINUTO, 3)
            .unwrap();
        assert_eq!(status, PendingSync);
        let item = t.banco.obter_item_outbox(id).unwrap().unwrap();
        assert_eq!(item.tentativas, 1);
        assert_eq!(item.proxima_tentativa_em_ms, t.agora() + 5 * MINUTO);
        assert!(
            t.banco.reservar_lote(10).unwrap().is_empty(),
            "ainda não venceu"
        );

        t.avancar(5 * MINUTO);
        let lote = t.banco.reservar_lote(10).unwrap();
        assert_eq!(lote.len(), 1);
        assert_eq!(t.banco.marcar_sincronizado(id, 1).unwrap(), Synced);
        let contagem = t.banco.contar_outbox().unwrap();
        assert_eq!((contagem.pendentes, contagem.sincronizados), (0, 1));

        // Salvar de novo o mesmo conteúdo de um SYNCED rearma (o DC pode ter mudado depois).
        assert_eq!(t.banco.enfileirar(&nota("dc:nota:1", 7)).unwrap(), id);
        let item = t.banco.obter_item_outbox(id).unwrap().unwrap();
        assert_eq!((item.status, item.versao), (PendingSync, 2));
        // Mesmo conteúdo com o item ainda pendente: nada muda.
        assert_eq!(t.banco.enfileirar(&nota("dc:nota:1", 7)).unwrap(), id);
        assert_eq!(t.banco.obter_item_outbox(id).unwrap().unwrap().versao, 2);
        // Conteúdo novo: versão 3, no mesmo item.
        assert_eq!(t.banco.enfileirar(&nota("dc:nota:1", 9)).unwrap(), id);
        let item = t.banco.obter_item_outbox(id).unwrap().unwrap();
        assert_eq!(
            (item.status, item.versao, item.tentativas),
            (PendingSync, 3, 0)
        );
        assert_eq!(item.operacao.payload, json!({"valor": 9}));
        // LGPD: o Debug do item não mostra o payload (onde ficam os RAs).
        let texto = format!("{item:?}");
        assert!(
            texto.contains("dc:nota:1") && !texto.contains("valor"),
            "{texto}"
        );
    }

    #[test]
    fn edicao_durante_o_envio_nao_se_perde_nem_sai_em_paralelo() {
        let t = BancoTeste::novo();
        let id = t.banco.enfileirar(&nota("dc:nota:2", 5)).unwrap();
        let enviado = t.banco.reservar_lote(1).unwrap().remove(0);
        assert_eq!(enviado.versao, 1);

        // O professor corrige a nota enquanto a versão 1 está sendo enviada.
        t.banco.enfileirar(&nota("dc:nota:2", 6)).unwrap();
        let item = t.banco.obter_item_outbox(id).unwrap().unwrap();
        assert_eq!((item.status, item.versao), (InFlight, 2));
        assert!(
            t.banco.reservar_lote(10).unwrap().is_empty(),
            "a versão 2 espera a 1 terminar"
        );

        // A versão 1 chegou: o item NÃO fica SYNCED, porque a 2 ainda precisa sair.
        assert_eq!(t.banco.marcar_sincronizado(id, 1).unwrap(), PendingSync);
        let proximo = t.banco.reservar_lote(1).unwrap().remove(0);
        assert_eq!(proximo.versao, 2);
        assert_eq!(proximo.operacao.payload, json!({"valor": 6}));
        assert_eq!(t.banco.marcar_sincronizado(id, 2).unwrap(), Synced);
    }

    #[test]
    fn falha_da_versao_velha_nao_gasta_tentativa_da_nova() {
        let t = BancoTeste::novo();
        let id = t.banco.enfileirar(&nota("dc:nota:3", 1)).unwrap();
        t.banco.reservar_lote(1).unwrap();
        t.banco.enfileirar(&nota("dc:nota:3", 2)).unwrap();
        let recusa = ErroConector::Recusada("nota fora da faixa".into());
        assert_eq!(
            t.banco.marcar_falha(id, 1, &recusa, 0, 3).unwrap(),
            PendingSync
        );
        let item = t.banco.obter_item_outbox(id).unwrap().unwrap();
        assert_eq!((item.versao, item.tentativas), (2, 0));
    }

    #[test]
    fn sessao_expirada_espera_o_login_sem_gastar_tentativa() {
        let t = BancoTeste::novo();
        let id = t.banco.enfileirar(&nota("dc:nota:4", 3)).unwrap();
        let sessao = ErroConector::SessaoExpirada("redirecionado para o login".into());
        for _ in 0..5 {
            t.banco.reservar_lote(1).unwrap();
            assert_eq!(
                t.banco.marcar_falha(id, 1, &sessao, 0, 2).unwrap(),
                PendingSync
            );
        }
        assert_eq!(
            t.banco.obter_item_outbox(id).unwrap().unwrap().tentativas,
            0
        );
    }

    #[test]
    fn tentativas_esgotadas_e_recusa_viram_failed_e_o_professor_reabre() {
        let t = BancoTeste::novo();
        let a = t.banco.enfileirar(&nota("dc:nota:5", 1)).unwrap();
        let b = t.banco.enfileirar(&nota("dc:nota:6", 1)).unwrap();

        t.banco.reservar_lote(10).unwrap();
        assert_eq!(
            t.banco.marcar_falha(a, 1, &timeout(), 0, 2).unwrap(),
            PendingSync
        );
        let recusa = ErroConector::Recusada("aula já fechada".into());
        assert_eq!(t.banco.marcar_falha(b, 1, &recusa, 0, 8).unwrap(), Failed);

        t.banco.reservar_lote(10).unwrap();
        assert_eq!(
            t.banco.marcar_falha(a, 1, &timeout(), 0, 2).unwrap(),
            Failed
        );
        assert_eq!(t.banco.contar_outbox().unwrap().falhas, 2);
        assert_eq!(t.banco.listar_outbox(Some(Failed)).unwrap().len(), 2);

        // Salvar de novo o mesmo conteúdo de um FAILED rearma o item.
        t.banco.enfileirar(&nota("dc:nota:6", 1)).unwrap();
        assert_eq!(
            t.banco.obter_item_outbox(b).unwrap().unwrap().status,
            PendingSync
        );
        // "Tentar de novo" na tela de pendências: reabre o resto.
        assert_eq!(t.banco.reabrir_falhas().unwrap(), 1);
        let item = t.banco.obter_item_outbox(a).unwrap().unwrap();
        assert_eq!((item.status, item.tentativas), (PendingSync, 0));
    }

    #[test]
    fn recuperacao_depois_de_queda_do_aplicativo() {
        let t = BancoTeste::novo();
        t.banco.enfileirar(&nota("dc:nota:7", 1)).unwrap();
        t.banco.enfileirar(&nota("dc:nota:8", 1)).unwrap();
        assert_eq!(t.banco.reservar_lote(10).unwrap().len(), 2);
        // O aplicativo caiu aqui. Ao abrir de novo:
        assert_eq!(t.banco.recuperar_em_voo().unwrap(), 2);
        assert_eq!(t.banco.contar_outbox().unwrap().pendentes, 2);
    }

    #[test]
    fn marcar_item_que_nao_esta_em_voo_e_erro() {
        let t = BancoTeste::novo();
        let id = t.banco.enfileirar(&nota("dc:nota:9", 1)).unwrap();
        assert!(matches!(
            t.banco.marcar_sincronizado(id, 1),
            Err(StorageError::ItemNaoReservado(i)) if i == id
        ));
        assert!(matches!(
            t.banco.marcar_falha(999, 1, &timeout(), 0, 3),
            Err(StorageError::ItemNaoReservado(999))
        ));
    }

    #[test]
    fn erro_gravado_sem_o_segredo_da_url() {
        let t = BancoTeste::novo();
        let id = t.banco.enfileirar(&nota("dc:nota:10", 1)).unwrap();
        t.banco.reservar_lote(1).unwrap();
        let erro = ErroConector::Rede("falhou https://sed.exemplo.org/api?token=SEGREDO".into());
        t.banco.marcar_falha(id, 1, &erro, 0, 3).unwrap();
        let item = t.banco.obter_item_outbox(id).unwrap().unwrap();
        let texto = item.ultimo_erro.unwrap();
        assert!(texto.ends_with("https://sed.exemplo.org/api"), "{texto}");
        assert!(!texto.contains("SEGREDO"));
    }

    #[test]
    fn limpeza_dos_sincronizados_antigos() {
        let t = BancoTeste::novo();
        let id = t.banco.enfileirar(&nota("dc:nota:11", 1)).unwrap();
        t.banco.reservar_lote(1).unwrap();
        t.banco.marcar_sincronizado(id, 1).unwrap();
        assert_eq!(t.banco.limpar_sincronizados(t.agora()).unwrap(), 0);
        t.avancar(MINUTO);
        assert_eq!(t.banco.limpar_sincronizados(t.agora()).unwrap(), 1);
        assert!(t.banco.obter_item_outbox(id).unwrap().is_none());
    }

    #[test]
    fn operacao_sem_chave_ou_tipo_e_recusada() {
        let t = BancoTeste::novo();
        let mut op = nota(" ", 1);
        assert!(matches!(
            t.banco.enfileirar(&op),
            Err(StorageError::Dominio(CoreError::IdentificadorAusente(_)))
        ));
        op.chave_idempotencia = "dc:nota:12".into();
        op.tipo = String::new();
        assert!(t.banco.enfileirar(&op).is_err());
        assert_eq!(t.banco.contar_outbox().unwrap(), ContagemOutbox::default());
    }

    #[test]
    fn descartar_so_o_que_nao_esta_em_voo() {
        let mut t = BancoTeste::novo();
        let a = t.banco.enfileirar(&nota("dc:nota:13", 1)).unwrap();
        let b = t.banco.enfileirar(&nota("dc:nota:14", 1)).unwrap();
        t.banco.reservar_lote(10).unwrap();
        let recusa = ErroConector::Recusada("aula já fechada".into());
        assert_eq!(t.banco.marcar_falha(a, 1, &recusa, 0, 8).unwrap(), Failed);
        assert!(t.banco.descartar_item_outbox(a).unwrap());
        assert!(t.banco.obter_item_outbox(a).unwrap().is_none());
        assert!(!t.banco.descartar_item_outbox(b).unwrap(), "IN_FLIGHT fica");
        assert!(!t.banco.descartar_item_outbox(999).unwrap());
        assert_eq!(t.banco.contar_outbox().unwrap().em_voo, 1);
    }

    #[test]
    fn chave_da_chamada() {
        assert_eq!(chave_frequencia("A-10"), "dc:frequencia:A-10");
    }
}
