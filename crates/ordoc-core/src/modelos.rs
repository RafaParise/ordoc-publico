//! Modelos canônicos unificados: a forma única que o Ordoc usa para Diário de Classe (DC),
//! Sala do Futuro (SDF) e Painéis de BI, independente do formato de cada portal.
//!
//! Regra 4 do regras do projeto: todo modelo vindo de fora guarda o JSON bruto integral em `metadata_bruta`,
//! ao lado dos campos normalizados. Nada que a plataforma devolve é descartado.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{CoreError, Result};

/// Marcador que o parser do DC (`ordoc-extract`) usa quando o payload não traz o RA.
/// Nunca vira chave: dois alunos sem RA se fundiriam num só registro.
pub const RA_AUSENTE: &str = "SEM_RA";

fn exigir(valor: &str, campo: &'static str) -> Result<()> {
    if valor.trim().is_empty() {
        Err(CoreError::IdentificadorAusente(campo))
    } else {
        Ok(())
    }
}

fn exigir_ra(ra: &str) -> Result<()> {
    let ra = ra.trim();
    if ra.is_empty() || ra == RA_AUSENTE {
        Err(CoreError::RaAusente)
    } else {
        Ok(())
    }
}

fn bissexto(ano: u32) -> bool {
    (ano % 4 == 0 && ano % 100 != 0) || ano % 400 == 0
}

/// Confere a data civil `AAAA-MM-DD` (formato que o SQLite ordena corretamente como texto),
/// inclusive o número de dias do mês e o 29 de fevereiro.
pub fn validar_data_iso(data: &str) -> Result<()> {
    let invalida = || CoreError::DataInvalida(data.to_string());
    let b = data.as_bytes();
    let formato_ok = b.len() == 10
        && b.iter().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                *c == b'-'
            } else {
                c.is_ascii_digit()
            }
        });
    if !formato_ok {
        return Err(invalida());
    }

    // Só dígitos ASCII nas posições abaixo: o fatiamento e o parse não falham.
    let ano: u32 = data[0..4].parse().map_err(|_| invalida())?;
    let mes: u32 = data[5..7].parse().map_err(|_| invalida())?;
    let dia: u32 = data[8..10].parse().map_err(|_| invalida())?;
    let dias_no_mes = match mes {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if bissexto(ano) => 29,
        2 => 28,
        _ => return Err(invalida()),
    };
    if dia == 0 || dia > dias_no_mes {
        return Err(invalida());
    }
    Ok(())
}

/// Turma do professor. A chave canônica é o código da turma no Diário de Classe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Turma {
    /// Código da turma no Diário de Classe (SED).
    pub id: String,
    pub nome: String,
    pub ano_letivo: u16,
    pub escola: Option<String>,
    /// Código da mesma turma na Sala do Futuro (`classCode`), quando já foi associado.
    /// É o que liga as atividades da SDF a esta turma.
    pub codigo_sdf: Option<String>,
    pub metadata_bruta: Value,
}

impl Turma {
    pub fn validar(&self) -> Result<()> {
        exigir(&self.id, "turma.id")
    }
}

/// Aluno. A chave canônica é o RA, que cruza o DC com a Sala do Futuro e com o BI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Aluno {
    pub ra: String,
    pub digito_ra: Option<String>,
    pub uf_ra: Option<String>,
    pub nome: String,
    pub situacao_matricula: String,
    pub metadata_bruta: Value,
}

impl Aluno {
    pub fn validar(&self) -> Result<()> {
        exigir_ra(&self.ra)
    }

    /// RA no formato de exibição da SED: `000123456-7/SP` (só as partes que existirem).
    pub fn ra_completo(&self) -> String {
        let mut s = self.ra.clone();
        if let Some(d) = self.digito_ra.as_deref().filter(|d| !d.is_empty()) {
            s.push('-');
            s.push_str(d);
        }
        if let Some(uf) = self.uf_ra.as_deref().filter(|u| !u.is_empty()) {
            s.push('/');
            s.push_str(uf);
        }
        s
    }
}

/// Vínculo do aluno com uma turma (o mesmo RA pode estar em mais de uma turma do professor).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Matricula {
    pub turma_id: String,
    pub aluno_ra: String,
    pub numero_chamada: Option<u32>,
    pub situacao: String,
    pub metadata_bruta: Value,
}

