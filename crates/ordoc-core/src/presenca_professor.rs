//! Presença do professor (Professor Presente da SED), calculada a partir do Diário de Classe.
//!
//! Regra validada no caso de uso principal (documentação interna, seção 8): uma aula
//! fica verde quando a **frequência** e o **registro de aula** foram lançados pelo próprio professor.
//! Nos dados do caso, as 248 aulas completas estavam verdes e as 102 brancas tinham algo faltando.
//!
//! Este módulo **não** produz presença: só diz o que falta no Diário e até quando cada perfil ainda
//! pode lançar (janelas do Comunicado CITEM/DGREM/CVESC nº 5). O lançamento é sempre feito pelo
//! professor, com o conteúdo que ele deu de verdade.
//!
//! O dia vermelho ("falta dia") não é calculado aqui: ele vem da SED, que o gera por outra via.

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::modelos::validar_data_iso;

/// Quem lançou uma das metades (frequência ou registro) de uma aula no Diário.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "estado", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LancamentoDiario {
    /// Ninguém lançou.
    Ausente,
    /// O próprio professor titular lançou.
    Proprio,
    /// Outra pessoa lançou (ex.: professor eventual). Nome nunca é guardado aqui (LGPD): só o perfil,
    /// o tipo de aula e a data do lançamento, se a plataforma informar.
    Outro {
        perfil: String,
        tipo: String,
        data_lancamento: Option<String>,
    },
}

/// Uma aula da grade do professor, como o Diário a mostra.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AulaDiario {
    /// AAAA-MM-DD.
    pub data: String,
    /// HH:MM do início.
    pub inicio: String,
    pub turma_id: String,
    pub disciplina: String,
    pub frequencia: LancamentoDiario,
    pub registro: LancamentoDiario,
}

impl AulaDiario {
    pub fn validar(&self) -> Result<()> {
        validar_data_iso(&self.data)?;
        if self.turma_id.trim().is_empty() {
            return Err(CoreError::IdentificadorAusente("turma_id"));
        }
        validar_hora(&self.inicio)
    }

    pub fn estado(&self) -> EstadoAulaProfessor {
        use LancamentoDiario::{Outro, Proprio};
        match (&self.frequencia, &self.registro) {
            (Outro { .. }, _) | (_, Outro { .. }) => EstadoAulaProfessor::LancadaPorOutro,
            (Proprio, Proprio) => EstadoAulaProfessor::Completa,
            (Proprio, _) => EstadoAulaProfessor::FaltaRegistro,
            (_, Proprio) => EstadoAulaProfessor::FaltaFrequencia,
            _ => EstadoAulaProfessor::FaltaTudo,
        }
    }
}

fn validar_hora(h: &str) -> Result<()> {
    let b = h.as_bytes();
    let ok = b.len() == 5
        && b[2] == b':'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 2 || c.is_ascii_digit())
        && h[0..2].parse::<u8>().is_ok_and(|x| x < 24)
        && h[3..5].parse::<u8>().is_ok_and(|x| x < 60);
    if ok {
        Ok(())
    } else {
        Err(CoreError::ValorDesconhecido {
            campo: "inicio",
            valor: h.to_string(),
        })
    }
}

/// Estado de uma aula para o Professor Presente.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EstadoAulaProfessor {
    /// Frequência e registro lançados pelo professor: verde.
    Completa,
    /// Frequência lançada, falta o registro de aula (conteúdo).
    FaltaRegistro,
    /// Registro lançado, falta a frequência.
    FaltaFrequencia,
    /// Nada lançado.
    FaltaTudo,
    /// Outra pessoa lançou: a SED trata como aula não dada pelo titular. Só a gestão resolve.
    LancadaPorOutro,
}

impl EstadoAulaProfessor {
    pub fn verde(self) -> bool {
        self == Self::Completa
    }

    /// Falta algo que o próprio professor pode lançar (se ainda estiver na janela).
    pub fn lancavel(self) -> bool {
        matches!(
            self,
            Self::FaltaRegistro | Self::FaltaFrequencia | Self::FaltaTudo
        )
    }
}

