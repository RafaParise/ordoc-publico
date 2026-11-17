//! Motor de alertas locais (`alert-rules` no regras do projeto): cruza a frequência do Diário de Classe com
//! as entregas da Sala do Futuro usando só o banco local (funciona sem internet, dentro da sala).
//!
//! A regra principal é a do contrato: aluno com >= 20% de faltas E nenhuma entrega na Sala do Futuro.
//! Duas regras parciais (só faltas, só entregas) dão o aviso mais cedo, com severidade menor, e somem
//! quando a principal dispara para o mesmo aluno (para o professor não receber três avisos iguais).

use std::collections::HashSet;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::modelos::{IndicadorRisco, Severidade};

/// Identificadores estáveis das regras (gravados no banco: não mudar depois de publicados).
pub const REGRA_FALTAS_SEM_ENTREGA: &str = "faltas_sem_entrega";
pub const REGRA_FREQUENCIA_BAIXA: &str = "frequencia_baixa";
pub const REGRA_SEM_ENTREGAS: &str = "sem_entregas";

/// Números de um aluno numa turma, calculados pelo armazenamento local.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumoAluno {
    pub turma_id: String,
    pub aluno_ra: String,
    /// Aulas com frequência registrada para o aluno, já com o peso (aula dupla = 2).
    pub aulas_registradas: u32,
    /// Faltas sem justificativa, com o mesmo peso.
    pub faltas: u32,
    pub faltas_justificadas: u32,
    /// Atividades da Sala do Futuro que já podem ser cobradas deste aluno: prazo vencido, sem prazo
    /// ou já entregues. Atividade ainda no prazo e não entregue NÃO entra (senão toda atividade
    /// recém-publicada geraria alerta para a turma inteira).
    pub atividades_cobradas: u32,
    /// Quantas das cobradas o aluno entregou.
    pub atividades_entregues: u32,
}

impl ResumoAluno {
    fn faltas_consideradas(&self, contar_justificadas: bool) -> u32 {
        if contar_justificadas {
            self.faltas.saturating_add(self.faltas_justificadas)
        } else {
            self.faltas
        }
    }

    /// Percentual de faltas (0 enquanto não há aula registrada).
    pub fn percentual_faltas(&self, contar_justificadas: bool) -> f64 {
        if self.aulas_registradas == 0 {
            return 0.0;
        }
        f64::from(self.faltas_consideradas(contar_justificadas)) * 100.0
            / f64::from(self.aulas_registradas)
    }

    /// `faltas / aulas >= percentual / 100`, feito em inteiros: o limite exato (ex.: 1 falta em
    /// 5 aulas = 20%) dispara sem depender de arredondamento de ponto flutuante.
    pub fn atingiu_percentual_faltas(&self, percentual: u32, contar_justificadas: bool) -> bool {
        self.aulas_registradas > 0
            && u64::from(self.faltas_consideradas(contar_justificadas)) * 100
                >= u64::from(percentual) * u64::from(self.aulas_registradas)
    }

    /// Há atividade cobrável e nenhuma foi entregue.
    pub fn sem_nenhuma_entrega(&self) -> bool {
        self.atividades_cobradas > 0 && self.atividades_entregues == 0
    }
}

/// Limites das regras de frequência.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LimitesAlerta {
    /// Percentual mínimo de faltas (regras do projeto: 20).
    pub percentual_faltas: u32,
    /// Aulas registradas antes de avaliar a frequência. O padrão 1 segue o contrato ao pé da letra;
    /// um valor maior evita alerta no começo do bimestre (1 falta em 1 aula = 100%).
    pub aulas_minimas: u32,
    /// Se a falta justificada entra no percentual (padrão: não entra).
    pub contar_justificadas: bool,
}

impl Default for LimitesAlerta {
    fn default() -> Self {
        Self {
            percentual_faltas: 20,
            aulas_minimas: 1,
            contar_justificadas: false,
        }
    }
}

impl LimitesAlerta {
    fn frequencia_critica(&self, r: &ResumoAluno) -> bool {
        r.aulas_registradas >= self.aulas_minimas.max(1)
            && r.atingiu_percentual_faltas(self.percentual_faltas, self.contar_justificadas)
    }
}

