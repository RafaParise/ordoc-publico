//! Telemetria dos conectores (o último estado de cada plataforma, para o Modo Diagnóstico) e os
//! cursores da sincronização diferencial.

use ordoc_core::{sanitizar_detalhe, Plataforma, StatusConector, TelemetriaConector};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::sql::{enum_da_coluna, listar, obter};
use crate::BancoOrdoc;

/// O estado de um conector e quando ele funcionou pela última vez.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EstadoConector {
    pub telemetria: TelemetriaConector,
    pub ultimo_ok_em_ms: Option<i64>,
}

fn estado_da_linha(row: &Row<'_>) -> rusqlite::Result<EstadoConector> {
    Ok(EstadoConector {
        telemetria: TelemetriaConector {
            plataforma: enum_da_coluna(row, 0, Plataforma::parse)?,
            status: enum_da_coluna(row, 1, StatusConector::parse)?,
            detalhe: row.get(2)?,
            registrado_em_ms: row.get(3)?,
        },
        ultimo_ok_em_ms: row.get(4)?,
    })
}

/// Grava o cursor da plataforma (dentro da conexão ou da transação recebida).
pub(crate) fn upsert_cursor(
    conn: &Connection,
    plataforma: Plataforma,
    cursor: &str,
    agora: i64,
) -> Result<()> {
    conn.prepare_cached(
        "INSERT INTO cursores_sync (plataforma, valor, atualizado_em) VALUES (?1, ?2, ?3)
         ON CONFLICT (plataforma) DO UPDATE SET
             valor = excluded.valor,
             atualizado_em = excluded.atualizado_em",
    )?
    .execute(params![plataforma.as_str(), cursor, agora])?;
    Ok(())
}

impl BancoOrdoc {
    /// Grava o estado do conector (o detalhe é sanitizado). Um registro mais antigo que o gravado
    /// não troca o estado (o envio e a coleta podem terminar fora de ordem) e devolve `false`; mas
    /// um OK atrasado ainda atualiza o "último OK" se for mais novo que o gravado.
    pub fn registrar_telemetria(&self, telemetria: &TelemetriaConector) -> Result<bool> {
        let detalhe = telemetria.detalhe.as_deref().map(sanitizar_detalhe);
        let virou_atual: Option<bool> = self
            .conn
            .prepare_cached(
                "INSERT INTO telemetria_conectores (plataforma, status, detalhe, registrado_em, ultimo_ok_em)
                 VALUES (?1, ?2, ?3, ?4, CASE WHEN ?2 = 'OK' THEN ?4 END)
                 ON CONFLICT (plataforma) DO UPDATE SET
                     status = CASE WHEN excluded.registrado_em >= telemetria_conectores.registrado_em
                                   THEN excluded.status ELSE telemetria_conectores.status END,
                     detalhe = CASE WHEN excluded.registrado_em >= telemetria_conectores.registrado_em
                                    THEN excluded.detalhe ELSE telemetria_conectores.detalhe END,
                     registrado_em = MAX(excluded.registrado_em, telemetria_conectores.registrado_em),
                     ultimo_ok_em = CASE
                         WHEN excluded.ultimo_ok_em IS NULL THEN telemetria_conectores.ultimo_ok_em
                         WHEN telemetria_conectores.ultimo_ok_em IS NULL THEN excluded.ultimo_ok_em
                         ELSE MAX(excluded.ultimo_ok_em, telemetria_conectores.ultimo_ok_em) END
                 WHERE excluded.registrado_em >= telemetria_conectores.registrado_em
                    OR excluded.ultimo_ok_em > COALESCE(telemetria_conectores.ultimo_ok_em, -1)
                 RETURNING registrado_em = ?4",
            )?
            .query_row(
                params![
                    telemetria.plataforma.as_str(),
                    telemetria.status.as_str(),
                    detalhe,
                    telemetria.registrado_em_ms
                ],
                |r| r.get(0),
            )
            .optional()?;
        Ok(virou_atual.unwrap_or(false))
    }

    pub fn estado_conector(&self, plataforma: Plataforma) -> Result<Option<EstadoConector>> {
        obter(
            &self.conn,
            "SELECT plataforma, status, detalhe, registrado_em, ultimo_ok_em
             FROM telemetria_conectores WHERE plataforma = ?1",
            [plataforma.as_str()],
            estado_da_linha,
        )
    }

    /// Todos os conectores com estado registrado (para o Modo Diagnóstico).
    pub fn listar_estado_conectores(&self) -> Result<Vec<EstadoConector>> {
        listar(
            &self.conn,
            "SELECT plataforma, status, detalhe, registrado_em, ultimo_ok_em
             FROM telemetria_conectores ORDER BY plataforma",
            [],
            estado_da_linha,
        )
    }