impl Matricula {
    pub fn validar(&self) -> Result<()> {
        exigir(&self.turma_id, "matricula.turma_id")?;
        exigir_ra(&self.aluno_ra)
    }
}

/// Aula dada numa data. `quantidade` é o número de aulas do dia (aula dupla = 2): é o peso das faltas.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Aula {
    pub id: String,
    pub turma_id: String,
    /// Data civil `AAAA-MM-DD`.
    pub data: String,
    pub disciplina: Option<String>,
    pub quantidade: u32,
    pub metadata_bruta: Value,
}

impl Aula {
    pub fn validar(&self) -> Result<()> {
        exigir(&self.id, "aula.id")?;
        exigir(&self.turma_id, "aula.turma_id")?;
        if self.quantidade == 0 {
            return Err(CoreError::QuantidadeInvalida);
        }
        validar_data_iso(&self.data)
    }
}

/// Situação do aluno numa aula.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StatusPresenca {
    Presente,
    Falta,
    /// Falta abonada/justificada: fica registrada e, por padrão, não conta no percentual de faltas
    /// (ver `LimitesAlerta::contar_justificadas`).
    FaltaJustificada,
}

impl StatusPresenca {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Presente => "PRESENTE",
            Self::Falta => "FALTA",
            Self::FaltaJustificada => "FALTA_JUSTIFICADA",
        }
    }

    pub fn parse(valor: &str) -> Result<Self> {
        match valor {
            "PRESENTE" => Ok(Self::Presente),
            "FALTA" => Ok(Self::Falta),
            "FALTA_JUSTIFICADA" => Ok(Self::FaltaJustificada),
            outro => Err(CoreError::ValorDesconhecido {
                campo: "status_presenca",
                valor: outro.to_string(),
            }),
        }
    }
}

/// Uma linha da chamada: o aluno `aluno_ra` na aula `aula_id`.
/// Vindo do DC, `metadata_bruta` guarda o lançamento inteiro (ex.: o id interno do lançamento, que o
/// conector usa na idempotência); lançada no Ordoc, começa como `{}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegistroFrequencia {
    pub aula_id: String,
    pub aluno_ra: String,
    pub status: StatusPresenca,
    pub metadata_bruta: Value,
}

impl RegistroFrequencia {
    pub fn validar(&self) -> Result<()> {
        exigir(&self.aula_id, "frequencia.aula_id")?;
        exigir_ra(&self.aluno_ra)
    }
}

/// Atividade publicada na Sala do Futuro (visão agregada da turma).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AtividadeSdf {
    pub id: String,
    /// Código da turma como a Sala do Futuro o chama (`classCode`); liga com `Turma::codigo_sdf`.
    pub turma_codigo_sdf: String,
    pub titulo: String,
    pub prazo_iso: Option<String>,
    pub total_alunos: u32,
    pub total_entregues: u32,
    pub metadata_bruta: Value,
}

impl AtividadeSdf {
    pub fn validar(&self) -> Result<()> {
        exigir(&self.id, "atividade_sdf.id")?;
        exigir(&self.turma_codigo_sdf, "atividade_sdf.turma_codigo_sdf")
    }
}

/// Situação de um aluno numa atividade da Sala do Futuro.
/// Sem esta visão por aluno, o Ordoc não afirma que alguém deixou de entregar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntregaSdf {
    pub atividade_id: String,
    pub aluno_ra: String,
    pub entregue: bool,
    pub entregue_em: Option<String>,
    pub metadata_bruta: Value,
}

impl EntregaSdf {
    pub fn validar(&self) -> Result<()> {
        exigir(&self.atividade_id, "entrega_sdf.atividade_id")?;
        exigir_ra(&self.aluno_ra)
    }
}

/// Linha de um painel de BI. A linha inteira (todas as colunas) fica em `metadata_bruta`;
/// `turma_id` e `aluno_ra` são só os vínculos já identificados.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndicadorBi {
    pub painel: String,
    pub turma_id: Option<String>,
    pub aluno_ra: Option<String>,
    pub coletado_em_ms: i64,
    pub metadata_bruta: Value,
}

impl IndicadorBi {
    pub fn validar(&self) -> Result<()> {
        exigir(&self.painel, "indicador_bi.painel")
    }

    /// Valor de uma coluna da linha do painel.
    pub fn valor(&self, coluna: &str) -> Option<&Value> {
        self.metadata_bruta.get(coluna)
    }
}

