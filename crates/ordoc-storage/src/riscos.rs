//! Resumos por aluno (a entrada do motor `alert_rules` do ordoc-core) e os indicadores de risco.
//!
//! O resumo é calculado no SQLite, numa consulta só por turma:
//! - frequência com o peso da aula (aula dupla = 2); falta justificada separada;
//! - atividades da Sala do Futuro "cobráveis": já entregues, sem prazo, ou com o prazo vencido.
//!   Atividade ainda no prazo e não entregue não conta (senão toda atividade recém-publicada
//!   geraria alerta para a turma inteira). Prazo só com a data vale até o fim do dia; aceita ISO
//!   (`2026-11-20`, `2026-11-20T23:59:00Z`) e o formato brasileiro (`20/11/2026`). Prazo em formato
//!   desconhecido conta como vencido: melhor um aviso a mais do que um aluno esquecido.
//! - Também aceita o JSON antigo do .NET (`/Date(ms)/`) e fuso sem dois-pontos (`-0300`, `-03`).
//!   Data brasileira com dia ou mês de um dígito (`5/11/2026`) ainda cai em "formato desconhecido".
//! - Fuso: prazo com fuso (`Z` ou `±HH:MM`) é comparado em UTC; prazo sem fuso (só a data, ISO sem
//!   offset ou brasileiro) é hora LOCAL do professor (`BancoOrdoc::definir_fuso_minutos`, padrão
//!   Brasília). Sem isso, "vence hoje" viraria cobrado às 21 h.
//! - Aluno sem linha em `entregas_sdf` não é cobrado: sem a visão por aluno, o Ordoc não afirma que
//!   alguém deixou de entregar (decisão do ordoc-core). Confirmar com a resposta real da SDF se ela
//!   lista todos os alunos (com entregue = falso) ou só quem entregou; no segundo caso, mudar aqui.

use std::collections::HashSet;

use ordoc_core::{CoreError, IndicadorRisco, MotorAlertas, ResumoAluno, Severidade};
use rusqlite::{params, Row, TransactionBehavior};
use serde::{Deserialize, Serialize};

use crate::error::{Result, StorageError};
use crate::sql::{enum_da_coluna, json_da_coluna, json_texto};
use crate::BancoOrdoc;

/// O que mudou ao aplicar os indicadores de uma turma.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultadoRiscos {
    pub novos: usize,
    pub mantidos: usize,
    pub resolvidos: usize,
}

/// Um indicador ativo, com as datas para a tela ("desde quando").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RiscoRegistrado {
    pub indicador: IndicadorRisco,
    pub detectado_em_ms: i64,
    pub atualizado_em_ms: i64,
}