/// Cor do dia, derivada das aulas (sem o vermelho, que vem da SED).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CorDiaProfessor {
    SemAula,
    Verde,
    Amarelo,
    Branco,
}

/// Cor do dia a partir dos estados das aulas desse dia.
pub fn cor_do_dia(estados: &[EstadoAulaProfessor]) -> CorDiaProfessor {
    let verdes = estados.iter().filter(|e| e.verde()).count();
    match (estados.len(), verdes) {
        (0, _) => CorDiaProfessor::SemAula,
        (n, v) if v == n => CorDiaProfessor::Verde,
        (_, 0) => CorDiaProfessor::Branco,
        _ => CorDiaProfessor::Amarelo,
    }
}

/// Quem ainda pode lançar ou corrigir o Diário de uma aula.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum QuemPodeLancar {
    Professor,
    GestaoEscola,
    Supervisor,
    Dirigente,
    /// Passou de todas as janelas: só com pedido formal à Diretoria de Ensino.
    ForaDasJanelas,
}

/// Janelas de edição do Diário em dias corridos (CVESC nº 5, desde 05/03/2025), **contando o dia da aula**:
/// com 5 dias, a aula de 10/11 ainda aceita lançamento do professor em 14/11 e a de 09/11 não
/// (conferido no calendário de lançamento da Sala do Futuro em 14/11/2026). Gestão, supervisor e
/// dirigente seguem a mesma contagem [Provável: não conferido na tela desses perfis].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct JanelasEdicao {
    pub professor: i64,
    pub gestao: i64,
    pub supervisor: i64,
    pub dirigente: i64,
}

impl Default for JanelasEdicao {
    fn default() -> Self {
        Self {
            professor: 5,
            gestao: 16,
            supervisor: 24,
            dirigente: 30,
        }
    }
}

impl JanelasEdicao {
    /// Quem pode lançar, dado quantos dias corridos se passaram desde a aula (0 = no próprio dia).
    pub fn quem_pode(&self, dias_desde_aula: i64) -> QuemPodeLancar {
        match dias_desde_aula {
            d if d < self.professor => QuemPodeLancar::Professor,
            d if d < self.gestao => QuemPodeLancar::GestaoEscola,
            d if d < self.supervisor => QuemPodeLancar::Supervisor,
            d if d < self.dirigente => QuemPodeLancar::Dirigente,
            _ => QuemPodeLancar::ForaDasJanelas,
        }
    }
}

/// De onde vem a chamada de uma aula que vai ser lançada (regra do professor, caso de uso principal, 14/11/2026):
/// ele faz a chamada numa aula por dia em cada turma e ela vale para as outras aulas da turma no mesmo dia.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "origem", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OrigemChamada {
    /// O professor informou a lista de faltas (por número de chamada) para esse dia e turma.
    ListaDoProfessor,
    /// Copiar a chamada já lançada pelo professor em outra aula da mesma turma no mesmo dia (início HH:MM).
    ReplicarDe { inicio: String },
    /// O professor declarou, para esse dia, que todos estavam presentes (ex.: aula online).
    DeclaracaoTodosPresentes,
    /// Não há base: o app pergunta ao professor e não lança nada até ele responder.
    PerguntarAoProfessor,
}

/// Escolhe a origem da chamada de `aula`, na ordem combinada: lista do professor > chamada do mesmo dia >
/// declaração de todos presentes > perguntar. Nunca inventa faltas nem presenças.
pub fn origem_da_chamada(
    aula: &AulaDiario,
    aulas_do_dia_da_turma: &[AulaDiario],
    tem_lista_do_professor: bool,
    declarou_todos_presentes: bool,
) -> OrigemChamada {
    if tem_lista_do_professor {
        return OrigemChamada::ListaDoProfessor;
    }
    let fonte = aulas_do_dia_da_turma
        .iter()
        .filter(|a| a.data == aula.data && a.turma_id == aula.turma_id && a.inicio != aula.inicio)
        .filter(|a| a.frequencia == LancamentoDiario::Proprio)
        .min_by(|x, y| x.inicio.cmp(&y.inicio));
    match fonte {
        Some(f) => OrigemChamada::ReplicarDe {
            inicio: f.inicio.clone(),
        },
        None if declarou_todos_presentes => OrigemChamada::DeclaracaoTodosPresentes,
        None => OrigemChamada::PerguntarAoProfessor,
    }
}

