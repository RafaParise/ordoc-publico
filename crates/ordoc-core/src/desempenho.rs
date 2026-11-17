//! Desempenho do aluno: síntese bimestral, discrepâncias, frequência, risco de reprovação e o
//! acompanhamento do pedido de alteração de notas.
//!
//! Fontes das regras (marcação do regras do projeto global):
//! - Resolução SE 62/2019, art. 1º [Certo]: nota bimestral e final em inteiros de 0 a 10. Parágrafo
//!   único: a síntese decorre de **diferentes** instrumentos, em avaliação contínua, prevalecendo os
//!   aspectos qualitativos e os resultados ao longo do período sobre os de provas finais.
//! - Res. SE 62/2019, art. 2º [Certo]: nota >= 5 é desempenho satisfatório.
//! - Res. SE 62/2019, art. 5º [Certo]: no Ensino Médio a frequência é apurada pelo número de aulas de
//!   cada componente curricular.
//! - Res. SE 62/2019, art. 7º, §§ 1º e 2º [Certo]: o Conselho de Classe emite o parecer final; se
//!   aprovar por decisão dele, a nota final é atribuída pelo colegiado dentro do satisfatório (>= 5).
//! - LDB (Lei 9.394/1996), art. 24, VI [Certo]: frequência mínima de 75% do total de horas letivas.
//! - Prova Paulista [Provável, fonte: imprensa]: avaliação diagnóstica/formativa; a escola PODE usar
//!   até 30% do resultado na nota bimestral. Por isso o teto é configurável (padrão 30%).
//! - Compensação de ausências: a regra estadual vigente NÃO foi verificada. Este módulo só calcula
//!   quantas aulas faltam para voltar ao mínimo; o procedimento oficial deve ser confirmado com a escola.
//!
//! Domínio puro: só calcula e descreve. Nada aqui lança nota ou falta em sistema externo, e nenhuma
//! sugestão é "alterar nota sem avaliação": a saída para nota baixa é sempre nova avaliação
//! (recuperação) ou o Conselho de Classe. Fixtures e textos dos testes são sintéticos.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::conector::sanitizar_detalhe;
use crate::error::{CoreError, Result};
use crate::modelos::validar_data_iso;
use crate::presenca_professor::dia_juliano;

/// Maior nota da escala (Res. SE 62/2019, art. 1º).
pub const NOTA_MAXIMA: u8 = 10;
/// Nota mínima do desempenho satisfatório (Res. SE 62/2019, art. 2º).
pub const NOTA_SATISFATORIA: u8 = 5;

/// Folga para comparações em ponto flutuante (4,5 calculado como 4,4999999 ainda vira 5).
const EPSILON: f64 = 1e-9;

fn fora(campo: &'static str, valor: impl ToString, esperado: &'static str) -> CoreError {
    CoreError::ForaDoIntervalo {
        campo,
        valor: valor.to_string(),
        esperado,
    }
}

/// Número como o professor lê: inteiro sem casas (`7`), senão uma casa com vírgula (`6,2`).
fn numero_br(valor: f64) -> String {
    let arredondado = (valor * 10.0).round() / 10.0;
    if (arredondado - arredondado.round()).abs() < EPSILON {
        format!("{arredondado:.0}")
    } else {
        format!("{arredondado:.1}").replace('.', ",")
    }
}

/// `AAAA-MM-DD` -> `DD/MM/AAAA` (devolve o texto como veio se não estiver no formato).
fn data_br(iso: &str) -> String {
    if iso.len() == 10 && iso.is_char_boundary(4) && iso.is_char_boundary(7) {
        format!("{}/{}/{}", &iso[8..10], &iso[5..7], &iso[0..4])
    } else {
        iso.to_string()
    }
}

/// Uma casa decimal (0,3 * 100 = 30,000000000000004 vira 30).
fn uma_casa(valor: f64) -> f64 {
    (valor * 10.0).round() / 10.0
}

/// "no bimestre restante" / "em cada um dos 3 bimestres restantes".
fn nos_restantes(r: u32) -> String {
    if r == 1 {
        "no bimestre restante".to_string()
    } else {
        format!("em cada um dos {r} bimestres restantes")
    }
}

fn plural(n: impl Into<u64>, singular: &str, plural: &str) -> String {
    let n = n.into();
    format!("{n} {}", if n == 1 { singular } else { plural })
}

// ---------------------------------------------------------------------------------------------
// Síntese bimestral
// ---------------------------------------------------------------------------------------------

/// Tipo de instrumento de avaliação.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TipoInstrumento {
    /// Avaliação estadual; peso limitado ao teto configurado.
    ProvaPaulista,
    Atividade,
    /// Plataforma digital (ex.: Tarefa SP, Educação Profissional).
    Plataforma,
    Caderno,
    Participacao,
    /// Não entra na média: só substitui (ou faz média com) a síntese quando é maior.
    Recuperacao,
    Outro(String),
}

/// Um instrumento de avaliação do bimestre.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstrumentoAvaliacao {
    pub nome: String,
    pub tipo: TipoInstrumento,
    /// 0 a 10, com casas decimais. `None`: ainda sem nota (fica fora do cálculo).
    pub nota: Option<f64>,
    /// Peso relativo (> 0). Os pesos são normalizados entre os instrumentos com nota.
    pub peso: f64,
    /// Dado bruto de origem (ex.: linha da plataforma), guardado inteiro. `null` quando digitado.
    #[serde(default)]
    pub metadata_bruta: Value,
}

impl InstrumentoAvaliacao {
    pub fn novo(
        nome: impl Into<String>,
        tipo: TipoInstrumento,
        nota: Option<f64>,
        peso: f64,
    ) -> Self {
        Self {
            nome: nome.into(),
            tipo,
            nota,
            peso,
            metadata_bruta: Value::Null,
        }
    }

    pub fn validar(&self) -> Result<()> {
        if self.nome.trim().is_empty() {
            return Err(CoreError::IdentificadorAusente("instrumento.nome"));
        }
        if let TipoInstrumento::Outro(tipo) = &self.tipo {
            if tipo.trim().is_empty() {
                return Err(CoreError::IdentificadorAusente("instrumento.tipo"));
            }
        }
        if let Some(nota) = self.nota {
            if !(nota.is_finite() && (0.0..=f64::from(NOTA_MAXIMA)).contains(&nota)) {
                return Err(fora("instrumento.nota", nota, "0 a 10"));
            }
        }
        if !(self.peso.is_finite() && self.peso > 0.0) {
            return Err(fora("instrumento.peso", self.peso, "maior que 0"));
        }
        Ok(())
    }

    fn eh_prova_paulista(&self) -> bool {
        self.tipo == TipoInstrumento::ProvaPaulista
    }

    fn eh_recuperacao(&self) -> bool {
        self.tipo == TipoInstrumento::Recuperacao
    }
}

/// Como a nota de recuperação maior que a síntese entra nela. **Decisão da escola** (proposta
/// pedagógica / Conselho de Classe): a Res. SE 62/2019 não fixa a fórmula.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PoliticaRecuperacao {
    /// A síntese passa a ser a nota da recuperação.
    #[default]
    SubstituiSeMaior,
    /// A síntese passa a ser a média entre a síntese original e a recuperação.
    MediaComOriginal,
}

/// Como a média vira o inteiro de 0 a 10 exigido pelo art. 1º da Res. SE 62/2019.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Arredondamento {
    /// Inteiro mais próximo, ,5 para cima (4,5 -> 5; 4,49 -> 4).
    #[default]
    MaisProximo,
    ParaCima,
    ParaBaixo,
}

impl Arredondamento {
    pub fn aplicar(self, valor: f64) -> u8 {
        let inteiro = match self {
            Self::MaisProximo => (valor + 0.5 + EPSILON).floor(),
            Self::ParaCima => (valor - EPSILON).ceil(),
            Self::ParaBaixo => (valor + EPSILON).floor(),
        };
        inteiro.clamp(0.0, f64::from(NOTA_MAXIMA)) as u8
    }
}

/// Regras da síntese e do risco final. Todas configuráveis; os padrões seguem as fontes do módulo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegrasAvaliacao {
    /// Fração máxima da Prova Paulista na síntese (0,30 = 30%) [Provável, fonte: imprensa].
    pub teto_prova_paulista: f64,
    pub politica_recuperacao: PoliticaRecuperacao,
    pub arredondamento: Arredondamento,
    /// Diferença (em pontos) entre um instrumento e a média dos outros que gera alerta.
    pub limiar_discrepancia: f64,
    /// Mínimo de instrumentos com nota (Res. SE 62/2019, art. 1º, parágrafo único: "diferentes").
    pub minimo_instrumentos: usize,
    /// Fração do peso efetivo acima da qual um instrumento "domina" a síntese.
    pub fracao_maxima_instrumento: f64,
    /// Res. SE 62/2019, art. 2º.
    pub nota_satisfatoria: u8,
    /// Bimestres do ano letivo.
    pub bimestres: u8,
}