/// Uma regra do motor. Implemente para criar regras novas.
pub trait RegraAlerta: Send + Sync {
    /// Identificador estável, gravado no banco.
    fn id(&self) -> &'static str;

    /// Avalia um aluno. O motor preenche `regra`, `turma_id` e `aluno_ra` do indicador devolvido.
    fn avaliar(&self, resumo: &ResumoAluno) -> Option<IndicadorRisco>;

    /// Regras que esta torna redundantes para o mesmo aluno e turma.
    fn substitui(&self) -> &'static [&'static str] {
        &[]
    }
}

/// Percentual com vírgula decimal, como o professor lê: `25,0`.
fn percentual_br(valor: f64) -> String {
    format!("{valor:.1}").replace('.', ",")
}

/// Trecho da mensagem sobre entregas, em minúsculas (para continuar uma frase).
fn texto_entregas(cobradas: u32) -> String {
    if cobradas == 1 {
        "a atividade da Sala do Futuro não foi entregue".to_string()
    } else {
        format!("nenhuma das {cobradas} atividades da Sala do Futuro foi entregue")
    }
}

fn maiuscula_inicial(texto: &str) -> String {
    let mut letras = texto.chars();
    match letras.next() {
        Some(primeira) => primeira.to_uppercase().chain(letras).collect(),
        None => String::new(),
    }
}

fn evidencias_frequencia(r: &ResumoAluno, l: &LimitesAlerta) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("aulas_registradas".into(), json!(r.aulas_registradas));
    m.insert("faltas".into(), json!(r.faltas));
    m.insert("faltas_justificadas".into(), json!(r.faltas_justificadas));
    m.insert("contar_justificadas".into(), json!(l.contar_justificadas));
    m.insert(
        "percentual_faltas".into(),
        json!((r.percentual_faltas(l.contar_justificadas) * 10.0).round() / 10.0),
    );
    m.insert("limite_percentual".into(), json!(l.percentual_faltas));
    m
}

fn indicador(
    severidade: Severidade,
    r: &ResumoAluno,
    mensagem: String,
    evidencias: Value,
) -> IndicadorRisco {
    IndicadorRisco {
        regra: String::new(),
        severidade,
        turma_id: r.turma_id.clone(),
        aluno_ra: r.aluno_ra.clone(),
        mensagem,
        evidencias,
    }
}

/// Regra do contrato: >= `percentual_faltas`% de faltas no DC e nenhuma entrega na Sala do Futuro.
#[derive(Debug, Clone, Copy, Default)]
pub struct RegraFaltasSemEntrega(pub LimitesAlerta);

impl RegraAlerta for RegraFaltasSemEntrega {
    fn id(&self) -> &'static str {
        REGRA_FALTAS_SEM_ENTREGA
    }

    fn avaliar(&self, r: &ResumoAluno) -> Option<IndicadorRisco> {
        let l = &self.0;
        if !(l.frequencia_critica(r) && r.sem_nenhuma_entrega()) {
            return None;
        }
        let mut evidencias = evidencias_frequencia(r, l);
        evidencias.insert("atividades_cobradas".into(), json!(r.atividades_cobradas));
        evidencias.insert("atividades_entregues".into(), json!(r.atividades_entregues));
        let mensagem = format!(
            "{}% de faltas ({} de {} aulas) e {}.",
            percentual_br(r.percentual_faltas(l.contar_justificadas)),
            r.faltas_consideradas(l.contar_justificadas),
            r.aulas_registradas,
            texto_entregas(r.atividades_cobradas)
        );
        Some(indicador(
            Severidade::Critico,
            r,
            mensagem,
            Value::Object(evidencias),
        ))
    }

    fn substitui(&self) -> &'static [&'static str] {
        &[REGRA_FREQUENCIA_BAIXA, REGRA_SEM_ENTREGAS]
    }
}

/// Só a frequência passou do limite.
#[derive(Debug, Clone, Copy, Default)]
pub struct RegraFrequenciaBaixa(pub LimitesAlerta);