/// Uma aula que não está verde, com o que fazer e o prazo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendenciaPresenca {
    pub aula: AulaDiario,
    pub estado: EstadoAulaProfessor,
    pub quem_pode: QuemPodeLancar,
    /// Último dia (AAAA-MM-DD) em que o próprio professor ainda lança.
    pub vence_professor: String,
    /// Dias que faltam para a janela do professor fechar (negativo = já fechou).
    pub dias_restantes_professor: i64,
}

/// Lista as aulas não verdes até `hoje` (inclusive), da mais urgente para a menos urgente:
/// primeiro as que o professor ainda lança (menos dias restantes primeiro), depois as outras.
/// Aulas futuras são ignoradas (não se lança antes do início da aula).
pub fn pendencias(
    aulas: &[AulaDiario],
    hoje: &str,
    janelas: &JanelasEdicao,
) -> Result<Vec<PendenciaPresenca>> {
    let hoje_d = dia_juliano(hoje)?;
    let mut saida = Vec::new();
    for aula in aulas {
        aula.validar()?;
        let estado = aula.estado();
        let d = dia_juliano(&aula.data)?;
        if estado.verde() || d > hoje_d {
            continue;
        }
        let desde = hoje_d - d;
        saida.push(PendenciaPresenca {
            vence_professor: data_de_dia_juliano(d + janelas.professor - 1),
            dias_restantes_professor: janelas.professor - 1 - desde,
            quem_pode: if estado.lancavel() {
                janelas.quem_pode(desde)
            } else {
                QuemPodeLancar::GestaoEscola
            },
            estado,
            aula: aula.clone(),
        });
    }
    saida.sort_by(|a, b| {
        let chave = |p: &PendenciaPresenca| {
            (
                !(p.estado.lancavel() && p.quem_pode == QuemPodeLancar::Professor),
                p.dias_restantes_professor,
                p.aula.data.clone(),
                p.aula.inicio.clone(),
            )
        };
        chave(a).cmp(&chave(b))
    });
    Ok(saida)
}

/// Dias desde 1970-01-01 (algoritmo civil de H. Hinnant). A data é validada antes.
pub(crate) fn dia_juliano(data: &str) -> Result<i64> {
    validar_data_iso(data)?;
    let invalida = || CoreError::DataInvalida(data.to_string());
    let a: i64 = data[0..4].parse().map_err(|_| invalida())?;
    let m: i64 = data[5..7].parse().map_err(|_| invalida())?;
    let d: i64 = data[8..10].parse().map_err(|_| invalida())?;
    let a = if m <= 2 { a - 1 } else { a };
    let era = a.div_euclid(400);
    let ano_da_era = a - era * 400;
    let mp = (m + 9) % 12;
    let dia_do_ano = (153 * mp + 2) / 5 + d - 1;
    let dia_da_era = ano_da_era * 365 + ano_da_era / 4 - ano_da_era / 100 + dia_do_ano;
    Ok(era * 146_097 + dia_da_era - 719_468)
}