impl Default for RegrasAvaliacao {
    fn default() -> Self {
        Self {
            teto_prova_paulista: 0.30,
            politica_recuperacao: PoliticaRecuperacao::default(),
            arredondamento: Arredondamento::default(),
            limiar_discrepancia: 4.0,
            minimo_instrumentos: 3,
            fracao_maxima_instrumento: 0.5,
            nota_satisfatoria: NOTA_SATISFATORIA,
            bimestres: 4,
        }
    }
}

impl RegrasAvaliacao {
    pub fn validar(&self) -> Result<()> {
        let t = self.teto_prova_paulista;
        if !(t.is_finite() && (0.0..=1.0).contains(&t)) {
            return Err(fora("teto_prova_paulista", t, "0 a 1"));
        }
        let l = self.limiar_discrepancia;
        if !(l.is_finite() && l > 0.0) {
            return Err(fora("limiar_discrepancia", l, "maior que 0"));
        }
        let f = self.fracao_maxima_instrumento;
        if !(f.is_finite() && f > 0.0 && f <= 1.0) {
            return Err(fora("fracao_maxima_instrumento", f, "maior que 0 e até 1"));
        }
        if self.nota_satisfatoria > NOTA_MAXIMA {
            return Err(fora("nota_satisfatoria", self.nota_satisfatoria, "0 a 10"));
        }
        if self.bimestres == 0 {
            return Err(fora("bimestres", self.bimestres, "pelo menos 1"));
        }
        Ok(())
    }
}

/// Como cada instrumento entrou na síntese.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DetalheInstrumento {
    pub nome: String,
    pub tipo: TipoInstrumento,
    pub nota: Option<f64>,
    pub peso_informado: f64,
    /// Fração da síntese (0 a 1) depois do teto da Prova Paulista. 0 para recuperação e sem nota.
    pub peso_efetivo: f64,
    /// `peso_efetivo * nota`.
    pub contribuicao: f64,
    pub observacao: Option<String>,
}

/// Alerta sobre a síntese de um aluno, para o professor ver ANTES do fechamento.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "tipo", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AlertaDesempenho {
    /// Um instrumento passa da fração máxima do peso efetivo.
    InstrumentoUnicoDomina {
        instrumento: String,
        percentual: f64,
    },
    /// O peso configurado da Prova Paulista passava do teto (e foi limitado).
    ProvaPaulistaAcimaDoTeto {
        percentual_configurado: f64,
        teto_percentual: f64,
    },
    /// Nota muito longe da média dos outros instrumentos (erro de lançamento ou caso a registrar).
    Discrepancia {
        instrumento: String,
        nota: f64,
        media_dos_outros: f64,
    },
    AbaixoDoSatisfatorio {
        nota: u8,
    },
    SemInstrumentosSuficientes {
        quantidade: usize,
        minimo: usize,
    },
}

impl AlertaDesempenho {
    /// Código curto e estável.
    pub fn codigo(&self) -> &'static str {
        match self {
            Self::InstrumentoUnicoDomina { .. } => "instrumento_unico_domina",
            Self::ProvaPaulistaAcimaDoTeto { .. } => "prova_paulista_acima_do_teto",
            Self::Discrepancia { .. } => "discrepancia",
            Self::AbaixoDoSatisfatorio { .. } => "abaixo_do_satisfatorio",
            Self::SemInstrumentosSuficientes { .. } => "sem_instrumentos_suficientes",
        }
    }

    pub fn mensagem(&self) -> String {
        match self {
            Self::InstrumentoUnicoDomina {
                instrumento,
                percentual,
            } => format!(
                "\"{instrumento}\" responde por {}% da síntese.",
                numero_br(*percentual)
            ),
            Self::ProvaPaulistaAcimaDoTeto {
                percentual_configurado,
                teto_percentual,
            } => format!(
                "A Prova Paulista estava com {}% do peso; o teto é {}%.",
                numero_br(*percentual_configurado),
                numero_br(*teto_percentual)
            ),
            Self::Discrepancia {
                instrumento,
                nota,
                media_dos_outros,
            } => format!(
                "\"{instrumento}\" teve nota {}, longe da média {} dos outros instrumentos.",
                numero_br(*nota),
                numero_br(*media_dos_outros)
            ),
            Self::AbaixoDoSatisfatorio { nota } => {
                format!("Síntese {nota}: abaixo do satisfatório (5, Res. SE 62/2019, art. 2º).")
            }
            Self::SemInstrumentosSuficientes { quantidade, minimo } => format!(
                "Só {} com nota; o mínimo configurado é {minimo}.",
                plural(*quantidade as u64, "instrumento", "instrumentos")
            ),
        }
    }

    pub fn o_que_fazer(&self) -> String {
        match self {
            Self::InstrumentoUnicoDomina { .. } => "Distribuir o peso entre diferentes instrumentos: a síntese decorre de \
                 diferentes instrumentos e prevalecem os resultados ao longo do período sobre provas \
                 finais (Res. SE 62/2019, art. 1º, parágrafo único)."
                .to_string(),
            Self::ProvaPaulistaAcimaDoTeto {
                teto_percentual, ..
            } => format!(
                "O peso foi limitado a {}% e o excedente foi redistribuído aos outros instrumentos. \
                 Confirmar com a escola o percentual adotado (até 30% segundo a imprensa; não \
                 conferido em norma).",
                numero_br(*teto_percentual)
            ),
            Self::Discrepancia { .. } => "Conferir o lançamento (erro de digitação?) e, se a nota estiver certa, \
                 registrar o motivo antes do fechamento."
                .to_string(),
            Self::AbaixoDoSatisfatorio { .. } => "Oferecer recuperação paralela com nova atividade avaliativa e registrar a \
                 nota dela. Nunca alterar nota sem nova avaliação."
                .to_string(),
            Self::SemInstrumentosSuficientes { .. } => "Incluir outros instrumentos (atividade, caderno, participação, plataforma) \
                 antes do fechamento (Res. SE 62/2019, art. 1º, parágrafo único)."
                .to_string(),
        }
    }
}

/// Resultado da síntese bimestral de um aluno num componente.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SinteseBimestral {
    /// Inteiro de 0 a 10 (Res. SE 62/2019, art. 1º).
    pub nota: u8,
    /// Média ponderada dos instrumentos (sem a recuperação).
    pub media_ponderada: f64,
    /// Média depois da recuperação, antes do arredondamento.
    pub media_final: f64,
    pub recuperacao_aplicada: bool,
    /// Um item por instrumento, na ordem recebida.
    pub detalhamento: Vec<DetalheInstrumento>,
    pub alertas: Vec<AlertaDesempenho>,
}