    /// Cursor da última coleta da plataforma (`None` = nunca coletou: carga completa).
    pub fn obter_cursor(&self, plataforma: Plataforma) -> Result<Option<String>> {
        obter(
            &self.conn,
            "SELECT valor FROM cursores_sync WHERE plataforma = ?1",
            [plataforma.as_str()],
            |r| r.get(0),
        )
    }

    /// Grava o cursor fora de um lote (o `aplicar_lote` já grava o do lote junto com os dados).
    pub fn salvar_cursor(&self, plataforma: Plataforma, cursor: &str) -> Result<()> {
        upsert_cursor(&self.conn, plataforma, cursor, self.agora())
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::apoio::{BancoTeste, MINUTO};
    use ordoc_core::ErroConector;

    #[test]
    fn telemetria_guarda_o_ultimo_ok_e_ignora_registro_atrasado() {
        let t = BancoTeste::novo();
        let dc = Plataforma::DiarioClasse;
        assert!(t.banco.estado_conector(dc).unwrap().is_none());

        assert!(t
            .banco
            .registrar_telemetria(&TelemetriaConector::ok(dc, 1_000))
            .unwrap());
        let sessao = ErroConector::SessaoExpirada(
            "redirecionado para https://login.exemplo.org/?ticket=SEGREDO".into(),
        );
        let falha = TelemetriaConector::de_erro(dc, &sessao, 2_000).unwrap();
        assert!(t.banco.registrar_telemetria(&falha).unwrap());

        let estado = t.banco.estado_conector(dc).unwrap().unwrap();
        assert_eq!(estado.telemetria.status, StatusConector::SessaoExpirada);
        assert_eq!(
            estado.ultimo_ok_em_ms,
            Some(1_000),
            "o último OK continua lá"
        );
        assert!(!estado.telemetria.detalhe.unwrap().contains("SEGREDO"));

        // Um OK mais antigo que chega atrasado não apaga o erro mais novo, mas atualiza o último OK.
        assert!(!t
            .banco
            .registrar_telemetria(&TelemetriaConector::ok(dc, 1_500))
            .unwrap());
        let estado = t.banco.estado_conector(dc).unwrap().unwrap();
        assert_eq!(estado.telemetria.status, StatusConector::SessaoExpirada);
        assert_eq!(estado.telemetria.registrado_em_ms, 2_000);
        assert_eq!(estado.ultimo_ok_em_ms, Some(1_500));
        // Um OK ainda mais velho que o último OK não muda nada.
        assert!(!t
            .banco
            .registrar_telemetria(&TelemetriaConector::ok(dc, 500))
            .unwrap());
        assert_eq!(
            t.banco
                .estado_conector(dc)
                .unwrap()
                .unwrap()
                .ultimo_ok_em_ms,
            Some(1_500)
        );

        // Detalhe montado à mão (sem passar pelo core) também é sanitizado ao gravar.
        let manual = TelemetriaConector {
            plataforma: Plataforma::PainelBi,
            status: StatusConector::Timeout,
            detalhe: Some("GET https://bi.exemplo.org/painel?code=SEGREDO".into()),
            registrado_em_ms: 3_000,
        };
        t.banco.registrar_telemetria(&manual).unwrap();
        let lista = t.banco.listar_estado_conectores().unwrap();
        assert_eq!(lista.len(), 2);
        assert!(lista.iter().all(|e| !e
            .telemetria
            .detalhe
            .clone()
            .unwrap_or_default()
            .contains("SEGREDO")));
    }

    #[test]
    fn cursor_por_plataforma() {
        let t = BancoTeste::novo();
        assert_eq!(
            t.banco.obter_cursor(Plataforma::SalaDoFuturo).unwrap(),
            None
        );
        t.banco
            .salvar_cursor(Plataforma::SalaDoFuturo, "2026-11-13T10:00:00Z")
            .unwrap();
        t.avancar(MINUTO);
        t.banco
            .salvar_cursor(Plataforma::SalaDoFuturo, "2026-11-14T10:00:00Z")
            .unwrap();
        assert_eq!(
            t.banco
                .obter_cursor(Plataforma::SalaDoFuturo)
                .unwrap()
                .as_deref(),
            Some("2026-11-14T10:00:00Z")
        );
        assert_eq!(
            t.banco.obter_cursor(Plataforma::DiarioClasse).unwrap(),
            None
        );
    }
}