impl RegraAlerta for RegraFrequenciaBaixa {
    fn id(&self) -> &'static str {
        REGRA_FREQUENCIA_BAIXA
    }

    fn avaliar(&self, r: &ResumoAluno) -> Option<IndicadorRisco> {
        let l = &self.0;
        if !l.frequencia_critica(r) {
            return None;
        }
        let mensagem = format!(
            "{}% de faltas ({} de {} aulas).",
            percentual_br(r.percentual_faltas(l.contar_justificadas)),
            r.faltas_consideradas(l.contar_justificadas),
            r.aulas_registradas
        );
        Some(indicador(
            Severidade::Atencao,
            r,
            mensagem,
            Value::Object(evidencias_frequencia(r, l)),
        ))
    }
}

/// Só as entregas: há atividade cobrável e nenhuma foi entregue.
#[derive(Debug, Clone, Copy, Default)]
pub struct RegraSemEntregas;

impl RegraAlerta for RegraSemEntregas {
    fn id(&self) -> &'static str {
        REGRA_SEM_ENTREGAS
    }

    fn avaliar(&self, r: &ResumoAluno) -> Option<IndicadorRisco> {
        if !r.sem_nenhuma_entrega() {
            return None;
        }
        let evidencias = json!({
            "atividades_cobradas": r.atividades_cobradas,
            "atividades_entregues": r.atividades_entregues,
        });
        let mensagem = format!(
            "{}.",
            maiuscula_inicial(&texto_entregas(r.atividades_cobradas))
        );
        Some(indicador(Severidade::Atencao, r, mensagem, evidencias))
    }
}

/// O motor: aplica as regras a cada aluno e devolve os indicadores de risco.
pub struct MotorAlertas {
    regras: Vec<Box<dyn RegraAlerta>>,
}

impl fmt::Debug for MotorAlertas {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MotorAlertas")
            .field("regras", &self.ids_das_regras())
            .finish()
    }
}

impl Default for MotorAlertas {
    fn default() -> Self {
        Self::padrao()
    }
}

impl MotorAlertas {
    /// Motor sem nenhuma regra.
    pub fn vazio() -> Self {
        Self { regras: Vec::new() }
    }

    /// As três regras com os limites do contrato (20% de faltas).
    pub fn padrao() -> Self {
        Self::com_limites(LimitesAlerta::default())
    }

    pub fn com_limites(limites: LimitesAlerta) -> Self {
        let mut motor = Self::vazio();
        motor
            .adicionar(Box::new(RegraFaltasSemEntrega(limites)))
            .adicionar(Box::new(RegraFrequenciaBaixa(limites)))
            .adicionar(Box::new(RegraSemEntregas));
        motor
    }

    pub fn adicionar(&mut self, regra: Box<dyn RegraAlerta>) -> &mut Self {
        self.regras.push(regra);
        self
    }