/// Calcula a síntese bimestral: média ponderada, Prova Paulista limitada ao teto (o peso excedente
/// vai proporcionalmente aos outros instrumentos), recuperação conforme a política da escola e
/// arredondamento para inteiro. Instrumento sem nota fica fora do cálculo (aparece no detalhamento).
pub fn sintese_bimestral(
    instrumentos: &[InstrumentoAvaliacao],
    cfg: &RegrasAvaliacao,
) -> Result<SinteseBimestral> {
    cfg.validar()?;
    for instrumento in instrumentos {
        instrumento.validar()?;
    }

    // Entram na média: com nota e fora da recuperação.
    let na_media: Vec<(&InstrumentoAvaliacao, f64)> = instrumentos
        .iter()
        .filter(|i| !i.eh_recuperacao())
        .filter_map(|i| i.nota.map(|n| (i, n)))
        .collect();
    if na_media.is_empty() {
        return Err(CoreError::SemNotaLancada);
    }

    let peso_pp: f64 = na_media
        .iter()
        .filter(|(i, _)| i.eh_prova_paulista())
        .map(|(i, _)| i.peso)
        .sum();
    let peso_outros: f64 = na_media
        .iter()
        .filter(|(i, _)| !i.eh_prova_paulista())
        .map(|(i, _)| i.peso)
        .sum();
    let peso_total = peso_pp + peso_outros;
    let teto = cfg.teto_prova_paulista;
    let fracao_pp = peso_pp / peso_total;
    let acima_do_teto = fracao_pp > teto + EPSILON;
    // Só há para onde redistribuir se houver outro instrumento com nota.
    let redistribui = acima_do_teto && peso_outros > 0.0;
    let peso_efetivo = |i: &InstrumentoAvaliacao| -> f64 {
        match (redistribui, i.eh_prova_paulista()) {
            (false, _) => i.peso / peso_total,
            (true, true) => i.peso / peso_pp * teto,
            (true, false) => i.peso / peso_outros * (1.0 - teto),
        }
    };

    let media_ponderada: f64 = na_media.iter().map(|(i, n)| peso_efetivo(i) * n).sum();

    // A maior nota de recuperação (em empate, a primeira).
    let recuperacao = instrumentos
        .iter()
        .enumerate()
        .filter(|(_, i)| i.eh_recuperacao())
        .filter_map(|(k, i)| i.nota.map(|n| (k, n)))
        .fold(None, |melhor: Option<(usize, f64)>, (k, n)| match melhor {
            Some((_, m)) if m >= n => melhor,
            _ => Some((k, n)),
        });
    let (media_final, recuperacao_usada) = match recuperacao {
        Some((k, r)) if r > media_ponderada + EPSILON => {
            let nova = match cfg.politica_recuperacao {
                PoliticaRecuperacao::SubstituiSeMaior => r,
                PoliticaRecuperacao::MediaComOriginal => (media_ponderada + r) / 2.0,
            };
            (nova, Some(k))
        }
        _ => (media_ponderada, None),
    };
    let nota = cfg.arredondamento.aplicar(media_final);

    let detalhamento = instrumentos
        .iter()
        .enumerate()
        .map(|(k, i)| {
            let (peso_ef, contribuicao, observacao) = match (i.eh_recuperacao(), i.nota) {
                (_, None) => (0.0, 0.0, Some("sem nota: fora do cálculo".to_string())),
                (true, Some(_)) => {
                    let obs = match (recuperacao_usada, cfg.politica_recuperacao) {
                        (Some(usada), PoliticaRecuperacao::SubstituiSeMaior) if usada == k => {
                            "recuperação maior que a média: passou a ser a síntese"
                        }
                        (Some(usada), PoliticaRecuperacao::MediaComOriginal) if usada == k => {
                            "recuperação maior que a média: síntese = média entre as duas"
                        }
                        (Some(_), _) => "outra recuperação, com nota maior, foi a considerada",
                        (None, _) => "recuperação não supera a média: síntese mantida",
                    };
                    (0.0, 0.0, Some(obs.to_string()))
                }
                (false, Some(n)) => {
                    let p = peso_efetivo(i);
                    let obs = if i.eh_prova_paulista() && acima_do_teto {
                        Some(if redistribui {
                            format!("peso limitado ao teto de {}%", numero_br(teto * 100.0))
                        } else {
                            "sem outro instrumento para receber o peso acima do teto".to_string()
                        })
                    } else if redistribui {
                        Some("recebeu parte do peso acima do teto da Prova Paulista".to_string())
                    } else {
                        None
                    };
                    (p, p * n, obs)
                }
            };
            DetalheInstrumento {
                nome: i.nome.clone(),
                tipo: i.tipo.clone(),
                nota: i.nota,
                peso_informado: i.peso,
                peso_efetivo: peso_ef,
                contribuicao,
                observacao,
            }
        })
        .collect();

    let mut alertas = Vec::new();
    if na_media.len() < cfg.minimo_instrumentos {
        alertas.push(AlertaDesempenho::SemInstrumentosSuficientes {
            quantidade: na_media.len(),
            minimo: cfg.minimo_instrumentos,
        });
    }
    if acima_do_teto {
        alertas.push(AlertaDesempenho::ProvaPaulistaAcimaDoTeto {
            percentual_configurado: uma_casa(fracao_pp * 100.0),
            teto_percentual: uma_casa(teto * 100.0),
        });
    }
    for (i, _) in &na_media {
        let p = peso_efetivo(i);
        if p > cfg.fracao_maxima_instrumento + EPSILON {
            alertas.push(AlertaDesempenho::InstrumentoUnicoDomina {
                instrumento: i.nome.clone(),
                percentual: uma_casa(p * 100.0),
            });
        }
    }
    // Com só 2 instrumentos a discrepância seria mútua (os dois acusariam): exige pelo menos 3.
    if na_media.len() >= 3 {
        let soma: f64 = na_media.iter().map(|(_, n)| n).sum();
        let outros = (na_media.len() - 1) as f64;
        for (i, n) in &na_media {
            let media_dos_outros = (soma - n) / outros;
            if (n - media_dos_outros).abs() + EPSILON >= cfg.limiar_discrepancia {
                alertas.push(AlertaDesempenho::Discrepancia {
                    instrumento: i.nome.clone(),
                    nota: *n,
                    media_dos_outros,
                });
            }
        }
    }
    if nota < cfg.nota_satisfatoria {
        alertas.push(AlertaDesempenho::AbaixoDoSatisfatorio { nota });
    }

    Ok(SinteseBimestral {
        nota,
        media_ponderada,
        media_final,
        recuperacao_aplicada: recuperacao_usada.is_some(),
        detalhamento,
        alertas,
    })
}

// ---------------------------------------------------------------------------------------------
// Discrepância de turma
// ---------------------------------------------------------------------------------------------

/// Limites da comparação de uma turma com a referência (ex.: outras turmas da mesma série).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LimitesTurma {
    /// Diferença entre as médias (em pontos) que gera alerta.
    pub diferenca_media: f64,
    /// Diferença no percentual de notas abaixo do satisfatório (em pontos percentuais).
    pub diferenca_percentual_abaixo: f64,
    /// Mínimo de notas em cada lado (turma pequena dá média instável).
    pub minimo_notas: usize,
    pub nota_satisfatoria: u8,
}

impl Default for LimitesTurma {
    fn default() -> Self {
        Self {
            diferenca_media: 1.5,
            diferenca_percentual_abaixo: 20.0,
            minimo_notas: 5,
            nota_satisfatoria: NOTA_SATISFATORIA,
        }
    }
}

impl LimitesTurma {
    pub fn validar(&self) -> Result<()> {
        if !(self.diferenca_media.is_finite() && self.diferenca_media > 0.0) {
            return Err(fora("diferenca_media", self.diferenca_media, "maior que 0"));
        }
        let p = self.diferenca_percentual_abaixo;
        if !(p.is_finite() && p > 0.0 && p <= 100.0) {
            return Err(fora(
                "diferenca_percentual_abaixo",
                p,
                "maior que 0 e até 100",
            ));
        }
        if self.nota_satisfatoria > NOTA_MAXIMA {
            return Err(fora("nota_satisfatoria", self.nota_satisfatoria, "0 a 10"));
        }
        Ok(())
    }
}

/// Para que lado a turma se afasta da referência.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DirecaoDiscrepancia {
    AbaixoDaReferencia,
    /// Nota muito acima da série também é questionada pela supervisão.
    AcimaDaReferencia,
}

/// Aviso de turma fora do padrão, para o professor ver ANTES do fechamento.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlertaTurma {
    pub direcao: DirecaoDiscrepancia,
    pub media_turma: f64,
    pub media_referencia: f64,
    pub percentual_abaixo_turma: f64,
    pub percentual_abaixo_referencia: f64,
    pub mensagem: String,
    pub o_que_fazer: String,
}

fn media_e_percentual_abaixo(notas: &[u8], satisfatoria: u8) -> (f64, f64) {
    let n = notas.len() as f64;
    let soma: u32 = notas.iter().map(|x| u32::from(*x)).sum();
    let abaixo = notas.iter().filter(|x| **x < satisfatoria).count() as f64;
    (f64::from(soma) / n, abaixo * 100.0 / n)
}

/// Compara as sínteses da turma com as de referência usando os limites padrão.
pub fn discrepancia_turma(
    notas_da_turma: &[u8],
    notas_de_referencia: &[u8],
) -> Option<AlertaTurma> {
    discrepancia_turma_com_limites(
        notas_da_turma,
        notas_de_referencia,
        &LimitesTurma::default(),
    )
    .ok()
    .flatten()
}

/// Compara as sínteses da turma com as de referência. Notas acima de 10 são ignoradas; com menos
/// de `minimo_notas` válidas em algum lado não há alerta.
pub fn discrepancia_turma_com_limites(
    notas_da_turma: &[u8],
    notas_de_referencia: &[u8],
    limites: &LimitesTurma,
) -> Result<Option<AlertaTurma>> {
    limites.validar()?;
    let validas = |notas: &[u8]| -> Vec<u8> {
        notas
            .iter()
            .copied()
            .filter(|n| *n <= NOTA_MAXIMA)
            .collect()
    };
    let turma = validas(notas_da_turma);
    let referencia = validas(notas_de_referencia);
    let minimo = limites.minimo_notas.max(1);
    if turma.len() < minimo || referencia.len() < minimo {
        return Ok(None);
    }
    let sat = limites.nota_satisfatoria;
    let (media_t, abaixo_t) = media_e_percentual_abaixo(&turma, sat);
    let (media_r, abaixo_r) = media_e_percentual_abaixo(&referencia, sat);
    let lim_m = limites.diferenca_media;
    let lim_p = limites.diferenca_percentual_abaixo;

    let abaixo = media_t <= media_r - lim_m + EPSILON || abaixo_t >= abaixo_r + lim_p - EPSILON;
    let acima = media_t >= media_r + lim_m - EPSILON || abaixo_t <= abaixo_r - lim_p + EPSILON;
    let (direcao, o_que_fazer) = if abaixo {
        (
            DirecaoDiscrepancia::AbaixoDaReferencia,
            "Rever os instrumentos e oferecer recuperação antes do fechamento; ter à mão o \
             registro de cada instrumento para o Conselho de Classe e a supervisão.",
        )
    } else if acima {
        (
            DirecaoDiscrepancia::AcimaDaReferencia,
            "Conferir se instrumentos e critérios são comparáveis aos das outras turmas e \
             manter o registro de cada instrumento: nota muito acima da série também é \
             questionada.",
        )
    } else {
        return Ok(None);
    };
    let mensagem = format!(
        "Média da turma {} contra {} da referência; {}% abaixo de {sat} (referência: {}%).",
        numero_br(media_t),
        numero_br(media_r),
        numero_br(abaixo_t),
        numero_br(abaixo_r)
    );
    Ok(Some(AlertaTurma {
        direcao,
        media_turma: media_t,
        media_referencia: media_r,
        percentual_abaixo_turma: abaixo_t,
        percentual_abaixo_referencia: abaixo_r,
        mensagem,
        o_que_fazer: o_que_fazer.to_string(),
    }))
}