pub(crate) fn data_de_dia_juliano(z: i64) -> String {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let dia_da_era = z - era * 146_097;
    let ano_da_era =
        (dia_da_era - dia_da_era / 1460 + dia_da_era / 36_524 - dia_da_era / 146_096) / 365;
    let dia_do_ano = dia_da_era - (365 * ano_da_era + ano_da_era / 4 - ano_da_era / 100);
    let mp = (5 * dia_do_ano + 2) / 153;
    let d = dia_do_ano - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let a = ano_da_era + era * 400 + i64::from(m <= 2);
    format!("{a:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod testes {
    use super::*;

    // Fixtures sintéticas (turmas e disciplinas fictícias).
    fn aula(data: &str, inicio: &str, f: LancamentoDiario, r: LancamentoDiario) -> AulaDiario {
        AulaDiario {
            data: data.into(),
            inicio: inicio.into(),
            turma_id: "T1".into(),
            disciplina: "Disciplina X".into(),
            frequencia: f,
            registro: r,
        }
    }
    use LancamentoDiario::{Ausente, Proprio};
    fn eventual() -> LancamentoDiario {
        LancamentoDiario::Outro {
            perfil: "Professor".into(),
            tipo: "Aula Eventual".into(),
            data_lancamento: Some("2026-09-18".into()),
        }
    }

    #[test]
    fn verde_so_com_frequencia_e_registro_proprios() {
        assert_eq!(
            aula("2026-11-09", "07:00", Proprio, Proprio).estado(),
            EstadoAulaProfessor::Completa
        );
        assert_eq!(
            aula("2026-11-09", "07:00", Proprio, Ausente).estado(),
            EstadoAulaProfessor::FaltaRegistro
        );
        assert_eq!(
            aula("2026-11-09", "07:00", Ausente, Proprio).estado(),
            EstadoAulaProfessor::FaltaFrequencia
        );
        assert_eq!(
            aula("2026-11-09", "07:00", Ausente, Ausente).estado(),
            EstadoAulaProfessor::FaltaTudo
        );
        assert_eq!(
            aula("2026-09-17", "07:00", eventual(), eventual()).estado(),
            EstadoAulaProfessor::LancadaPorOutro
        );
        assert_eq!(
            aula("2026-09-17", "07:00", Proprio, eventual()).estado(),
            EstadoAulaProfessor::LancadaPorOutro
        );
    }

    #[test]
    fn cor_do_dia_segue_a_legenda() {
        use EstadoAulaProfessor::*;
        assert_eq!(cor_do_dia(&[]), CorDiaProfessor::SemAula);
        assert_eq!(cor_do_dia(&[Completa, Completa]), CorDiaProfessor::Verde);
        assert_eq!(
            cor_do_dia(&[Completa, FaltaRegistro]),
            CorDiaProfessor::Amarelo
        );
        assert_eq!(
            cor_do_dia(&[FaltaTudo, LancadaPorOutro]),
            CorDiaProfessor::Branco
        );
    }

    #[test]
    fn janelas_de_edicao() {
        let j = JanelasEdicao::default();
        assert_eq!(j.quem_pode(0), QuemPodeLancar::Professor);
        assert_eq!(j.quem_pode(4), QuemPodeLancar::Professor); // 10/11 lançável em 14/11
        assert_eq!(j.quem_pode(5), QuemPodeLancar::GestaoEscola); // 09/11 fechou em 14/11
        assert_eq!(j.quem_pode(15), QuemPodeLancar::GestaoEscola);
        assert_eq!(j.quem_pode(16), QuemPodeLancar::Supervisor);
        assert_eq!(j.quem_pode(23), QuemPodeLancar::Supervisor);
        assert_eq!(j.quem_pode(29), QuemPodeLancar::Dirigente);
        assert_eq!(j.quem_pode(30), QuemPodeLancar::ForaDasJanelas);
    }

    #[test]
    fn calendario_ida_e_volta() {
        for d in [
            "1970-01-01",
            "2024-02-29",
            "2026-09-17",
            "2026-12-31",
            "2000-03-01",
        ] {
            assert_eq!(data_de_dia_juliano(dia_juliano(d).unwrap()), d);
        }
        assert_eq!(dia_juliano("1970-01-01").unwrap(), 0);
        assert_eq!(
            dia_juliano("2026-11-14").unwrap() - dia_juliano("2026-09-29").unwrap(),
            46
        );
        assert_eq!(
            data_de_dia_juliano(dia_juliano("2026-02-26").unwrap() + 5),
            "2026-03-03"
        );
    }

    #[test]
    fn pendencias_ordenadas_por_urgencia() {
        let aulas = vec![
            aula("2026-10-20", "07:00", Ausente, Ausente), // 25 dias: só o dirigente
            aula("2026-11-12", "07:00", Proprio, Ausente), // 2 dias restantes (vence 16/11)
            aula("2026-11-10", "08:00", Ausente, Ausente), // vence hoje (5º dia contando a aula)
            aula("2026-11-11", "07:00", Proprio, Proprio), // verde: some
            aula("2026-11-20", "07:00", Ausente, Ausente), // futura: some
            aula("2026-09-17", "07:00", eventual(), eventual()),
        ];
        let p = pendencias(&aulas, "2026-11-14", &JanelasEdicao::default()).unwrap();
        let datas: Vec<_> = p.iter().map(|x| x.aula.data.as_str()).collect();
        assert_eq!(
            datas,
            ["2026-11-10", "2026-11-12", "2026-09-17", "2026-10-20"]
        );
        assert_eq!(p[0].dias_restantes_professor, 0);
        assert_eq!(p[0].vence_professor, "2026-11-14");
        assert_eq!(p[1].estado, EstadoAulaProfessor::FaltaRegistro);
        assert_eq!(p[1].vence_professor, "2026-11-16");
        assert_eq!(p[2].quem_pode, QuemPodeLancar::GestaoEscola);
        assert_eq!(p[3].quem_pode, QuemPodeLancar::Dirigente);
    }

    #[test]
    fn rejeita_entrada_invalida() {
        let mut a = aula("2026-11-09", "7:00", Ausente, Ausente);
        assert!(a.validar().is_err());
        a.inicio = "24:00".into();
        assert!(a.validar().is_err());
        a.inicio = "07:00".into();
        a.turma_id = " ".into();
        assert!(a.validar().is_err());
        assert!(pendencias(&[], "2026-13-01", &JanelasEdicao::default()).is_err());
    }

    #[test]
    fn serializa_sem_nome_de_quem_lancou() {
        let j = serde_json::to_string(&eventual()).unwrap();
        assert!(j.contains("\"estado\":\"OUTRO\""));
        assert!(!j.contains("nome"));
    }

    #[test]
    fn origem_da_chamada_segue_a_ordem_combinada() {
        let alvo = aula("2026-11-11", "11:30", Ausente, Ausente);
        let dia = vec![
            aula("2026-11-11", "07:50", Ausente, Ausente),
            aula("2026-11-11", "10:40", Proprio, Proprio),
            alvo.clone(),
        ];
        assert_eq!(
            origem_da_chamada(&alvo, &dia, true, false),
            OrigemChamada::ListaDoProfessor
        );
        assert_eq!(
            origem_da_chamada(&alvo, &dia, false, true),
            OrigemChamada::ReplicarDe {
                inicio: "10:40".into()
            }
        );
        let sem_fonte = vec![aula("2026-11-11", "07:50", Ausente, Ausente), alvo.clone()];
        assert_eq!(
            origem_da_chamada(&alvo, &sem_fonte, false, true),
            OrigemChamada::DeclaracaoTodosPresentes
        );
        assert_eq!(
            origem_da_chamada(&alvo, &sem_fonte, false, false),
            OrigemChamada::PerguntarAoProfessor
        );
        // Chamada de outra turma ou de outro dia não serve de fonte.
        let mut outra = aula("2026-11-11", "08:40", Proprio, Proprio);
        outra.turma_id = "T2".into();
        let outro_dia = aula("2026-11-10", "08:40", Proprio, Proprio);
        assert_eq!(
            origem_da_chamada(&alvo, &[outra, outro_dia], false, false),
            OrigemChamada::PerguntarAoProfessor
        );
    }
}