const SQL_RESUMOS: &str = "
WITH agora AS (
    SELECT julianday(?2 / 1000.0, 'unixepoch') AS utc,
           julianday(?2 / 1000.0, 'unixepoch', ?3) AS local
),
freq AS (
    SELECT f.aluno_ra,
           SUM(a.quantidade) AS aulas,
           SUM(CASE WHEN f.status = 'FALTA' THEN a.quantidade ELSE 0 END) AS faltas,
           SUM(CASE WHEN f.status = 'FALTA_JUSTIFICADA' THEN a.quantidade ELSE 0 END) AS justificadas
    FROM frequencias f
    JOIN aulas a ON a.id = f.aula_id
    WHERE a.turma_id = ?1
    GROUP BY f.aluno_ra
),
ent AS (
    SELECT e.aluno_ra,
           SUM(CASE
                 WHEN e.entregue = 1 THEN 1
                 WHEN s.prazo_iso IS NULL OR trim(s.prazo_iso) = '' THEN 1
                 WHEN length(s.prazo_iso) = 10 AND julianday(s.prazo_iso) IS NOT NULL
                      THEN julianday(s.prazo_iso, '+1 day') <= (SELECT local FROM agora)
                 WHEN julianday(s.prazo_iso) IS NOT NULL
                      AND (s.prazo_iso GLOB '*[0-9][Zz]'
                           OR s.prazo_iso GLOB '*[0-9][+-][0-9][0-9]:[0-9][0-9]')
                      THEN julianday(s.prazo_iso) <= (SELECT utc FROM agora)
                 WHEN julianday(s.prazo_iso) IS NOT NULL
                      THEN julianday(s.prazo_iso) <= (SELECT local FROM agora)
                 WHEN s.prazo_iso GLOB '[0-9][0-9]/[0-9][0-9]/[0-9][0-9][0-9][0-9]*'
                      THEN julianday(substr(s.prazo_iso, 7, 4) || '-' || substr(s.prazo_iso, 4, 2)
                                     || '-' || substr(s.prazo_iso, 1, 2), '+1 day')
                           <= (SELECT local FROM agora)
                 -- JSON antigo do .NET: /Date(1763693940000)/ ou /Date(1763693940000-0300)/ (ms UTC)
                 WHEN s.prazo_iso GLOB '/Date([0-9]*'
                      THEN CAST(substr(s.prazo_iso, 7) AS INTEGER) <= ?2
                 -- fuso sem dois-pontos (+0300) ou só com a hora (-03)
                 WHEN s.prazo_iso GLOB '*T*[0-9][+-][0-9][0-9][0-9][0-9]'
                      AND julianday(substr(s.prazo_iso, 1, length(s.prazo_iso) - 2) || ':'
                                    || substr(s.prazo_iso, -2)) IS NOT NULL
                      THEN julianday(substr(s.prazo_iso, 1, length(s.prazo_iso) - 2) || ':'
                                     || substr(s.prazo_iso, -2)) <= (SELECT utc FROM agora)
                 WHEN s.prazo_iso GLOB '*T*[0-9][+-][0-9][0-9]'
                      AND julianday(s.prazo_iso || ':00') IS NOT NULL
                      THEN julianday(s.prazo_iso || ':00') <= (SELECT utc FROM agora)
                 ELSE 1
               END) AS cobradas,
           SUM(e.entregue) AS entregues
    FROM entregas_sdf e
    JOIN atividades_sdf s ON s.id = e.atividade_id
    JOIN turmas t ON t.codigo_sdf = s.turma_codigo_sdf
    WHERE t.id = ?1
    GROUP BY e.aluno_ra
)
SELECT m.aluno_ra,
       COALESCE(freq.aulas, 0), COALESCE(freq.faltas, 0), COALESCE(freq.justificadas, 0),
       COALESCE(ent.cobradas, 0), COALESCE(ent.entregues, 0)
FROM matriculas m
LEFT JOIN freq ON freq.aluno_ra = m.aluno_ra
LEFT JOIN ent ON ent.aluno_ra = m.aluno_ra
WHERE m.turma_id = ?1
ORDER BY m.numero_chamada IS NULL, m.numero_chamada, m.aluno_ra";

const SQL_UPSERT_RISCO: &str = "
INSERT INTO indicadores_risco (regra, turma_id, aluno_ra, severidade, mensagem, evidencias,
                               detectado_em, atualizado_em, resolvido_em)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, NULL)
ON CONFLICT (regra, turma_id, aluno_ra) DO UPDATE SET
    severidade = excluded.severidade,
    mensagem = excluded.mensagem,
    evidencias = excluded.evidencias,
    detectado_em = CASE WHEN indicadores_risco.resolvido_em IS NULL
                        THEN indicadores_risco.detectado_em ELSE excluded.detectado_em END,
    atualizado_em = excluded.atualizado_em,
    resolvido_em = NULL";

fn risco_da_linha(row: &Row<'_>) -> rusqlite::Result<RiscoRegistrado> {
    Ok(RiscoRegistrado {
        indicador: IndicadorRisco {
            regra: row.get(0)?,
            severidade: enum_da_coluna(row, 1, Severidade::parse)?,
            turma_id: row.get(2)?,
            aluno_ra: row.get(3)?,
            mensagem: row.get(4)?,
            evidencias: json_da_coluna(row, 5)?,
        },
        detectado_em_ms: row.get(6)?,
        atualizado_em_ms: row.get(7)?,
    })
}