// ---------------------------------------------------------------------------------------------
// Frequência
// ---------------------------------------------------------------------------------------------

/// Limites de frequência, em percentual inteiro (a comparação é feita em inteiros: 75% exatos
/// não viram 74,999...).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegrasFrequencia {
    /// LDB, art. 24, VI: 75.
    pub minimo_percentual: u32,
    /// Abaixo deste (e no mínimo ou acima dele) o risco é `Atencao`. Padrão 80.
    pub atencao_percentual: u32,
}

impl Default for RegrasFrequencia {
    fn default() -> Self {
        Self {
            minimo_percentual: 75,
            atencao_percentual: 80,
        }
    }
}

impl RegrasFrequencia {
    pub fn validar(&self) -> Result<()> {
        if self.atencao_percentual > 100 {
            return Err(fora(
                "atencao_percentual",
                self.atencao_percentual,
                "0 a 100",
            ));
        }
        if self.minimo_percentual > self.atencao_percentual {
            return Err(fora(
                "minimo_percentual",
                self.minimo_percentual,
                "até o atencao_percentual",
            ));
        }
        Ok(())
    }
}

/// Risco de retenção por falta.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RiscoFrequencia {
    Ok,
    /// No mínimo ou acima dele, mas abaixo do percentual de atenção (75% a 80%, por padrão).
    Atencao,
    /// Abaixo do mínimo (LDB, art. 24, VI).
    Retencao,
}

/// Situação de frequência de um aluno num componente (Res. SE 62/2019, art. 5º: no Ensino Médio,
/// por número de aulas do componente).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SituacaoFrequencia {
    pub aulas_dadas: u32,
    pub faltas: u32,
    /// Percentual de presença (100 quando ainda não há aula).
    pub percentual: f64,
    pub minimo: u32,
    /// Maior número de faltas que ainda deixa a presença no mínimo, sobre as aulas informadas.
    pub faltas_maximas_permitidas: u32,
    pub faltas_acima_do_limite: u32,
    /// Aulas de compensação que trazem a presença de volta ao mínimo. Premissa [Provável, a
    /// confirmar]: cada aula compensada abona uma falta (então é igual a `faltas_acima_do_limite`).
    /// O procedimento oficial de compensação de ausências NÃO foi verificado: confirmar com a escola.
    pub aulas_a_compensar: u32,
    pub risco: RiscoFrequencia,
}

/// Situação de frequência. `aulas_dadas` é a base do percentual: as aulas já dadas (situação de
/// agora) ou as previstas no ano (para saber o teto anual de faltas).
pub fn situacao_frequencia(
    aulas_dadas: u32,
    faltas: u32,
    cfg: &RegrasFrequencia,
) -> Result<SituacaoFrequencia> {
    cfg.validar()?;
    if faltas > aulas_dadas {
        return Err(CoreError::FaltasAcimaDasAulas {
            faltas,
            aulas: aulas_dadas,
        });
    }
    let presencas = aulas_dadas - faltas;
    let aulas = u64::from(aulas_dadas);
    let percentual = if aulas_dadas == 0 {
        100.0
    } else {
        f64::from(presencas) * 100.0 / f64::from(aulas_dadas)
    };
    // Maior f com (aulas - f) * 100 >= minimo * aulas, ou seja, f * 100 <= (100 - minimo) * aulas.
    let faltas_maximas_permitidas =
        u32::try_from(u64::from(100 - cfg.minimo_percentual) * aulas / 100).unwrap_or(u32::MAX);
    let faltas_acima_do_limite = faltas.saturating_sub(faltas_maximas_permitidas);
    let presencas_x100 = u64::from(presencas) * 100;
    let risco = if aulas_dadas == 0 {
        RiscoFrequencia::Ok
    } else if presencas_x100 < u64::from(cfg.minimo_percentual) * aulas {
        RiscoFrequencia::Retencao
    } else if presencas_x100 < u64::from(cfg.atencao_percentual) * aulas {
        RiscoFrequencia::Atencao
    } else {
        RiscoFrequencia::Ok
    };
    Ok(SituacaoFrequencia {
        aulas_dadas,
        faltas,
        percentual,
        minimo: cfg.minimo_percentual,
        faltas_maximas_permitidas,
        faltas_acima_do_limite,
        aulas_a_compensar: faltas_acima_do_limite,
        risco,
    })
}

// ---------------------------------------------------------------------------------------------
// Risco final ("quanto preciso tirar?", "vou repetir por falta?")
// ---------------------------------------------------------------------------------------------

/// Risco de reprovação no componente.
///
/// Premissa [Provável]: a estimativa usa a média aritmética dos bimestres. A nota final, pela
/// Res. SE 62/2019, art. 1º, é a síntese do professor, e o parecer final é do Conselho de Classe
/// (art. 7º): o número é orientação, não decisão.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RiscoFinal {
    /// Média dos bimestres com nota (`None` se nenhum tem).
    pub media_parcial: Option<f64>,
    pub bimestres_com_nota: u32,
    pub bimestres_restantes: u32,
    /// Menor nota inteira que, tirada em CADA bimestre restante, leva a média a >= satisfatório.
    /// `Some(0)`: já garantida. `None`: impossível (passaria de 10) ou não resta bimestre.
    pub nota_necessaria_nos_bimestres_restantes: Option<u8>,
    /// Precisa de nota acima da média atual, ou a média já não alcança o satisfatório.
    pub por_nota: bool,
    /// Frequência abaixo do mínimo.
    pub por_falta: bool,
    pub mensagem: String,
}

/// Risco final a partir das sínteses de cada bimestre (`None` = bimestre ainda sem nota, contado
/// como restante) e da situação de frequência.
pub fn risco_final(
    sinteses_bimestrais: &[Option<u8>],
    frequencia: &SituacaoFrequencia,
    cfg: &RegrasAvaliacao,
) -> Result<RiscoFinal> {
    cfg.validar()?;
    if sinteses_bimestrais.len() > usize::from(cfg.bimestres) {
        return Err(CoreError::BimestresDemais {
            informados: sinteses_bimestrais.len(),
            maximo: cfg.bimestres,
        });
    }
    let mut soma: u32 = 0;
    let mut com_nota: u32 = 0;
    for nota in sinteses_bimestrais.iter().flatten() {
        if *nota > NOTA_MAXIMA {
            return Err(fora("sinteses_bimestrais", nota, "0 a 10"));
        }
        soma += u32::from(*nota);
        com_nota += 1;
    }
    let total = u32::from(cfg.bimestres);
    let restantes = total - com_nota;
    let media_parcial = (com_nota > 0).then(|| f64::from(soma) / f64::from(com_nota));
    let sat = cfg.nota_satisfatoria;
    let falta = (u32::from(sat) * total).saturating_sub(soma);
    let necessaria = if falta == 0 {
        Some(0)
    } else if restantes == 0 {
        None
    } else {
        u8::try_from(falta.div_ceil(restantes))
            .ok()
            .filter(|x| *x <= NOTA_MAXIMA)
    };
    let por_nota = match necessaria {
        None => true,
        Some(x) => restantes > 0 && media_parcial.is_some_and(|m| f64::from(x) > m + EPSILON),
    };
    let por_falta = frequencia.risco == RiscoFrequencia::Retencao;

    let parte_nota = match (media_parcial, necessaria, restantes) {
        (None, Some(x), _) => format!(
            "Ainda sem nota bimestral: para média {sat}, basta {x} em cada um dos {total} bimestres."
        ),
        (Some(m), Some(0), 0) => format!("Média final {}: satisfatória.", numero_br(m)),
        (Some(m), Some(0), _) => format!(
            "Média parcial {} em {}: a média {sat} já está garantida.",
            numero_br(m),
            plural(com_nota, "bimestre", "bimestres")
        ),
        (Some(m), Some(x), r) => format!(
            "Média parcial {} em {}: precisa de pelo menos {x} {} para fechar com média {sat}.",
            numero_br(m),
            plural(com_nota, "bimestre", "bimestres"),
            nos_restantes(r)
        ),
        (m, None, 0) => format!(
            "Média final {}: abaixo de {sat}. O parecer final é do Conselho de Classe (Res. SE \
             62/2019, art. 7º).",
            m.map_or_else(|| "-".to_string(), numero_br)
        ),
        (m, None, r) => format!(
            "Média parcial {}: mesmo com 10 {} a média não chega a {sat}. Restam a \
             recuperação e o Conselho de Classe (Res. SE 62/2019, art. 7º).",
            m.map_or_else(|| "-".to_string(), numero_br),
            nos_restantes(r)
        ),
    };
    let parte_falta = match frequencia.risco {
        RiscoFrequencia::Retencao => format!(
            "Frequência {}%: abaixo do mínimo de {}% (LDB, art. 24, VI); {} acima do limite. \
             Compensação de ausências: confirmar o procedimento com a escola.",
            numero_br(frequencia.percentual),
            frequencia.minimo,
            plural(frequencia.faltas_acima_do_limite, "falta", "faltas")
        ),
        RiscoFrequencia::Atencao => format!(
            "Frequência {}%: perto do mínimo de {}%.",
            numero_br(frequencia.percentual),
            frequencia.minimo
        ),
        RiscoFrequencia::Ok => format!(
            "Frequência {}%: acima do mínimo.",
            numero_br(frequencia.percentual)
        ),
    };
    Ok(RiscoFinal {
        media_parcial,
        bimestres_com_nota: com_nota,
        bimestres_restantes: restantes,
        nota_necessaria_nos_bimestres_restantes: necessaria,
        por_nota,
        por_falta,
        mensagem: format!("{parte_nota} {parte_falta}"),
    })
}