    pub fn ids_das_regras(&self) -> Vec<&'static str> {
        self.regras.iter().map(|r| r.id()).collect()
    }

    /// Avalia um aluno: cada regra dispara no máximo uma vez, e a regra que substitui outras
    /// esconde as parciais.
    pub fn avaliar_aluno(&self, resumo: &ResumoAluno) -> Vec<IndicadorRisco> {
        let mut vistas = HashSet::new();
        let mut disparos: Vec<(&dyn RegraAlerta, IndicadorRisco)> = Vec::new();
        for regra in &self.regras {
            if !vistas.insert(regra.id()) {
                continue; // a mesma regra adicionada duas vezes vale uma
            }
            if let Some(mut ind) = regra.avaliar(resumo) {
                // O motor garante a chave do indicador, mesmo numa regra escrita com descuido.
                ind.regra = regra.id().to_string();
                ind.turma_id = resumo.turma_id.clone();
                ind.aluno_ra = resumo.aluno_ra.clone();
                disparos.push((regra.as_ref(), ind));
            }
        }
        let substituidas: HashSet<&str> = disparos
            .iter()
            .flat_map(|(regra, _)| regra.substitui().iter().copied())
            .collect();
        disparos
            .into_iter()
            .filter(|(_, ind)| !substituidas.contains(ind.regra.as_str()))
            .map(|(_, ind)| ind)
            .collect()
    }

    /// Avalia vários alunos. Ordem estável: crítico primeiro, depois turma, RA e regra.
    pub fn avaliar(&self, resumos: &[ResumoAluno]) -> Vec<IndicadorRisco> {
        let mut todos: Vec<IndicadorRisco> =
            resumos.iter().flat_map(|r| self.avaliar_aluno(r)).collect();
        todos.sort_by(|a, b| {
            b.severidade
                .cmp(&a.severidade)
                .then_with(|| a.turma_id.cmp(&b.turma_id))
                .then_with(|| a.aluno_ra.cmp(&b.aluno_ra))
                .then_with(|| a.regra.cmp(&b.regra))
        });
        todos
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    fn resumo(ra: &str, aulas: u32, faltas: u32, cobradas: u32, entregues: u32) -> ResumoAluno {
        ResumoAluno {
            turma_id: "T-3B".into(),
            aluno_ra: ra.into(),
            aulas_registradas: aulas,
            faltas,
            faltas_justificadas: 0,
            atividades_cobradas: cobradas,
            atividades_entregues: entregues,
        }
    }

    fn regras(ind: &[IndicadorRisco]) -> Vec<&str> {
        ind.iter().map(|i| i.regra.as_str()).collect()
    }

    #[test]
    fn regra_do_contrato_dispara_no_limite_exato_de_20_por_cento() {
        let motor = MotorAlertas::padrao();
        // 1 falta em 5 aulas = 20% exatos, 0 de 3 atividades cobradas.
        let ind = motor.avaliar_aluno(&resumo("000000001", 5, 1, 3, 0));
        assert_eq!(regras(&ind), vec![REGRA_FALTAS_SEM_ENTREGA]);
        assert_eq!(ind[0].severidade, Severidade::Critico);
        assert_eq!(ind[0].aluno_ra, "000000001");
        assert_eq!(
            ind[0].mensagem,
            "20,0% de faltas (1 de 5 aulas) e nenhuma das 3 atividades da Sala do Futuro foi entregue."
        );
        assert!(
            !ind[0].mensagem.contains("000000001"),
            "a mensagem não leva o RA"
        );
        assert_eq!(ind[0].evidencias["percentual_faltas"], 20.0);
        assert_eq!(ind[0].evidencias["atividades_cobradas"], 3);
    }

    #[test]
    fn abaixo_do_limite_nao_dispara_frequencia() {
        let motor = MotorAlertas::padrao();
        // 19 faltas em 100 aulas = 19%: só sobra o aviso de entregas.
        let ind = motor.avaliar_aluno(&resumo("000000002", 100, 19, 2, 0));
        assert_eq!(regras(&ind), vec![REGRA_SEM_ENTREGAS]);
        assert_eq!(ind[0].severidade, Severidade::Atencao);
        assert_eq!(
            ind[0].mensagem,
            "Nenhuma das 2 atividades da Sala do Futuro foi entregue."
        );
    }

    #[test]
    fn faltas_altas_com_entrega_feita_e_so_atencao() {
        let motor = MotorAlertas::padrao();
        let ind = motor.avaliar_aluno(&resumo("000000003", 10, 5, 4, 1));
        assert_eq!(regras(&ind), vec![REGRA_FREQUENCIA_BAIXA]);
        assert_eq!(ind[0].mensagem, "50,0% de faltas (5 de 10 aulas).");
    }

    #[test]
    fn atividade_no_prazo_ou_sem_dados_nao_conta_como_falta_de_entrega() {
        let motor = MotorAlertas::padrao();
        // Nenhuma atividade cobrável (todas no prazo, ou nenhuma situação conhecida da SDF):
        // só a frequência pesa, e nunca sai CRITICO por "0 entregas".
        let ind = motor.avaliar_aluno(&resumo("000000004", 10, 3, 0, 0));
        assert_eq!(regras(&ind), vec![REGRA_FREQUENCIA_BAIXA]);
        assert_eq!(ind[0].severidade, Severidade::Atencao);
    }

    #[test]
    fn sem_aula_registrada_nao_divide_por_zero() {
        let r = resumo("000000005", 0, 0, 0, 0);
        assert_eq!(r.percentual_faltas(false), 0.0);
        assert!(!r.atingiu_percentual_faltas(20, false));
        assert!(MotorAlertas::padrao().avaliar_aluno(&r).is_empty());
    }

    #[test]
    fn justificadas_so_contam_quando_configurado() {
        let mut r = resumo("000000006", 10, 1, 1, 0);
        r.faltas_justificadas = 2;
        // Padrão: 1 de 10 = 10%, abaixo do limite.
        let ind = MotorAlertas::padrao().avaliar_aluno(&r);
        assert_eq!(regras(&ind), vec![REGRA_SEM_ENTREGAS]);
        assert_eq!(
            ind[0].mensagem,
            "A atividade da Sala do Futuro não foi entregue."
        );
        // Contando as justificadas: 3 de 10 = 30%.
        let motor = MotorAlertas::com_limites(LimitesAlerta {
            contar_justificadas: true,
            ..LimitesAlerta::default()
        });
        let ind = motor.avaliar_aluno(&r);
        assert_eq!(regras(&ind), vec![REGRA_FALTAS_SEM_ENTREGA]);
        assert_eq!(
            ind[0].mensagem,
            "30,0% de faltas (3 de 10 aulas) e a atividade da Sala do Futuro não foi entregue."
        );
    }

    #[test]
    fn aulas_minimas_seguram_o_alerta_no_comeco() {
        let motor = MotorAlertas::com_limites(LimitesAlerta {
            aulas_minimas: 4,
            ..LimitesAlerta::default()
        });
        assert!(motor
            .avaliar_aluno(&resumo("000000007", 3, 3, 0, 0))
            .is_empty());
        assert_eq!(
            regras(&motor.avaliar_aluno(&resumo("000000007", 4, 1, 0, 0))),
            vec![REGRA_FREQUENCIA_BAIXA]
        );
    }

    struct RegraDescuidada;

    impl RegraAlerta for RegraDescuidada {
        fn id(&self) -> &'static str {
            "descuidada"
        }
        fn avaliar(&self, _r: &ResumoAluno) -> Option<IndicadorRisco> {
            Some(IndicadorRisco {
                regra: "outro_nome".into(),
                severidade: Severidade::Atencao,
                turma_id: "turma_errada".into(),
                aluno_ra: "ra_errado".into(),
                mensagem: "teste".into(),
                evidencias: json!({}),
            })
        }
    }

    #[test]
    fn motor_garante_a_chave_e_ignora_regra_repetida() {
        let mut motor = MotorAlertas::vazio();
        motor
            .adicionar(Box::new(RegraDescuidada))
            .adicionar(Box::new(RegraDescuidada));
        let ind = motor.avaliar_aluno(&resumo("000000008", 1, 0, 0, 0));
        assert_eq!(ind.len(), 1);
        assert_eq!(ind[0].regra, "descuidada");
        assert_eq!(ind[0].turma_id, "T-3B");
        assert_eq!(ind[0].aluno_ra, "000000008");
        assert_eq!(
            format!("{motor:?}"),
            r#"MotorAlertas { regras: ["descuidada", "descuidada"] }"#
        );
    }

    #[test]
    fn avaliacao_da_turma_em_ordem_estavel() {
        let motor = MotorAlertas::padrao();
        let resumos = vec![
            resumo("000000020", 10, 0, 2, 0), // só entregas (atenção)
            resumo("000000010", 10, 4, 2, 0), // crítico
            resumo("000000030", 10, 0, 2, 2), // em dia
            resumo("000000005", 10, 3, 2, 1), // só frequência (atenção)
        ];
        let ind = motor.avaliar(&resumos);
        let chaves: Vec<(&str, &str)> = ind
            .iter()
            .map(|i| (i.aluno_ra.as_str(), i.regra.as_str()))
            .collect();
        assert_eq!(
            chaves,
            vec![
                ("000000010", REGRA_FALTAS_SEM_ENTREGA),
                ("000000005", REGRA_FREQUENCIA_BAIXA),
                ("000000020", REGRA_SEM_ENTREGAS),
            ]
        );
    }
}