/// Gravidade de um indicador de risco (`Critico` > `Atencao` na ordenação).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Severidade {
    Atencao,
    Critico,
}

impl Severidade {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Atencao => "ATENCAO",
            Self::Critico => "CRITICO",
        }
    }

    pub fn parse(valor: &str) -> Result<Self> {
        match valor {
            "ATENCAO" => Ok(Self::Atencao),
            "CRITICO" => Ok(Self::Critico),
            outro => Err(CoreError::ValorDesconhecido {
                campo: "severidade",
                valor: outro.to_string(),
            }),
        }
    }
}

/// Resultado do motor de alertas para um aluno numa turma.
/// Calculado no aparelho (não vem de plataforma), por isso guarda `evidencias` em vez de `metadata_bruta`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndicadorRisco {
    /// Identificador estável da regra que disparou (ex.: `faltas_sem_entrega`).
    pub regra: String,
    pub severidade: Severidade,
    pub turma_id: String,
    /// O RA é dado pessoal de aluno menor: a interface junta com o nome da chamada, e o indicador
    /// não deve aparecer em notificação de tela bloqueada.
    pub aluno_ra: String,
    /// Texto para o professor, sem nome nem RA (o aluno já está em `aluno_ra`).
    pub mensagem: String,
    /// Números usados na decisão, para o professor conferir.
    pub evidencias: Value,
}

/// Estado de um item da fila Outbox (offline-first). O texto é o gravado no banco.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StatusSincronizacao {
    /// Gravado localmente, esperando envio.
    PendingSync,
    /// Reservado por um worker e sendo enviado.
    InFlight,
    /// Confirmado pela plataforma.
    Synced,
    /// Esgotou as tentativas ou foi recusado; precisa de ação do professor.
    Failed,
}