// ---------------------------------------------------------------------------------------------
// Pedido de alteração de notas (acompanhamento)
// ---------------------------------------------------------------------------------------------

/// Resposta ao pedido de alteração de notas, classificada a partir do texto recebido.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "resposta", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RespostaPedido {
    Pendente,
    Deferido {
        /// AAAA-MM-DD.
        liberacao_inicio: Option<String>,
        /// AAAA-MM-DD.
        liberacao_fim: Option<String>,
        /// Texto recebido, sanitizado (números longos ocultos, tamanho limitado).
        instrucoes: String,
    },
    Indeferido {
        motivo: String,
    },
    PedeInformacao {
        o_que: String,
    },
}

impl RespostaPedido {
    pub fn validar(&self) -> Result<()> {
        if let Self::Deferido {
            liberacao_inicio,
            liberacao_fim,
            ..
        } = self
        {
            if let Some(d) = liberacao_inicio {
                validar_data_iso(d)?;
            }
            if let Some(d) = liberacao_fim {
                validar_data_iso(d)?;
            }
            if let (Some(i), Some(f)) = (liberacao_inicio, liberacao_fim) {
                if i > f {
                    return Err(CoreError::DataInvalida(format!("{i} depois de {f}")));
                }
            }
        }
        Ok(())
    }

    /// Dias corridos da janela de lançamento, contando o primeiro e o último.
    pub fn dias_da_janela(&self) -> Option<i64> {
        match self {
            Self::Deferido {
                liberacao_inicio: Some(i),
                liberacao_fim: Some(f),
                ..
            } => Some(dia_juliano(f).ok()? - dia_juliano(i).ok()? + 1),
            _ => None,
        }
    }
}

/// Raízes (início de palavra, texto sem acento) que indicam deferimento.
const RAIZES_DEFERIDO: [&str; 7] = [
    "deferid",
    "autorizad",
    "autorizamos",
    "liberad",
    "liberamos",
    "reabert",
    "reabrimos",
];
const RAIZES_INDEFERIDO: [&str; 3] = ["indeferid", "negad", "negamos"];
/// Verbos de pedido de informação. Substantivos como "solicitação" e "encaminhada" ficam de fora:
/// "sua solicitação foi encaminhada" é andamento, não pedido.
const VERBOS_INFORMACAO: [&str; 17] = [
    "solicitamos",
    "solicita",
    "solicito",
    "solicite",
    "encaminhe",
    "encaminhem",
    "encaminhar",
    "apresente",
    "apresentem",
    "apresentar",
    "informe",
    "informem",
    "informar",
    "envie",
    "enviar",
    "anexe",
    "anexar",
];
/// Antes da decisão, indicam condição ("para que seja liberado", "caso seja deferido").
const CONDICIONAIS: [&str; 4] = ["para", "caso", "quando", "apos"];
const SUBJUNTIVOS: [&str; 6] = ["seja", "sejam", "for", "forem", "fosse", "estiver"];

fn sem_acento(c: char) -> char {
    match c {
        'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'í' | 'ì' | 'î' | 'ï' => 'i',
        'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
        'ú' | 'ù' | 'û' | 'ü' => 'u',
        'ç' => 'c',
        outro => outro,
    }
}

/// Minúsculas e sem acento, caractere a caractere.
fn dobrar(texto: &str) -> Vec<char> {
    texto
        .chars()
        .flat_map(char::to_lowercase)
        .map(sem_acento)
        .collect()
}

struct Palavra {
    inicio: usize,
    texto: String,
}

fn palavras(chars: &[char]) -> Vec<Palavra> {
    let mut saida = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_alphanumeric() {
            let inicio = i;
            while i < chars.len() && chars[i].is_alphanumeric() {
                i += 1;
            }
            saida.push(Palavra {
                inicio,
                texto: chars[inicio..i].iter().collect(),
            });
        } else {
            i += 1;
        }
    }
    saida
}

fn comeca_com_alguma(palavra: &str, raizes: &[&str]) -> bool {
    raizes.iter().any(|r| palavra.starts_with(r))
}

fn ler_numero(chars: &[char], inicio: usize, min: usize, max: usize) -> Option<(u32, usize)> {
    let mut j = inicio;
    let mut valor = 0u32;
    while j < chars.len() && j - inicio < max {
        match chars[j].to_digit(10) {
            Some(d) => {
                valor = valor * 10 + d;
                j += 1;
            }
            None => break,
        }
    }
    if j - inicio < min || chars.get(j).is_some_and(char::is_ascii_digit) {
        return None;
    }
    Some((valor, j))
}

/// Lê `d/m/aaaa` (dia e mês com 1 ou 2 dígitos) a partir de `inicio`. Devolve o fim e a data ISO.
fn ler_data(chars: &[char], inicio: usize) -> Option<(usize, String)> {
    let (dia, j) = ler_numero(chars, inicio, 1, 2)?;
    if chars.get(j) != Some(&'/') {
        return None;
    }
    let (mes, j) = ler_numero(chars, j + 1, 1, 2)?;
    if chars.get(j) != Some(&'/') {
        return None;
    }
    let (ano, j) = ler_numero(chars, j + 1, 4, 4)?;
    let iso = format!("{ano:04}-{mes:02}-{dia:02}");
    validar_data_iso(&iso).ok()?;
    Some((j, iso))
}

/// Datas `dd/mm/aaaa` válidas no texto, com a posição de cada uma.
fn datas_no_texto(chars: &[char]) -> Vec<(usize, String)> {
    let mut datas = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let comeca_numero = chars[i].is_ascii_digit()
            && (i == 0 || !(chars[i - 1].is_ascii_digit() || chars[i - 1] == '/'));
        if comeca_numero {
            if let Some((fim, iso)) = ler_data(chars, i) {
                datas.push((i, iso));
                i = fim;
                continue;
            }
        }
        i += 1;
    }
    datas
}

/// A data em `posicao` vem logo depois de "até" (até 3 palavras antes)?
fn precedida_de_ate(ps: &[Palavra], posicao: usize) -> bool {
    let antes: Vec<&str> = ps
        .iter()
        .filter(|p| p.inicio < posicao)
        .map(|p| p.texto.as_str())
        .collect();
    antes.iter().rev().take(3).any(|p| *p == "ate")
}

/// Janela de liberação: as datas depois da palavra de decisão (se houver), senão todas.
fn janela(
    ps: &[Palavra],
    datas: &[(usize, String)],
    desde: usize,
) -> (Option<String>, Option<String>) {
    let depois: Vec<&(usize, String)> = datas.iter().filter(|(p, _)| *p >= desde).collect();
    let usadas: Vec<&(usize, String)> = if depois.is_empty() {
        datas.iter().collect()
    } else {
        depois
    };
    match usadas.as_slice() {
        [] => (None, None),
        [(posicao, data)] => {
            if precedida_de_ate(ps, *posicao) {
                (None, Some(data.clone()))
            } else {
                (Some(data.clone()), None)
            }
        }
        [(_, a), (_, b), ..] => {
            if a <= b {
                (Some(a.clone()), Some(b.clone()))
            } else {
                (Some(b.clone()), Some(a.clone()))
            }
        }
    }
}

/// Frases do texto original que contêm um verbo de pedido de informação.
fn frases_de_pedido(texto: &str) -> String {
    let frases: Vec<&str> = texto
        .split(['.', '!', '?', ';', '\n'])
        .map(str::trim)
        .filter(|f| {
            palavras(&dobrar(f))
                .iter()
                .any(|p| VERBOS_INFORMACAO.contains(&p.texto.as_str()))
        })
        .collect();
    if frases.is_empty() {
        sanitizar_detalhe(texto.trim())
    } else {
        sanitizar_detalhe(&frases.join("; "))
    }
}