impl BancoOrdoc {
    /// Um resumo por aluno matriculado na turma, na ordem da chamada.
    pub fn resumos_da_turma(&self, turma_id: &str) -> Result<Vec<ResumoAluno>> {
        let mut stmt = self.conn.prepare_cached(SQL_RESUMOS)?;
        let resumos = stmt
            .query_map(
                params![
                    turma_id,
                    self.agora(),
                    format!("{} minutes", self.fuso_minutos)
                ],
                |row| {
                    Ok(ResumoAluno {
                        turma_id: turma_id.to_string(),
                        aluno_ra: row.get(0)?,
                        aulas_registradas: row.get(1)?,
                        faltas: row.get(2)?,
                        faltas_justificadas: row.get(3)?,
                        atividades_cobradas: row.get(4)?,
                        atividades_entregues: row.get(5)?,
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(resumos)
    }

    /// Deixa os indicadores ATIVOS da turma iguais a `indicadores`: os novos entram, os que
    /// continuam são atualizados (mantendo a data em que foram detectados) e os que sumiram ficam
    /// resolvidos (com a data). Um indicador resolvido que volta conta como novo.
    pub fn aplicar_indicadores_risco(
        &mut self,
        turma_id: &str,
        indicadores: &[IndicadorRisco],
    ) -> Result<ResultadoRiscos> {
        for ind in indicadores {
            if ind.turma_id != turma_id {
                return Err(StorageError::TurmaDivergente {
                    esperada: turma_id.to_string(),
                    encontrada: ind.turma_id.clone(),
                });
            }
            if ind.regra.trim().is_empty() {
                return Err(CoreError::IdentificadorAusente("indicador_risco.regra").into());
            }
            if ind.aluno_ra.trim().is_empty() {
                return Err(CoreError::RaAusente.into());
            }
        }

        let agora = self.agora();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ativos: HashSet<(String, String)> = {
            let mut stmt = tx.prepare_cached(
                "SELECT regra, aluno_ra FROM indicadores_risco
                 WHERE turma_id = ?1 AND resolvido_em IS NULL",
            )?;
            let chaves = stmt
                .query_map([turma_id], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<HashSet<(String, String)>>>()?;
            chaves
        };

        let mut atuais = HashSet::new();
        for ind in indicadores {
            tx.prepare_cached(SQL_UPSERT_RISCO)?.execute(params![
                ind.regra,
                turma_id,
                ind.aluno_ra,
                ind.severidade.as_str(),
                ind.mensagem,
                json_texto(&ind.evidencias)?,
                agora
            ])?;
            atuais.insert((ind.regra.clone(), ind.aluno_ra.clone()));
        }

        let mut resultado = ResultadoRiscos::default();
        for chave in &atuais {
            if ativos.contains(chave) {
                resultado.mantidos += 1;
            } else {
                resultado.novos += 1;
            }
        }
        for (regra, ra) in ativos.difference(&atuais) {
            tx.prepare_cached(
                "UPDATE indicadores_risco SET resolvido_em = ?4, atualizado_em = ?4
                 WHERE regra = ?1 AND turma_id = ?2 AND aluno_ra = ?3",
            )?
            .execute(params![regra, turma_id, ra, agora])?;
            resultado.resolvidos += 1;
        }
        tx.commit()?;
        Ok(resultado)
    }

    /// Calcula os resumos da turma, roda o motor e grava os indicadores. Tudo local, sem internet.
    pub fn avaliar_alertas_da_turma(
        &mut self,
        turma_id: &str,
        motor: &MotorAlertas,
    ) -> Result<(Vec<IndicadorRisco>, ResultadoRiscos)> {
        let resumos = self.resumos_da_turma(turma_id)?;
        let indicadores = motor.avaliar(&resumos);
        let resultado = self.aplicar_indicadores_risco(turma_id, &indicadores)?;
        Ok((indicadores, resultado))
    }

    /// Indicadores ativos (de uma turma, ou de todas com `None`): crítico primeiro.
    pub fn listar_riscos_ativos(&self, turma_id: Option<&str>) -> Result<Vec<RiscoRegistrado>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT regra, severidade, turma_id, aluno_ra, mensagem, evidencias, detectado_em, atualizado_em
             FROM indicadores_risco
             WHERE resolvido_em IS NULL AND (?1 IS NULL OR turma_id = ?1)
             ORDER BY CASE severidade WHEN 'CRITICO' THEN 0 ELSE 1 END, turma_id, aluno_ra, regra",
        )?;
        let riscos = stmt
            .query_map([turma_id], risco_da_linha)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(riscos)
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::apoio::{aula, presenca, BancoTeste, DIA};

    const HORA: i64 = DIA / 24;
    use ordoc_core::StatusPresenca::{Falta, FaltaJustificada, Presente};
    use ordoc_core::{
        AtividadeSdf, EntregaSdf, REGRA_FALTAS_SEM_ENTREGA, REGRA_FREQUENCIA_BAIXA,
        REGRA_SEM_ENTREGAS,
    };
    use serde_json::json;

    const RA1: &str = "000000001";
    const RA2: &str = "000000002";
    const RA3: &str = "000000003";

    fn atividade(id: &str, prazo: Option<&str>) -> AtividadeSdf {
        AtividadeSdf {
            id: id.into(),
            turma_codigo_sdf: "3B_SDF".into(),
            titulo: format!("Atividade {id}"),
            prazo_iso: prazo.map(Into::into),
            total_alunos: 3,
            total_entregues: 0,
            metadata_bruta: json!({}),
        }
    }

    fn entrega(atividade_id: &str, ra: &str, entregue: bool) -> EntregaSdf {
        EntregaSdf {
            atividade_id: atividade_id.into(),
            aluno_ra: ra.into(),
            entregue,
            entregue_em: None,
            metadata_bruta: json!({}),
        }
    }

    /// Turma T1 (SDF 3B_SDF) com 3 alunos e 4 aulas que somam 5 (uma dupla).
    fn turma_com_aulas(t: &mut BancoTeste) {
        t.preparar_turma("T1", Some("3B_SDF"), &[RA1, RA2, RA3]);
        let aulas = [
            ("A1", "2026-11-03", 1),
            ("A2", "2026-11-05", 2),
            ("A3", "2026-11-10", 1),
            ("A4", "2026-11-12", 1),
        ];
        for (id, data, qtd) in aulas {
            let chamada = vec![
                presenca(id, RA1, if id == "A2" { Falta } else { Presente }),
                presenca(id, RA2, Presente),
                presenca(
                    id,
                    RA3,
                    if id == "A1" {
                        FaltaJustificada
                    } else {
                        Presente
                    },
                ),
            ];
            t.banco
                .registrar_chamada(&aula(id, "T1", data, qtd), &chamada)
                .unwrap();
        }
    }

    #[test]
    fn resumo_conta_peso_da_aula_justificada_e_prazo() {
        let mut t = BancoTeste::novo();
        turma_com_aulas(&mut t);
        // Agora = 13/11/2026 21:00 em Brasília (14/11 00:00 UTC). Prazos: vencidos, no prazo, só a
        // data, formato brasileiro, sem prazo e um formato desconhecido.
        for (id, prazo) in [
            ("vencida_iso", Some("2026-11-10T23:59:00Z")),
            ("no_prazo_iso", Some("2026-11-20T23:59:00Z")),
            ("hoje_so_data", Some("2026-11-14")),
            ("vencida_br", Some("10/11/2026")),
            ("no_prazo_br", Some("30/11/2026 23:59")),
            ("sem_prazo", None),
            ("formato_estranho", Some("semana 46")),
        ] {
            t.banco.salvar_atividade_sdf(&atividade(id, prazo)).unwrap();
            t.banco
                .salvar_entrega_sdf(&entrega(id, RA1, false))
                .unwrap();
        }
        // RA2 entregou uma atividade ainda no prazo: conta como cobrada e entregue.
        t.banco
            .salvar_entrega_sdf(&entrega("no_prazo_iso", RA2, true))
            .unwrap();

        let resumos = t.banco.resumos_da_turma("T1").unwrap();
        let ras: Vec<&str> = resumos.iter().map(|r| r.aluno_ra.as_str()).collect();
        assert_eq!(ras, vec![RA1, RA2, RA3], "na ordem da chamada");

        let r1 = &resumos[0];
        assert_eq!(
            (r1.aulas_registradas, r1.faltas),
            (5, 2),
            "a falta na aula dupla vale 2"
        );
        // Cobráveis para RA1: vencida_iso, vencida_br, sem_prazo e formato_estranho.
        // Não contam: no_prazo_iso, hoje_so_data (vale até o fim do dia) e no_prazo_br.
        assert_eq!((r1.atividades_cobradas, r1.atividades_entregues), (4, 0));

        let r2 = &resumos[1];
        assert_eq!(
            (r2.faltas, r2.atividades_cobradas, r2.atividades_entregues),
            (0, 1, 1)
        );

        let r3 = &resumos[2];
        assert_eq!((r3.faltas, r3.faltas_justificadas), (0, 1));
        assert_eq!(r3.atividades_cobradas, 0, "sem entrega registrada na SDF");

        // 14/11 às 22:00 de Brasília: o prazo "2026-11-14" ainda vale (vai até o fim do dia local).
        t.avancar(DIA + HORA);
        assert_eq!(
            t.banco.resumos_da_turma("T1").unwrap()[0].atividades_cobradas,
            4
        );
        // 15/11 00:00 de Brasília: passa a ser cobrada.
        t.avancar(2 * HORA);
        assert_eq!(
            t.banco.resumos_da_turma("T1").unwrap()[0].atividades_cobradas,
            5
        );
    }

    #[test]
    fn prazo_sem_fuso_e_hora_local_e_com_fuso_e_utc() {
        let t = BancoTeste::novo();
        t.preparar_turma("T1", Some("3B_SDF"), &[RA1]);
        for (id, prazo) in [
            ("so_data", "2026-11-14"),
            ("br", "14/11/2026"),
            ("sem_fuso", "2026-11-14T22:30:00"),
            ("com_fuso", "2026-11-14T22:30:00-03:00"),
        ] {
            t.banco
                .salvar_atividade_sdf(&atividade(id, Some(prazo)))
                .unwrap();
            t.banco
                .salvar_entrega_sdf(&entrega(id, RA1, false))
                .unwrap();
        }
        let cobradas =
            |t: &BancoTeste| t.banco.resumos_da_turma("T1").unwrap()[0].atividades_cobradas;
        t.avancar(DIA + HORA); // 14/11 22:00 em Brasília
        assert_eq!(cobradas(&t), 0);
        t.avancar(HORA); // 23:00: as duas de 22:30 venceram (sem fuso = local; com fuso = UTC)
        assert_eq!(cobradas(&t), 2);
        t.avancar(HORA); // 15/11 00:00: todas
        assert_eq!(cobradas(&t), 4);

        // Com fuso UTC, às 15/11 00:00 UTC (= 21:00 em Brasília) o de 22:30-03:00 ainda não venceu.
        let mut u = BancoTeste::novo();
        u.preparar_turma("T1", Some("3B_SDF"), &[RA1]);
        u.banco.definir_fuso_minutos(0);
        for (id, prazo) in [
            ("so_data", "2026-11-14"),
            ("sem_fuso", "2026-11-14T22:30:00"),
            ("com_fuso", "2026-11-14T22:30:00-03:00"),
        ] {
            u.banco
                .salvar_atividade_sdf(&atividade(id, Some(prazo)))
                .unwrap();
            u.banco
                .salvar_entrega_sdf(&entrega(id, RA1, false))
                .unwrap();
        }
        u.avancar(DIA);
        assert_eq!(cobradas(&u), 2);
    }

    #[test]
    fn prazo_em_formatos_do_dotnet() {
        let t = BancoTeste::novo();
        t.preparar_turma("T1", Some("3B_SDF"), &[RA1]);
        let ms = |dias: i64| t.agora() + dias * DIA;
        for (id, prazo) in [
            ("date_passado", format!("/Date({})/", ms(-1))),
            ("date_futuro", format!("/Date({}-0300)/", ms(1))),
            (
                "sem_dois_pontos_passado",
                "2026-11-13T10:00:00-0300".to_string(),
            ),
            (
                "sem_dois_pontos_futuro",
                "2026-11-20T10:00:00+0000".to_string(),
            ),
            ("so_hora_passado", "2026-11-13T10:00:00-03".to_string()),
            ("so_hora_futuro", "2026-11-20T10:00:00-03".to_string()),
        ] {
            t.banco
                .salvar_atividade_sdf(&atividade(id, Some(prazo.as_str())))
                .unwrap();
            t.banco
                .salvar_entrega_sdf(&entrega(id, RA1, false))
                .unwrap();
        }
        // Só as 3 "passado" contam; antes elas caíam no "formato desconhecido" e contavam as 6.
        assert_eq!(
            t.banco.resumos_da_turma("T1").unwrap()[0].atividades_cobradas,
            3
        );
    }

    #[test]
    fn alertas_aplicados_mantidos_resolvidos_e_reativados() {
        let mut t = BancoTeste::novo();
        turma_com_aulas(&mut t);
        t.banco
            .salvar_atividade_sdf(&atividade("vencida", Some("2026-11-10T23:59:00Z")))
            .unwrap();
        t.banco
            .salvar_entrega_sdf(&entrega("vencida", RA1, false))
            .unwrap();
        t.banco
            .salvar_entrega_sdf(&entrega("vencida", RA2, false))
            .unwrap();

        let motor = MotorAlertas::padrao();
        let (indicadores, r) = t.banco.avaliar_alertas_da_turma("T1", &motor).unwrap();
        // RA1: 2 de 5 = 40% e nada entregue -> CRITICO. RA2: em dia na frequência, sem entrega -> ATENCAO.
        let chaves: Vec<(&str, &str)> = indicadores
            .iter()
            .map(|i| (i.aluno_ra.as_str(), i.regra.as_str()))
            .collect();
        assert_eq!(
            chaves,
            vec![(RA1, REGRA_FALTAS_SEM_ENTREGA), (RA2, REGRA_SEM_ENTREGAS)]
        );
        assert_eq!(
            r,
            ResultadoRiscos {
                novos: 2,
                mantidos: 0,
                resolvidos: 0
            }
        );
        let ativos = t.banco.listar_riscos_ativos(Some("T1")).unwrap();
        assert_eq!(ativos.len(), 2);
        assert_eq!(ativos[0].indicador.severidade, Severidade::Critico);
        let detectado = ativos[0].detectado_em_ms;

        // Uma hora depois, RA2 entrega: o aviso dele some; o de RA1 continua com a data original.
        t.avancar(DIA / 24);
        t.banco
            .salvar_entrega_sdf(&entrega("vencida", RA2, true))
            .unwrap();
        let (_, r) = t.banco.avaliar_alertas_da_turma("T1", &motor).unwrap();
        assert_eq!(
            r,
            ResultadoRiscos {
                novos: 0,
                mantidos: 1,
                resolvidos: 1
            }
        );
        let ativos = t.banco.listar_riscos_ativos(None).unwrap();
        assert_eq!(ativos.len(), 1);
        assert_eq!(ativos[0].detectado_em_ms, detectado);
        assert!(ativos[0].atualizado_em_ms > detectado);

        // RA1 entrega também: o crítico resolve, e sobra só o aviso de frequência.
        t.banco
            .salvar_entrega_sdf(&entrega("vencida", RA1, true))
            .unwrap();
        let (indicadores, r) = t.banco.avaliar_alertas_da_turma("T1", &motor).unwrap();
        assert_eq!(indicadores[0].regra, REGRA_FREQUENCIA_BAIXA);
        assert_eq!(
            r,
            ResultadoRiscos {
                novos: 1,
                mantidos: 0,
                resolvidos: 1
            }
        );

        // Desfeita a entrega, o crítico volta como novo, com nova data de detecção.
        t.avancar(DIA / 24);
        t.banco
            .salvar_entrega_sdf(&entrega("vencida", RA1, false))
            .unwrap();
        let (_, r) = t.banco.avaliar_alertas_da_turma("T1", &motor).unwrap();
        assert_eq!(
            r,
            ResultadoRiscos {
                novos: 1,
                mantidos: 0,
                resolvidos: 1
            }
        );
        let ativos = t.banco.listar_riscos_ativos(Some("T1")).unwrap();
        assert_eq!(ativos[0].indicador.regra, REGRA_FALTAS_SEM_ENTREGA);
        assert_eq!(ativos[0].detectado_em_ms, t.agora());
    }

    #[test]
    fn indicador_de_outra_turma_ou_sem_chave_e_recusado() {
        let mut t = BancoTeste::novo();
        t.preparar_turma("T1", None, &[RA1]);
        let ind = IndicadorRisco {
            regra: REGRA_SEM_ENTREGAS.into(),
            severidade: Severidade::Atencao,
            turma_id: "T2".into(),
            aluno_ra: RA1.into(),
            mensagem: "x".into(),
            evidencias: json!({}),
        };
        assert!(matches!(
            t.banco
                .aplicar_indicadores_risco("T1", std::slice::from_ref(&ind)),
            Err(StorageError::TurmaDivergente { .. })
        ));
        let sem_ra = IndicadorRisco {
            turma_id: "T1".into(),
            aluno_ra: " ".into(),
            ..ind
        };
        assert!(t.banco.aplicar_indicadores_risco("T1", &[sem_ra]).is_err());
        assert!(t.banco.listar_riscos_ativos(None).unwrap().is_empty());
    }
}