impl StatusSincronizacao {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PendingSync => "PENDING_SYNC",
            Self::InFlight => "IN_FLIGHT",
            Self::Synced => "SYNCED",
            Self::Failed => "FAILED",
        }
    }

    pub fn parse(valor: &str) -> Result<Self> {
        match valor {
            "PENDING_SYNC" => Ok(Self::PendingSync),
            "IN_FLIGHT" => Ok(Self::InFlight),
            "SYNCED" => Ok(Self::Synced),
            "FAILED" => Ok(Self::Failed),
            outro => Err(CoreError::ValorDesconhecido {
                campo: "status_sincronizacao",
                valor: outro.to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use serde_json::json;

    #[test]
    fn datas_validas_e_invalidas() {
        assert!(validar_data_iso("2026-11-14").is_ok());
        assert!(validar_data_iso("2028-02-29").is_ok()); // bissexto
        assert!(validar_data_iso("2026-02-29").is_err());
        assert!(validar_data_iso("2026-04-31").is_err());
        assert!(validar_data_iso("2026-13-01").is_err());
        assert!(validar_data_iso("14/11/2026").is_err());
        assert!(validar_data_iso("2026-1-14").is_err());
        assert!(validar_data_iso("2026-11-00").is_err());
        assert!(validar_data_iso("2026/11/14").is_err());
        assert!(validar_data_iso("２０２６-11-14").is_err()); // dígitos não ASCII
    }

    #[test]
    fn aula_exige_ids_quantidade_e_data() {
        let mut aula = Aula {
            id: "A1".into(),
            turma_id: "T1".into(),
            data: "2026-11-14".into(),
            disciplina: None,
            quantidade: 2,
            metadata_bruta: json!({}),
        };
        assert!(aula.validar().is_ok());
        aula.quantidade = 0;
        assert_eq!(aula.validar(), Err(CoreError::QuantidadeInvalida));
        aula.quantidade = 1;
        aula.turma_id = " ".into();
        assert_eq!(
            aula.validar(),
            Err(CoreError::IdentificadorAusente("aula.turma_id"))
        );
    }

    #[test]
    fn ra_ausente_nunca_vira_chave() {
        let mut aluno = Aluno {
            ra: RA_AUSENTE.into(),
            digito_ra: None,
            uf_ra: None,
            nome: "Aluno Ficticio".into(),
            situacao_matricula: "ATIVO".into(),
            metadata_bruta: json!({}),
        };
        assert_eq!(aluno.validar(), Err(CoreError::RaAusente));
        aluno.ra = "  ".into();
        assert_eq!(aluno.validar(), Err(CoreError::RaAusente));
        aluno.ra = "000111222".into();
        assert!(aluno.validar().is_ok());

        let registro = RegistroFrequencia {
            aula_id: "A1".into(),
            aluno_ra: RA_AUSENTE.into(),
            status: StatusPresenca::Falta,
            metadata_bruta: json!({}),
        };
        assert_eq!(registro.validar(), Err(CoreError::RaAusente));

        let matricula = Matricula {
            turma_id: "T1".into(),
            aluno_ra: RA_AUSENTE.into(),
            numero_chamada: Some(7),
            situacao: "ATIVO".into(),
            metadata_bruta: json!({}),
        };
        assert_eq!(matricula.validar(), Err(CoreError::RaAusente));
    }

    #[test]
    fn frequencia_e_matricula_guardam_o_json_bruto() {
        // Campos que o Ordoc não conhece precisam sobreviver à ida e volta (regra 4).
        let frequencia = json!({
            "aula_id": "A1",
            "aluno_ra": "000111222",
            "status": "FALTA_JUSTIFICADA",
            "metadata_bruta": {"idLancamento": 991, "tipoFalta": "X", "observacao": "atestado"}
        });
        let registro: RegistroFrequencia = serde_json::from_value(frequencia.clone()).unwrap();
        assert_eq!(registro.status, StatusPresenca::FaltaJustificada);
        assert_eq!(registro.metadata_bruta["idLancamento"], 991);
        assert_eq!(serde_json::to_value(&registro).unwrap(), frequencia);

        let vinculo = json!({
            "turma_id": "T1",
            "aluno_ra": "000111222",
            "numero_chamada": 7,
            "situacao": "ATIVO",
            "metadata_bruta": {"dataMatricula": "2026-02-03", "codigoMatricula": 55}
        });
        let matricula: Matricula = serde_json::from_value(vinculo.clone()).unwrap();
        assert_eq!(serde_json::to_value(&matricula).unwrap(), vinculo);
    }

    #[test]
    fn enums_voltam_do_texto_do_banco() {
        for s in [
            StatusPresenca::Presente,
            StatusPresenca::Falta,
            StatusPresenca::FaltaJustificada,
        ] {
            assert_eq!(StatusPresenca::parse(s.as_str()), Ok(s));
            assert_eq!(serde_json::to_value(s).unwrap(), json!(s.as_str()));
        }
        for s in [
            StatusSincronizacao::PendingSync,
            StatusSincronizacao::InFlight,
            StatusSincronizacao::Synced,
            StatusSincronizacao::Failed,
        ] {
            assert_eq!(StatusSincronizacao::parse(s.as_str()), Ok(s));
            assert_eq!(serde_json::to_value(s).unwrap(), json!(s.as_str()));
        }
        for s in [Severidade::Atencao, Severidade::Critico] {
            assert_eq!(Severidade::parse(s.as_str()), Ok(s));
            assert_eq!(serde_json::to_value(s).unwrap(), json!(s.as_str()));
        }
        assert!(Severidade::Critico > Severidade::Atencao);
        assert!(Severidade::parse("GRAVE").is_err());
    }

    #[test]
    fn ra_completo_monta_so_o_que_existe() {
        let mut aluno = Aluno {
            ra: "000111222".into(),
            digito_ra: Some("3".into()),
            uf_ra: Some("SP".into()),
            nome: "Aluno Ficticio".into(),
            situacao_matricula: "ATIVO".into(),
            metadata_bruta: json!({}),
        };
        assert_eq!(aluno.ra_completo(), "000111222-3/SP");
        aluno.digito_ra = Some(String::new());
        aluno.uf_ra = None;
        assert_eq!(aluno.ra_completo(), "000111222");
    }

    #[test]
    fn indicador_bi_le_qualquer_coluna() {
        let ind = IndicadorBi {
            painel: "frequencia_mensal".into(),
            turma_id: Some("T1".into()),
            aluno_ra: None,
            coletado_em_ms: 1,
            metadata_bruta: json!({"turma": "3B", "frequencia_media": 92.5, "coluna_nova": [1, 2]}),
        };
        assert!(ind.validar().is_ok());
        assert_eq!(ind.valor("frequencia_media"), Some(&json!(92.5)));
        assert_eq!(ind.valor("coluna_nova"), Some(&json!([1, 2])));
        assert_eq!(ind.valor("inexistente"), None);
    }
}