/// Classifica a resposta ao pedido de alteração de notas por palavras (heurística):
/// indeferimento (inclusive "não foi autorizado") vence; depois deferimento não condicional, com a
/// janela tirada das datas `dd/mm/aaaa`; depois pedido de informação; o resto é `Pendente`.
/// O professor confere sempre: é uma leitura de apoio, não a decisão.
pub fn classificar_resposta(texto: &str) -> RespostaPedido {
    let chars = dobrar(texto);
    let ps = palavras(&chars);
    let textos: Vec<&str> = ps.iter().map(|p| p.texto.as_str()).collect();

    let mut indeferido = textos.windows(3).any(|w| w == ["nao", "sera", "possivel"]);
    let mut decisao: Option<usize> = None;
    for (k, palavra) in textos.iter().enumerate() {
        if comeca_com_alguma(palavra, &RAIZES_INDEFERIDO) {
            indeferido = true;
            continue;
        }
        let eh_decisao = comeca_com_alguma(palavra, &RAIZES_DEFERIDO);
        if !(eh_decisao || palavra.starts_with("autoriz")) {
            continue;
        }
        let antes = &textos[k.saturating_sub(3)..k];
        let ultimas_duas = &textos[k.saturating_sub(2)..k];
        if ultimas_duas.contains(&"nao") {
            indeferido = true; // "não foi autorizado", "não será liberado", "não autorizamos"
            continue;
        }
        let condicional = antes.iter().any(|a| CONDICIONAIS.contains(a))
            || ultimas_duas.iter().any(|a| SUBJUNTIVOS.contains(a))
            || antes.last() == Some(&"se");
        if eh_decisao && !condicional && decisao.is_none() {
            decisao = Some(ps[k].inicio);
        }
    }

    if indeferido {
        return RespostaPedido::Indeferido {
            motivo: sanitizar_detalhe(texto.trim()),
        };
    }
    if let Some(desde) = decisao {
        let (liberacao_inicio, liberacao_fim) = janela(&ps, &datas_no_texto(&chars), desde);
        return RespostaPedido::Deferido {
            liberacao_inicio,
            liberacao_fim,
            instrucoes: sanitizar_detalhe(texto.trim()),
        };
    }
    if textos.iter().any(|p| VERBOS_INFORMACAO.contains(p)) {
        return RespostaPedido::PedeInformacao {
            o_que: frases_de_pedido(texto),
        };
    }
    RespostaPedido::Pendente
}

/// O próximo passo do professor diante da resposta. Só descreve: quem lança ou responde é ele.
pub fn proximo_passo(resposta: &RespostaPedido) -> String {
    match resposta {
        RespostaPedido::Pendente => "Sem decisão ainda: guardar o número do protocolo e acompanhar; se \
             demorar, perguntar à gestão da escola em que etapa está o pedido."
            .to_string(),
        RespostaPedido::Deferido {
            liberacao_inicio,
            liberacao_fim,
            instrucoes,
        } => {
            let janela = match (liberacao_inicio, liberacao_fim) {
                (Some(i), Some(f)) => {
                    let dias = resposta
                        .dias_da_janela()
                        .map(|d| format!(" ({}, contando o primeiro e o último)", plural(d.max(0) as u64, "dia", "dias")))
                        .unwrap_or_default();
                    format!(
                        "Lançar as notas no Diário entre {} e {}{dias}",
                        data_br(i),
                        data_br(f)
                    )
                }
                (None, Some(f)) => format!("Lançar as notas no Diário até {}", data_br(f)),
                (Some(i), None) => format!(
                    "Diário liberado a partir de {}; confirmar com a escola até quando, e lançar \
                     dentro desse prazo",
                    data_br(i)
                ),
                (None, None) => "Pedido deferido sem janela de datas: confirmar com a escola quando \
                     o Diário fica liberado e lançar dentro desse prazo"
                    .to_string(),
            };
            format!(
                "{janela}, com a nova avaliação que justifica a alteração registrada. Instruções \
                 recebidas: {instrucoes}"
            )
        }
        RespostaPedido::Indeferido { motivo } => format!(
            "Indeferido (motivo informado: {motivo}). Caminhos: (1) réplica pedindo reconsideração, \
             citando a Resolução SE 62/2019, art. 1º, parágrafo único (a síntese decorre de \
             diferentes instrumentos, em avaliação contínua, prevalecendo os resultados ao longo do \
             período sobre provas finais) e o art. 7º (o Conselho de Classe emite o parecer final e \
             pode atribuir a nota final dentro do satisfatório); ou (2) recurso ao Conselho de \
             Classe com a recuperação documentada (atividade aplicada, data e nota). Nunca alterar \
             nota sem avaliação."
        ),
        RespostaPedido::PedeInformacao { o_que } => format!(
            "Juntar e enviar o que foi pedido: {o_que}. Ajuda ter à mão a lista de instrumentos \
             do bimestre com notas e pesos, o registro da recuperação (atividade, data e nota) e a \
             ata do Conselho de Classe, se houver."
        ),
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use TipoInstrumento::*;

    // Fixtures sintéticas: instrumentos, notas e textos fictícios.
    fn inst(nome: &str, tipo: TipoInstrumento, nota: f64, peso: f64) -> InstrumentoAvaliacao {
        InstrumentoAvaliacao::novo(nome, tipo, Some(nota), peso)
    }

    fn perto(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    fn codigos(s: &SinteseBimestral) -> Vec<&'static str> {
        s.alertas.iter().map(AlertaDesempenho::codigo).collect()
    }

    fn freq(aulas: u32, faltas: u32) -> SituacaoFrequencia {
        situacao_frequencia(aulas, faltas, &RegrasFrequencia::default()).unwrap()
    }

    #[test]
    fn media_ponderada_simples_sem_alertas() {
        let s = sintese_bimestral(
            &[
                inst("Atividade 1", Atividade, 8.0, 1.0),
                inst("Caderno", Caderno, 6.0, 1.0),
                inst("Participação", Participacao, 7.0, 2.0),
            ],
            &RegrasAvaliacao::default(),
        )
        .unwrap();
        assert!(perto(s.media_ponderada, 7.0));
        assert_eq!(s.nota, 7);
        assert!(s.alertas.is_empty(), "{:?}", s.alertas);
        assert!(perto(s.detalhamento[2].peso_efetivo, 0.5));
        assert!(perto(s.detalhamento[2].contribuicao, 3.5));
    }

    #[test]
    fn prova_paulista_limitada_ao_teto_com_redistribuicao() {
        // Pesos 2/1/1: a Prova Paulista teria 50%; o teto de 30% leva o excedente aos outros.
        let s = sintese_bimestral(
            &[
                inst("Prova Paulista", ProvaPaulista, 2.0, 2.0),
                inst("Atividade", Atividade, 8.0, 1.0),
                inst("Caderno", Caderno, 8.0, 1.0),
            ],
            &RegrasAvaliacao::default(),
        )
        .unwrap();
        let pesos: Vec<f64> = s.detalhamento.iter().map(|d| d.peso_efetivo).collect();
        assert!(perto(pesos[0], 0.30));
        assert!(perto(pesos[1], 0.35) && perto(pesos[2], 0.35));
        assert!(perto(pesos.iter().sum::<f64>(), 1.0));
        // 0,3*2 + 0,35*8 + 0,35*8 = 6,2 (sem o teto seria 5,0).
        assert!(perto(s.media_ponderada, 6.2));
        assert_eq!(s.nota, 6);
        assert_eq!(
            s.alertas,
            vec![
                AlertaDesempenho::ProvaPaulistaAcimaDoTeto {
                    percentual_configurado: 50.0,
                    teto_percentual: 30.0,
                },
                AlertaDesempenho::Discrepancia {
                    instrumento: "Prova Paulista".into(),
                    nota: 2.0,
                    media_dos_outros: 8.0,
                },
            ]
        );
        assert_eq!(
            s.alertas[0].mensagem(),
            "A Prova Paulista estava com 50% do peso; o teto é 30%."
        );
        assert_eq!(
            s.detalhamento[0].observacao.as_deref(),
            Some("peso limitado ao teto de 30%")
        );
    }

    #[test]
    fn prova_paulista_dentro_do_teto_nao_muda_e_teto_e_configuravel() {
        let instrumentos = [
            inst("Prova Paulista", ProvaPaulista, 5.0, 1.0),
            inst("Atividade", Atividade, 7.0, 2.0),
            inst("Plataforma", Plataforma, 7.0, 2.0),
        ];
        let s = sintese_bimestral(&instrumentos, &RegrasAvaliacao::default()).unwrap();
        assert!(perto(s.detalhamento[0].peso_efetivo, 0.2));
        assert!(!codigos(&s).contains(&"prova_paulista_acima_do_teto"));
        // Escola que não usa a Prova Paulista na nota: teto 0.
        let cfg = RegrasAvaliacao {
            teto_prova_paulista: 0.0,
            ..RegrasAvaliacao::default()
        };
        let s = sintese_bimestral(&instrumentos, &cfg).unwrap();
        assert!(perto(s.detalhamento[0].peso_efetivo, 0.0));
        assert!(perto(s.media_ponderada, 7.0));
    }

    #[test]
    fn so_prova_paulista_nao_tem_para_onde_redistribuir() {
        let s = sintese_bimestral(
            &[inst("Prova Paulista", ProvaPaulista, 6.0, 1.0)],
            &RegrasAvaliacao::default(),
        )
        .unwrap();
        assert_eq!(s.nota, 6);
        assert_eq!(
            codigos(&s),
            vec![
                "sem_instrumentos_suficientes",
                "prova_paulista_acima_do_teto",
                "instrumento_unico_domina"
            ]
        );
    }

    fn baixas_com_recuperacao(rec: f64) -> Vec<InstrumentoAvaliacao> {
        vec![
            inst("Atividade", Atividade, 3.0, 1.0),
            inst("Caderno", Caderno, 4.0, 1.0),
            inst("Participação", Participacao, 3.0, 1.0),
            inst("Recuperação paralela", Recuperacao, rec, 1.0),
        ]
    }

    #[test]
    fn recuperacao_maior_substitui_a_sintese() {
        let s =
            sintese_bimestral(&baixas_com_recuperacao(7.0), &RegrasAvaliacao::default()).unwrap();
        assert!(perto(s.media_ponderada, 10.0 / 3.0));
        assert!(s.recuperacao_aplicada);
        assert_eq!(s.nota, 7);
        assert!(!codigos(&s).contains(&"abaixo_do_satisfatorio"));
        assert_eq!(s.detalhamento[3].peso_efetivo, 0.0);
        assert_eq!(
            s.detalhamento[3].observacao.as_deref(),
            Some("recuperação maior que a média: passou a ser a síntese")
        );
    }

    #[test]
    fn recuperacao_em_media_com_a_original() {
        let cfg = RegrasAvaliacao {
            politica_recuperacao: PoliticaRecuperacao::MediaComOriginal,
            ..RegrasAvaliacao::default()
        };
        let s = sintese_bimestral(&baixas_com_recuperacao(7.0), &cfg).unwrap();
        // (3,33 + 7) / 2 = 5,17 -> 5.
        assert!(perto(s.media_final, (10.0 / 3.0 + 7.0) / 2.0));
        assert_eq!(s.nota, 5);
    }

    #[test]
    fn recuperacao_menor_nao_muda_e_alerta_abaixo_do_satisfatorio() {
        let s =
            sintese_bimestral(&baixas_com_recuperacao(2.0), &RegrasAvaliacao::default()).unwrap();
        assert!(!s.recuperacao_aplicada);
        assert_eq!(s.nota, 3);
        let alerta = s
            .alertas
            .iter()
            .find(|a| a.codigo() == "abaixo_do_satisfatorio")
            .unwrap();
        assert_eq!(alerta, &AlertaDesempenho::AbaixoDoSatisfatorio { nota: 3 });
        let fazer = alerta.o_que_fazer();
        assert!(fazer.contains("recuperação paralela com nova atividade"));
        assert!(fazer.contains("Nunca alterar nota sem nova avaliação"));
    }

    #[test]
    fn instrumento_unico_domina_a_sintese() {
        let s = sintese_bimestral(
            &[
                inst("Prova bimestral", Atividade, 6.0, 6.0),
                inst("Caderno", Caderno, 6.0, 2.0),
                inst("Participação", Participacao, 6.0, 2.0),
            ],
            &RegrasAvaliacao::default(),
        )
        .unwrap();
        assert_eq!(
            s.alertas,
            vec![AlertaDesempenho::InstrumentoUnicoDomina {
                instrumento: "Prova bimestral".into(),
                percentual: 60.0,
            }]
        );
        assert!(s.alertas[0]
            .o_que_fazer()
            .contains("art. 1º, parágrafo único"));
    }

    #[test]
    fn discrepancia_de_um_instrumento() {
        let s = sintese_bimestral(
            &[
                inst("Atividade 1", Atividade, 9.0, 1.0),
                inst("Atividade 2", Atividade, 9.0, 1.0),
                inst("Caderno", Caderno, 9.0, 1.0),
                inst("Tarefa SP", Plataforma, 1.0, 1.0),
            ],
            &RegrasAvaliacao::default(),
        )
        .unwrap();
        // Só a Tarefa SP acusa: as outras estão a 2,67 da média das demais.
        assert_eq!(
            s.alertas,
            vec![AlertaDesempenho::Discrepancia {
                instrumento: "Tarefa SP".into(),
                nota: 1.0,
                media_dos_outros: 9.0,
            }]
        );
        assert_eq!(
            s.alertas[0].mensagem(),
            "\"Tarefa SP\" teve nota 1, longe da média 9 dos outros instrumentos."
        );
    }

    #[test]
    fn poucos_instrumentos_e_sem_discrepancia_mutua() {
        let s = sintese_bimestral(
            &[
                inst("Atividade", Atividade, 10.0, 1.0),
                inst("Caderno", Caderno, 2.0, 1.0),
                InstrumentoAvaliacao::novo("Participação", Participacao, None, 1.0),
            ],
            &RegrasAvaliacao::default(),
        )
        .unwrap();
        assert_eq!(
            s.alertas,
            vec![AlertaDesempenho::SemInstrumentosSuficientes {
                quantidade: 2,
                minimo: 3,
            }]
        );
        assert_eq!(
            s.alertas[0].mensagem(),
            "Só 2 instrumentos com nota; o mínimo configurado é 3."
        );
        assert_eq!(
            s.detalhamento[2].observacao.as_deref(),
            Some("sem nota: fora do cálculo")
        );
        assert_eq!(s.nota, 6);
    }

    #[test]
    fn arredondamento_meio_para_cima_e_configuravel() {
        assert_eq!(Arredondamento::MaisProximo.aplicar(4.5), 5);
        assert_eq!(Arredondamento::MaisProximo.aplicar(4.49), 4);
        assert_eq!(Arredondamento::MaisProximo.aplicar(4.499_999_999_99), 5);
        assert_eq!(Arredondamento::ParaBaixo.aplicar(4.9), 4);
        assert_eq!(Arredondamento::ParaCima.aplicar(4.1), 5);
        assert_eq!(Arredondamento::ParaCima.aplicar(10.0), 10);
        let s = sintese_bimestral(
            &[
                inst("A", Atividade, 4.0, 1.0),
                inst("B", Caderno, 5.0, 1.0),
                inst("C", Participacao, 4.5, 1.0),
            ],
            &RegrasAvaliacao::default(),
        )
        .unwrap();
        assert_eq!(s.nota, 5);
    }

    #[test]
    fn entradas_invalidas_sao_recusadas() {
        let cfg = RegrasAvaliacao::default();
        assert!(matches!(
            sintese_bimestral(&[inst("A", Atividade, 11.0, 1.0)], &cfg),
            Err(CoreError::ForaDoIntervalo {
                campo: "instrumento.nota",
                ..
            })
        ));
        assert!(matches!(
            sintese_bimestral(&[inst("A", Atividade, 5.0, 0.0)], &cfg),
            Err(CoreError::ForaDoIntervalo {
                campo: "instrumento.peso",
                ..
            })
        ));
        assert_eq!(
            sintese_bimestral(&[inst("A", Outro(" ".into()), 5.0, 1.0)], &cfg),
            Err(CoreError::IdentificadorAusente("instrumento.tipo"))
        );
        assert_eq!(
            sintese_bimestral(
                &[
                    InstrumentoAvaliacao::novo("A", Atividade, None, 1.0),
                    inst("Rec", Recuperacao, 9.0, 1.0),
                ],
                &cfg
            ),
            Err(CoreError::SemNotaLancada)
        );
        let teto_errado = RegrasAvaliacao {
            teto_prova_paulista: 1.5,
            ..RegrasAvaliacao::default()
        };
        assert!(teto_errado.validar().is_err());
    }

    #[test]
    fn discrepancia_de_turma_abaixo_da_referencia() {
        let alerta = discrepancia_turma(&[3, 4, 4, 5, 3, 2, 6], &[6, 7, 6, 5, 7, 8, 6]).unwrap();
        assert_eq!(alerta.direcao, DirecaoDiscrepancia::AbaixoDaReferencia);
        assert!(perto(alerta.media_turma, 27.0 / 7.0));
        assert!(perto(alerta.percentual_abaixo_turma, 500.0 / 7.0));
        assert!(perto(alerta.percentual_abaixo_referencia, 0.0));
        assert!(alerta.mensagem.starts_with("Média da turma 3,9 contra 6,4"));
        assert!(alerta
            .o_que_fazer
            .contains("recuperação antes do fechamento"));
    }

    #[test]
    fn discrepancia_de_turma_acima_parecida_e_pequena() {
        let acima = discrepancia_turma(&[9, 10, 9, 9, 10], &[6, 5, 4, 7, 6, 5]).unwrap();
        assert_eq!(acima.direcao, DirecaoDiscrepancia::AcimaDaReferencia);
        assert!(discrepancia_turma(&[6, 7, 5, 6, 7], &[6, 6, 7, 5, 6]).is_none());
        // Menos de 5 notas: média instável, sem alerta.
        assert!(discrepancia_turma(&[1, 1, 1], &[9, 9, 9, 9, 9]).is_none());
        let limites = LimitesTurma {
            minimo_notas: 3,
            ..LimitesTurma::default()
        };
        assert!(
            discrepancia_turma_com_limites(&[1, 1, 1], &[9, 9, 9], &limites)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn frequencia_exatamente_75_por_cento_nao_retem() {
        let f = freq(16, 4);
        assert!(perto(f.percentual, 75.0));
        assert_eq!(f.faltas_maximas_permitidas, 4);
        assert_eq!(f.faltas_acima_do_limite, 0);
        assert_eq!(f.aulas_a_compensar, 0);
        assert_eq!(f.risco, RiscoFrequencia::Atencao);
        assert_eq!(freq(16, 5).risco, RiscoFrequencia::Retencao);
    }

    #[test]
    fn frequencia_abaixo_com_aulas_a_compensar() {
        let f = freq(40, 14);
        assert!(perto(f.percentual, 65.0));
        assert_eq!(f.faltas_maximas_permitidas, 10);
        assert_eq!(f.faltas_acima_do_limite, 4);
        assert_eq!(f.aulas_a_compensar, 4);
        assert_eq!(f.risco, RiscoFrequencia::Retencao);
        // Premissa: cada aula compensada abona uma falta -> 10 faltas em 40 = 75%.
        assert_eq!(
            freq(40, 14 - f.aulas_a_compensar).risco,
            RiscoFrequencia::Atencao
        );
    }

    #[test]
    fn frequencia_ok_sem_aulas_e_entrada_invalida() {
        assert_eq!(freq(40, 2).risco, RiscoFrequencia::Ok);
        assert_eq!(freq(40, 8).risco, RiscoFrequencia::Ok); // 80% exatos
        let vazia = freq(0, 0);
        assert!(perto(vazia.percentual, 100.0));
        assert_eq!(vazia.risco, RiscoFrequencia::Ok);
        assert_eq!(
            situacao_frequencia(3, 4, &RegrasFrequencia::default()),
            Err(CoreError::FaltasAcimaDasAulas {
                faltas: 4,
                aulas: 3
            })
        );
        let invertida = RegrasFrequencia {
            minimo_percentual: 90,
            atencao_percentual: 80,
        };
        assert!(situacao_frequencia(10, 1, &invertida).is_err());
    }

    #[test]
    fn risco_final_nota_necessaria_possivel() {
        let r = risco_final(
            &[Some(4), Some(5), None, None],
            &freq(40, 2),
            &RegrasAvaliacao::default(),
        )
        .unwrap();
        assert!(perto(r.media_parcial.unwrap(), 4.5));
        assert_eq!(r.bimestres_restantes, 2);
        // (20 - 9) / 2 = 5,5 -> 6 em cada um.
        assert_eq!(r.nota_necessaria_nos_bimestres_restantes, Some(6));
        assert!(r.por_nota && !r.por_falta);
        assert!(r
            .mensagem
            .contains("precisa de pelo menos 6 em cada um dos 2 bimestres restantes"));
    }

    #[test]
    fn risco_final_impossivel_e_por_falta() {
        let r = risco_final(
            &[Some(1), Some(1), Some(2), None],
            &freq(40, 14),
            &RegrasAvaliacao::default(),
        )
        .unwrap();
        assert_eq!(r.nota_necessaria_nos_bimestres_restantes, None);
        assert!(r.por_nota && r.por_falta);
        assert!(r.mensagem.contains("mesmo com 10"));
        assert!(r.mensagem.contains("Conselho de Classe"));
        assert!(r
            .mensagem
            .contains("abaixo do mínimo de 75% (LDB, art. 24, VI); 4 faltas"));
        assert!(r.mensagem.contains("confirmar o procedimento com a escola"));
    }

    #[test]
    fn risco_final_ja_garantido_fechado_e_invalido() {
        let cfg = RegrasAvaliacao::default();
        let r = risco_final(&[Some(8), Some(8), Some(6), None], &freq(40, 0), &cfg).unwrap();
        assert_eq!(r.nota_necessaria_nos_bimestres_restantes, Some(0));
        assert!(!r.por_nota);
        assert!(r.mensagem.contains("já está garantida"));
        let fechado =
            risco_final(&[Some(4), Some(4), Some(5), Some(4)], &freq(40, 0), &cfg).unwrap();
        assert_eq!(fechado.nota_necessaria_nos_bimestres_restantes, None);
        assert!(fechado.por_nota);
        let sem_nota = risco_final(&[], &freq(0, 0), &cfg).unwrap();
        assert_eq!(sem_nota.nota_necessaria_nos_bimestres_restantes, Some(5));
        assert!(!sem_nota.por_nota);
        assert_eq!(
            risco_final(&[None; 5], &freq(0, 0), &cfg),
            Err(CoreError::BimestresDemais {
                informados: 5,
                maximo: 4
            })
        );
        assert!(risco_final(&[Some(11)], &freq(0, 0), &cfg).is_err());
    }

    #[test]
    fn classifica_deferido_com_janela() {
        let texto = "Prezado(a) professor(a), o pedido de alteração de notas do 3º bimestre, \
                     protocolado em 05/03/2030, foi DEFERIDO. O Diário de Classe ficará liberado \
                     de 18/03/2030 a 22/03/2030 para o lançamento.";
        let r = classificar_resposta(texto);
        assert_eq!(
            r,
            RespostaPedido::Deferido {
                liberacao_inicio: Some("2030-03-18".into()),
                liberacao_fim: Some("2030-03-22".into()),
                instrucoes: sanitizar_detalhe(texto),
            }
        );
        assert!(r.validar().is_ok());
        assert_eq!(r.dias_da_janela(), Some(5));
        let passo = proximo_passo(&r);
        assert!(passo.starts_with(
            "Lançar as notas no Diário entre 18/03/2030 e 22/03/2030 (5 dias, contando o primeiro e o último)"
        ));
    }

    #[test]
    fn classifica_deferido_so_com_prazo_final() {
        let r = classificar_resposta("Pedido autorizado. Lançar as notas até o dia 20/11/2026.");
        match &r {
            RespostaPedido::Deferido {
                liberacao_inicio,
                liberacao_fim,
                ..
            } => {
                assert_eq!(liberacao_inicio, &None);
                assert_eq!(liberacao_fim.as_deref(), Some("2026-11-20"));
            }
            outro => panic!("esperado Deferido, veio {outro:?}"),
        }
        assert!(proximo_passo(&r).starts_with("Lançar as notas no Diário até 20/11/2026"));
    }

    #[test]
    fn classifica_indeferido_e_oculta_numero_longo() {
        let r = classificar_resposta(
            "Informamos que a solicitação foi indeferida, pois o prazo de fechamento do bimestre \
             já se encerrou. Aluno RA 000123456789.",
        );
        let RespostaPedido::Indeferido { motivo } = &r else {
            panic!("esperado Indeferido, veio {r:?}");
        };
        assert!(motivo.contains("prazo de fechamento"));
        assert!(!motivo.contains("000123456789"));
        let passo = proximo_passo(&r);
        assert!(passo.contains("Resolução SE 62/2019, art. 1º, parágrafo único"));
        assert!(passo.contains("art. 7º"));
        assert!(passo.contains("recurso ao Conselho de Classe com a recuperação documentada"));
        // Negação de palavra de deferimento também é indeferimento.
        assert!(matches!(
            classificar_resposta("O pedido não foi autorizado pela Diretoria de Ensino."),
            RespostaPedido::Indeferido { .. }
        ));
        assert!(matches!(
            classificar_resposta("Não será possível reabrir o diário neste bimestre."),
            RespostaPedido::Indeferido { .. }
        ));
    }

    #[test]
    fn classifica_pedido_de_informacao() {
        let r = classificar_resposta(
            "Para análise do pedido, solicitamos que apresente a ata do Conselho de Classe e o \
             registro da atividade de recuperação aplicada. Agradecemos a atenção.",
        );
        assert_eq!(
            r,
            RespostaPedido::PedeInformacao {
                o_que: "Para análise do pedido, solicitamos que apresente a ata do Conselho de \
                        Classe e o registro da atividade de recuperação aplicada"
                    .into(),
            }
        );
        assert!(proximo_passo(&r).starts_with("Juntar e enviar o que foi pedido: Para análise"));
        // Liberação condicionada à informação não é deferimento.
        assert!(matches!(
            classificar_resposta("Encaminhe a ata para que o diário seja liberado."),
            RespostaPedido::PedeInformacao { .. }
        ));
    }

    #[test]
    fn classifica_pendente() {
        let r = classificar_resposta(
            "Recebemos sua solicitação, que foi encaminhada à Diretoria de Ensino e está em \
             análise. Aguarde o retorno.",
        );
        assert_eq!(r, RespostaPedido::Pendente);
        assert!(proximo_passo(&r).contains("protocolo"));
        assert_eq!(classificar_resposta(""), RespostaPedido::Pendente);
    }

    #[test]
    fn resposta_serializa_com_etiqueta_e_valida_janela() {
        let r = RespostaPedido::Deferido {
            liberacao_inicio: Some("2026-11-21".into()),
            liberacao_fim: Some("2026-11-17".into()),
            instrucoes: "x".into(),
        };
        assert!(r.validar().is_err());
        let json = serde_json::to_value(&RespostaPedido::Pendente).unwrap();
        assert_eq!(json, serde_json::json!({"resposta": "PENDENTE"}));
        let volta: RespostaPedido = serde_json::from_value(serde_json::json!({
            "resposta": "INDEFERIDO", "motivo": "fora do prazo"
        }))
        .unwrap();
        assert_eq!(
            volta,
            RespostaPedido::Indeferido {
                motivo: "fora do prazo".into()
            }
        );
    }
}
