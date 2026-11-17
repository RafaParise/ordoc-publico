//! Situação funcional do professor: o painel do que falta para ele ficar 100% regularizado.
//!
//! Junta num lugar só as pendências de vários sistemas (Professor Presente, Diário da Sala do
//! Futuro, Fala SP, EFAPE, GDAE, atribuição online...), diz o que fazer com cada uma, em que ordem,
//! e o que uma trava na outra (ex.: farol vermelho trava a atribuição online).
//!
//! **Nada aqui escreve em sistema externo.** A [`Acao`] só descreve o próximo passo; quem lança,
//! pede ou cobra é o professor, e toda escrita no Diário passa por confirmação dele, operação por
//! operação (regras do projeto, seção 6).
//!
//! LGPD: título, detalhe e evidências falam do professor e das aulas, nunca de alunos. O título é
//! conferido (número longo, como RA ou CPF, é recusado). O número de protocolo é do próprio
//! professor e pode ser guardado.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::conector::ocultar_numeros_longos;
use crate::error::{CoreError, Result};
use crate::modelos::validar_data_iso;
use crate::presenca_professor::{
    data_de_dia_juliano, dia_juliano, EstadoAulaProfessor, JanelasEdicao, LancamentoDiario,
    PendenciaPresenca, QuemPodeLancar,
};

/// Prazo a esta distância de hoje (em dias, inclusive) ou já vencido sobe para o topo da lista.
pub const LIMIAR_PRAZO_VENCENDO_DIAS: i64 = 7;

/// Sistema de onde a pendência veio, ou por onde se pede algo a um órgão.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SistemaOrigem {
    SedProfessorPresente,
    DiarioSalaDoFuturo,
    FalaSp,
    SouSp,
    Efape,
    Gdae,
    AtribuicaoOnline,
    SuporteSed,
    /// Plataforma Educação Profissional Paulista (Moodle): correção das tarefas.
    PlataformaEp,
    /// Tarefa SP (Gerenciador de Atividades da Sala do Futuro, cmsp.ip.tv).
    TarefaSp,
    /// Canal sem sistema próprio (ex.: contato direto com a gestão da escola).
    Outro(String),
}

/// Assunto da pendência. Junto com a chave natural, forma o `id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AreaPendencia {
    /// Aula que não ficou verde no Professor Presente e que o lançamento no Diário não resolve
    /// (ex.: lançada por outro perfil).
    PresencaAula,
    /// Frequência ou registro de aula faltando no Diário.
    LancamentoDiario,
    /// Dia vermelho: requerimento de abono/justificativa.
    DiaFalta,
    /// Chamado aberto em algum canal, acompanhado até a resposta.
    Protocolo,
    FormacaoEfape,
    Farol,
    /// GDAE, inscrição e atribuição online.
    CadastroAtribuicao,
    /// Holerite, aulas pagas, revisão de pagamento.
    Pagamento,
    /// Envio de estudante esperando correção e nota nas plataformas (Educação Profissional, Tarefa SP).
    AvaliacaoPlataforma,
}

impl AreaPendencia {
    pub const TODAS: [AreaPendencia; 9] = [
        Self::PresencaAula,
        Self::LancamentoDiario,
        Self::DiaFalta,
        Self::Protocolo,
        Self::FormacaoEfape,
        Self::Farol,
        Self::CadastroAtribuicao,
        Self::Pagamento,
        Self::AvaliacaoPlataforma,
    ];

    /// Código curto e estável (entra no `id` da pendência: não mudar).
    pub fn codigo(self) -> &'static str {
        match self {
            Self::PresencaAula => "presenca_aula",
            Self::LancamentoDiario => "lancamento_diario",
            Self::DiaFalta => "dia_falta",
            Self::Protocolo => "protocolo",
            Self::FormacaoEfape => "formacao_efape",
            Self::Farol => "farol",
            Self::CadastroAtribuicao => "cadastro_atribuicao",
            Self::Pagamento => "pagamento",
            Self::AvaliacaoPlataforma => "avaliacao_plataforma",
        }
    }
}

/// O próximo passo, só descrito. Nenhuma variante executa nada.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "tipo", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Acao {
    /// O app só lê e junta provas; não há o que lançar nem pedir ainda.
    LevantarSozinho,
    /// O professor lança (com o conteúdo real), com confirmação dele a cada operação.
    LancarComConfirmacao { prazo: Option<String> },
    /// Só um órgão resolve: o professor pede pelo canal indicado.
    PedirAoOrgao { canal: SistemaOrigem, orgao: String },
    /// Pedido feito: aguardar a resposta do protocolo.
    AguardarOrgao { protocolo: String },
    /// Falta uma informação que só o professor tem.
    PerguntarAoProfessor { pergunta: String },
}

impl Acao {
    pub fn prazo(&self) -> Option<&str> {
        match self {
            Self::LancarComConfirmacao { prazo } => prazo.as_deref(),
            _ => None,
        }
    }

    pub fn validar(&self) -> Result<()> {
        match self {
            Self::LevantarSozinho => Ok(()),
            Self::LancarComConfirmacao { prazo } => {
                prazo.as_deref().map_or(Ok(()), validar_data_iso)
            }
            Self::PedirAoOrgao { canal, orgao } => {
                exigir(orgao, "acao.orgao")?;
                match canal {
                    SistemaOrigem::Outro(nome) => exigir(nome, "acao.canal"),
                    _ => Ok(()),
                }
            }
            Self::AguardarOrgao { protocolo } => exigir(protocolo, "acao.protocolo"),
            Self::PerguntarAoProfessor { pergunta } => exigir(pergunta, "acao.pergunta"),
        }
    }
}

/// Estado de uma pendência no painel.
///
/// Transições válidas: entre `Aberta`, `EmAndamento` e `AguardandoOrgao` em qualquer sentido, e de
/// qualquer uma delas para `Resolvida` ou `Descartada`. Sair de `Resolvida`/`Descartada` só pelo
/// `reabrir` explícito (com motivo). Ir para o mesmo estado não faz nada.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "estado", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EstadoPendencia {
    Aberta,
    EmAndamento,
    AguardandoOrgao,
    Resolvida,
    Descartada { motivo: String },
}

impl EstadoPendencia {
    pub fn codigo(&self) -> &'static str {
        match self {
            Self::Aberta => "ABERTA",
            Self::EmAndamento => "EM_ANDAMENTO",
            Self::AguardandoOrgao => "AGUARDANDO_ORGAO",
            Self::Resolvida => "RESOLVIDA",
            Self::Descartada { .. } => "DESCARTADA",
        }
    }

    /// Resolvida ou descartada: conta como regularizada.
    pub fn encerrada(&self) -> bool {
        matches!(self, Self::Resolvida | Self::Descartada { .. })
    }

    pub fn ativa(&self) -> bool {
        !self.encerrada()
    }

    /// Devolve o novo estado se a transição for válida.
    pub fn transicionar(&self, novo: EstadoPendencia) -> Result<EstadoPendencia> {
        if let Self::Descartada { motivo } = &novo {
            exigir(motivo, "descartada.motivo")?;
        }
        // Mesmo estado: nada muda. Estado ativo: vai para qualquer outro. Encerrado: só `reabrir`.
        if *self == novo || self.ativa() {
            return Ok(novo);
        }
        Err(CoreError::TransicaoInvalida {
            de: self.codigo(),
            para: novo.codigo(),
        })
    }

    fn ordem_na_agenda(&self) -> u8 {
        match self {
            Self::Aberta | Self::EmAndamento => 0,
            _ => 1,
        }
    }
}

/// Chamado aberto pelo professor em algum canal (o número é dele, pode ser guardado).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Protocolo {
    pub canal: SistemaOrigem,
    pub numero: String,
    /// AAAA-MM-DD.
    pub aberto_em: String,
    pub assunto: String,
    /// Situação como o canal mostra (texto livre: "Em análise", "Respondido"...).
    pub situacao: String,
    /// AAAA-MM-DD: depois disso, cobrar.
    pub prazo_resposta: Option<String>,
}

impl Protocolo {
    pub fn validar(&self) -> Result<()> {
        exigir(&self.numero, "protocolo.numero")?;
        exigir(&self.assunto, "protocolo.assunto")?;
        validar_data_iso(&self.aberto_em)?;
        self.prazo_resposta
            .as_deref()
            .map_or(Ok(()), validar_data_iso)
    }
}

/// Uma coisa que falta para o professor ficar regularizado.
///
/// A prioridade não é guardada: depende do dia de hoje e das outras pendências, e é calculada em
/// [`PainelSituacao::proximas_acoes`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pendencia {
    /// Estável: derivado de `area` + chave natural (ver [`gerar_id`]).
    pub id: String,
    pub area: AreaPendencia,
    pub origem: SistemaOrigem,
    /// Sem dado pessoal (número longo é recusado).
    pub titulo: String,
    #[serde(default)]
    pub detalhe: String,
    #[serde(default)]
    pub evidencias: Vec<String>,
    pub acao: Acao,
    pub estado: EstadoPendencia,
    /// Prazo da própria pendência (AAAA-MM-DD), além do que a ação ou o protocolo trazem.
    #[serde(default)]
    pub prazo: Option<String>,
    /// Ids das pendências que só andam depois desta.
    #[serde(default)]
    pub bloqueia: Vec<String>,
    #[serde(default)]
    pub protocolos: Vec<Protocolo>,
    /// Dado bruto de onde a pendência saiu (regra 4: nada é descartado).
    #[serde(default)]
    pub metadata_bruta: Value,
}

/// Id estável: `<código da área>-<FNV-1a 64 de área + chave natural>`. FNV foi escolhido por ser
/// fixo entre versões do Rust (o `DefaultHasher` não é). Não é sigilo: a chave natural não deve
/// conter dado pessoal.
pub fn gerar_id(area: AreaPendencia, chave_natural: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let entrada = format!("{}\u{1f}{}", area.codigo(), chave_natural.trim());
    for b in entrada.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{}-{h:016x}", area.codigo())
}

fn exigir(valor: &str, campo: &'static str) -> Result<()> {
    if valor.trim().is_empty() {
        Err(CoreError::IdentificadorAusente(campo))
    } else {
        Ok(())
    }
}

/// Junta `nova` em `atual` sem perder nada: objetos são mesclados chave a chave (a nova versão
/// vence no conflito, chaves antigas ficam); `null` na nova mantém a antiga.
fn mesclar_json(atual: &mut Value, nova: Value) {
    match (atual, nova) {
        (_, Value::Null) => {}
        (Value::Object(a), Value::Object(n)) => {
            for (k, v) in n {
                match a.get_mut(&k) {
                    Some(existente) => mesclar_json(existente, v),
                    None => {
                        a.insert(k, v);
                    }
                }
            }
        }
        (a, n) => *a = n,
    }
}

fn unir<T: PartialEq>(destino: &mut Vec<T>, novos: Vec<T>) {
    for item in novos {
        if !destino.contains(&item) {
            destino.push(item);
        }
    }
}

impl Pendencia {
    /// Pendência nova (`Aberta`), com o `id` derivado de `area` + `chave_natural`.
    pub fn nova(
        area: AreaPendencia,
        chave_natural: &str,
        origem: SistemaOrigem,
        titulo: impl Into<String>,
        acao: Acao,
    ) -> Result<Self> {
        exigir(chave_natural, "pendencia.chave_natural")?;
        let p = Self {
            id: gerar_id(area, chave_natural),
            area,
            origem,
            titulo: titulo.into(),
            detalhe: String::new(),
            evidencias: Vec::new(),
            acao,
            estado: EstadoPendencia::Aberta,
            prazo: None,
            bloqueia: Vec::new(),
            protocolos: Vec::new(),
            metadata_bruta: Value::Object(serde_json::Map::new()),
        };
        p.validar()?;
        Ok(p)
    }

    pub fn validar(&self) -> Result<()> {
        exigir(&self.id, "pendencia.id")?;
        exigir(&self.titulo, "pendencia.titulo")?;
        if ocultar_numeros_longos(&self.titulo) != self.titulo {
            return Err(CoreError::DadoPessoalSuspeito("pendencia.titulo"));
        }
        if let SistemaOrigem::Outro(nome) = &self.origem {
            exigir(nome, "pendencia.origem")?;
        }
        if let Some(p) = &self.prazo {
            validar_data_iso(p)?;
        }
        self.acao.validar()?;
        if let EstadoPendencia::Descartada { motivo } = &self.estado {
            exigir(motivo, "descartada.motivo")?;
        }
        for p in &self.protocolos {
            p.validar()?;
        }
        if self.bloqueia.contains(&self.id) {
            return Err(CoreError::ValorDesconhecido {
                campo: "pendencia.bloqueia",
                valor: self.id.clone(),
            });
        }
        Ok(())
    }

    /// Prazo mais próximo entre o da pendência, o da ação e, aguardando órgão, o dos protocolos.
    pub fn prazo_proprio(&self) -> Option<&str> {
        let protocolos = self
            .protocolos
            .iter()
            .filter(|_| self.estado == EstadoPendencia::AguardandoOrgao)
            .filter_map(|p| p.prazo_resposta.as_deref());
        self.prazo
            .as_deref()
            .into_iter()
            .chain(self.acao.prazo())
            .chain(protocolos)
            .min()
    }

    /// Muda o estado se a transição for válida (ver [`EstadoPendencia`]).
    pub fn transicionar(&mut self, novo: EstadoPendencia) -> Result<()> {
        self.estado = self.estado.transicionar(novo)?;
        Ok(())
    }

    /// Única saída de `Resolvida`/`Descartada`: volta para `Aberta` e anota o motivo nas evidências.
    pub fn reabrir(&mut self, motivo: &str) -> Result<()> {
        exigir(motivo, "reabrir.motivo")?;
        if self.estado.ativa() {
            return Err(CoreError::TransicaoInvalida {
                de: self.estado.codigo(),
                para: EstadoPendencia::Aberta.codigo(),
            });
        }
        self.evidencias.push(format!("Reaberta: {}", motivo.trim()));
        self.estado = EstadoPendencia::Aberta;
        Ok(())
    }

    /// Anota um chamado aberto: a pendência passa a aguardar o órgão e a ação vira acompanhar o
    /// protocolo. Mesmo canal + número substitui o registro anterior (situação atualizada).
    pub fn registrar_protocolo(&mut self, protocolo: Protocolo) -> Result<()> {
        protocolo.validar()?;
        let novo_estado = self.estado.transicionar(EstadoPendencia::AguardandoOrgao)?;
        self.acao = Acao::AguardarOrgao {
            protocolo: protocolo.numero.clone(),
        };
        self.estado = novo_estado;
        self.protocolos
            .retain(|p| !(p.canal == protocolo.canal && p.numero == protocolo.numero));
        self.protocolos.push(protocolo);
        Ok(())
    }

    /// Atualiza com uma versão recém-coletada da mesma pendência sem perder nada: o estado é o
    /// local (só muda por `transicionar`/`reabrir`), evidências, bloqueios e protocolos são
    /// unidos e o `metadata_bruta` é mesclado. Aguardando um protocolo, a ação local fica.
    fn mesclar(&mut self, nova: Pendencia) {
        self.origem = nova.origem;
        self.titulo = nova.titulo;
        self.detalhe = nova.detalhe;
        self.prazo = nova.prazo;
        let aguardando = self.estado == EstadoPendencia::AguardandoOrgao
            && matches!(self.acao, Acao::AguardarOrgao { .. });
        if !aguardando {
            self.acao = nova.acao;
        }
        unir(&mut self.evidencias, nova.evidencias);
        unir(&mut self.bloqueia, nova.bloqueia);
        for p in nova.protocolos {
            self.protocolos
                .retain(|q| !(q.canal == p.canal && q.numero == p.numero));
            self.protocolos.push(p);
        }
        mesclar_json(&mut self.metadata_bruta, nova.metadata_bruta);
    }

    /// Converte uma aula não verde do Professor Presente numa pendência do painel.
    ///
    /// - Falta algo no Diário e o professor ainda está na janela: `LancamentoDiario`, ação
    ///   `LancarComConfirmacao` com o último dia dele.
    /// - Falta algo e a janela dele fechou: `LancamentoDiario`, `PedirAoOrgao` ao perfil que ainda
    ///   lança (gestão, supervisor, dirigente) com o prazo desse perfil, ou pedido formal à
    ///   Diretoria de Ensino se todas fecharam.
    /// - Lançada por outro perfil: `PresencaAula`, `PedirAoOrgao` (no mínimo a gestão).
    ///
    /// Os canais sugeridos para falar com cada órgão são [Provável]: confirmar com a escola.
    pub fn de_presenca(pp: &PendenciaPresenca, janelas: &JanelasEdicao) -> Result<Self> {
        let aula = &pp.aula;
        aula.validar()?;
        let falta = match pp.estado {
            EstadoAulaProfessor::Completa => {
                return Err(CoreError::ValorDesconhecido {
                    campo: "pendencia_presenca.estado",
                    valor: "COMPLETA".into(),
                })
            }
            EstadoAulaProfessor::FaltaRegistro => "falta o registro de aula",
            EstadoAulaProfessor::FaltaFrequencia => "falta a frequência",
            EstadoAulaProfessor::FaltaTudo => "faltam a frequência e o registro de aula",
            EstadoAulaProfessor::LancadaPorOutro => "lançada por outro perfil",
        };
        let lancavel = pp.estado.lancavel();
        let dia_aula = dia_juliano(&aula.data)?;
        let desde = janelas.professor - 1 - pp.dias_restantes_professor;
        let perfil = if lancavel {
            pp.quem_pode
        } else {
            pp.quem_pode
                .max(janelas.quem_pode(desde))
                .max(QuemPodeLancar::GestaoEscola)
        };
        let (area, origem) = if lancavel {
            (
                AreaPendencia::LancamentoDiario,
                SistemaOrigem::DiarioSalaDoFuturo,
            )
        } else {
            (
                AreaPendencia::PresencaAula,
                SistemaOrigem::SedProfessorPresente,
            )
        };
        let (acao, prazo) = acao_para_perfil(perfil, dia_aula, janelas);

        let chave = format!("{}|{}|{}", aula.turma_id, aula.data, aula.inicio);
        let titulo = format!(
            "Aula de {}/{} às {}: {falta}",
            &aula.data[8..10],
            &aula.data[5..7],
            aula.inicio
        );
        let mut p = Self::nova(area, &chave, origem, titulo, acao)?;
        p.detalhe = match &p.acao {
            Acao::LancarComConfirmacao { .. } => format!(
                "Turma {}, {}. Você ainda lança no Diário: só o que foi dado de verdade, com a sua \
                 confirmação.",
                aula.turma_id, aula.disciplina
            ),
            _ => format!(
                "Turma {}, {}. A sua janela de lançamento fechou: o pedido vai ao órgão indicado, \
                 com as evidências.",
                aula.turma_id, aula.disciplina
            ),
        };
        p.prazo = prazo;
        p.evidencias = vec![
            format!("Diário, frequência: {}", descrever(&aula.frequencia)),
            format!("Diário, registro de aula: {}", descrever(&aula.registro)),
            format!("Janela do professor: até {}", pp.vence_professor),
        ];
        p.metadata_bruta =
            serde_json::to_value(pp).map_err(|e| CoreError::Serializacao(e.to_string()))?;
        p.validar()?;
        Ok(p)
    }
}

fn descrever(l: &LancamentoDiario) -> String {
    match l {
        LancamentoDiario::Ausente => "não lançado".into(),
        LancamentoDiario::Proprio => "lançado pelo professor".into(),
        LancamentoDiario::Outro {
            perfil,
            tipo,
            data_lancamento,
        } => match data_lancamento {
            Some(d) => format!("lançado por outro perfil ({perfil}, {tipo}, em {d})"),
            None => format!("lançado por outro perfil ({perfil}, {tipo})"),
        },
    }
}

/// Ação e prazo para quem ainda pode lançar a aula do dia `dia_aula` (dias desde 1970).
fn acao_para_perfil(
    perfil: QuemPodeLancar,
    dia_aula: i64,
    janelas: &JanelasEdicao,
) -> (Acao, Option<String>) {
    let ultimo_dia = |janela: i64| Some(data_de_dia_juliano(dia_aula + janela - 1));
    let pedir = |canal: &str, orgao: &str| Acao::PedirAoOrgao {
        canal: SistemaOrigem::Outro(canal.into()),
        orgao: orgao.into(),
    };
    match perfil {
        QuemPodeLancar::Professor => {
            let prazo = ultimo_dia(janelas.professor);
            (
                Acao::LancarComConfirmacao {
                    prazo: prazo.clone(),
                },
                prazo,
            )
        }
        QuemPodeLancar::GestaoEscola => (
            pedir(
                "Contato direto com a gestão da escola",
                "Gestão da escola (direção ou coordenação)",
            ),
            ultimo_dia(janelas.gestao),
        ),
        QuemPodeLancar::Supervisor => (
            pedir("Diretoria de Ensino", "Supervisor de ensino"),
            ultimo_dia(janelas.supervisor),
        ),
        QuemPodeLancar::Dirigente => (
            pedir("Diretoria de Ensino", "Dirigente de ensino"),
            ultimo_dia(janelas.dirigente),
        ),
        QuemPodeLancar::ForaDasJanelas => (
            pedir("Pedido formal à Diretoria de Ensino", "Diretoria de Ensino"),
            None,
        ),
    }
}

/// Ordem da agenda: prazo vencendo (ou vencido) > trava outra pendência > o resto (prazo mais
/// próximo primeiro, sem prazo por último).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "nivel", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Prioridade {
    /// Até [`LIMIAR_PRAZO_VENCENDO_DIAS`] dias (negativo = já venceu).
    PrazoVencendo {
        dias_restantes: i64,
    },
    /// Trava `dependentes` pendências ativas (direta ou indiretamente).
    Bloqueia {
        dependentes: usize,
    },
    Normal {
        dias_restantes: Option<i64>,
    },
}

impl Prioridade {
    fn chave(&self) -> (u8, i64) {
        match *self {
            Self::PrazoVencendo { dias_restantes } => (0, dias_restantes),
            Self::Bloqueia { dependentes } => (1, -i64::try_from(dependentes).unwrap_or(i64::MAX)),
            Self::Normal {
                dias_restantes: Some(d),
            } => (2, d),
            Self::Normal {
                dias_restantes: None,
            } => (3, 0),
        }
    }
}

impl PartialOrd for Prioridade {
    fn partial_cmp(&self, outro: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(outro))
    }
}

impl Ord for Prioridade {
    fn cmp(&self, outro: &Self) -> std::cmp::Ordering {
        self.chave().cmp(&outro.chave())
    }
}

/// Uma linha da agenda do professor.
#[derive(Debug, Clone, Serialize)]
pub struct ItemAgenda<'a> {
    pub pendencia: &'a Pendencia,
    pub prioridade: Prioridade,
    /// Prazo mais próximo entre o desta pendência e o das que ela trava (o farol tem que sair
    /// antes de fechar a inscrição que ele bloqueia).
    pub prazo_efetivo: Option<String>,
    /// Pendências ativas que travam esta diretamente.
    pub bloqueada_por: Vec<String>,
}

/// Quanto já foi regularizado (resolvidas + descartadas).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progresso {
    pub total: usize,
    pub encerradas: usize,
}

impl Progresso {
    /// De 0 a 100. Sem pendências = 100 (nada falta).
    pub fn percentual(&self) -> f64 {
        if self.total == 0 {
            100.0
        } else {
            self.encerradas as f64 * 100.0 / self.total as f64
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumoRegularizacao {
    pub geral: Progresso,
    /// Só as áreas que têm alguma pendência.
    pub por_area: BTreeMap<AreaPendencia, Progresso>,
}

/// Uma pendência ativa travada por outra(s) ainda ativa(s).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bloqueio {
    pub bloqueada: String,
    /// Quem trava diretamente.
    pub bloqueada_por: Vec<String>,
    /// Onde a cadeia começa (pendências ativas que não são travadas por ninguém). Vazio = só há
    /// dependência circular: resolver à mão.
    pub raizes: Vec<String>,
    /// Caminho mais curto da raiz mais próxima até a bloqueada (inclusive), ex.:
    /// `[farol, atribuição]`.
    pub cadeia: Vec<String>,
}

/// O painel do que falta para o professor ficar 100% regularizado.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PainelSituacao {
    pendencias: Vec<Pendencia>,
}

impl PainelSituacao {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn pendencias(&self) -> &[Pendencia] {
        &self.pendencias
    }

    pub fn obter(&self, id: &str) -> Option<&Pendencia> {
        self.pendencias.iter().find(|p| p.id == id)
    }

    fn obter_mut(&mut self, id: &str) -> Result<&mut Pendencia> {
        self.pendencias
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| CoreError::PendenciaNaoEncontrada(id.to_string()))
    }

    fn ativa(&self, id: &str) -> Option<&Pendencia> {
        self.obter(id).filter(|p| p.estado.ativa())
    }

    /// Upsert por `id`. Nova: entra como veio. Já existente: é atualizada sem perder estado local,
    /// evidências, protocolos nem `metadata_bruta` (ver `Pendencia::mesclar`). Devolve `true` se
    /// a pendência é nova. Aplicar a mesma pendência de novo não muda nada.
    pub fn adicionar(&mut self, pendencia: Pendencia) -> Result<bool> {
        pendencia.validar()?;
        match self.pendencias.iter_mut().find(|p| p.id == pendencia.id) {
            Some(existente) => {
                existente.mesclar(pendencia);
                Ok(false)
            }
            None => {
                self.pendencias.push(pendencia);
                Ok(true)
            }
        }
    }

    /// Mesmo que [`Self::adicionar`] (nome para quem chama com uma versão nova).
    pub fn atualizar(&mut self, pendencia: Pendencia) -> Result<bool> {
        self.adicionar(pendencia)
    }

    pub fn transicionar(&mut self, id: &str, novo: EstadoPendencia) -> Result<()> {
        self.obter_mut(id)?.transicionar(novo)
    }

    pub fn reabrir(&mut self, id: &str, motivo: &str) -> Result<()> {
        self.obter_mut(id)?.reabrir(motivo)
    }

    pub fn registrar_protocolo(&mut self, id: &str, protocolo: Protocolo) -> Result<()> {
        self.obter_mut(id)?.registrar_protocolo(protocolo)
    }

    /// Converte e junta (upsert) as aulas não verdes do Professor Presente. Devolve quantas eram
    /// novas. Aula que ficou verde some da lista de entrada, mas aqui só sai por `transicionar`
    /// para `Resolvida`, depois de conferida no Professor Presente.
    ///
    /// Se uma aula marcada `Resolvida` continua na lista (a coleta ainda a mostra sem verde), a
    /// pendência é reaberta com esse motivo: senão o painel diria 100% com a aula ainda branca.
    /// `Descartada` é decisão do professor e fica como está.
    pub fn importar_presenca(
        &mut self,
        pendencias: &[PendenciaPresenca],
        janelas: &JanelasEdicao,
    ) -> Result<usize> {
        let convertidas = pendencias
            .iter()
            .map(|pp| Pendencia::de_presenca(pp, janelas))
            .collect::<Result<Vec<_>>>()?;
        let mut novas = 0;
        for p in convertidas {
            let resolvida = self
                .obter(&p.id)
                .is_some_and(|e| e.estado == EstadoPendencia::Resolvida);
            let id = p.id.clone();
            if self.adicionar(p)? {
                novas += 1;
            }
            if resolvida {
                self.reabrir(
                    &id,
                    "a coleta do Professor Presente ainda mostra a aula sem verde",
                )?;
            }
        }
        Ok(novas)
    }

    /// Painel novo a partir de `presenca_professor::pendencias`.
    pub fn de_presenca(pendencias: &[PendenciaPresenca], janelas: &JanelasEdicao) -> Result<Self> {
        let mut painel = Self::new();
        painel.importar_presenca(pendencias, janelas)?;
        Ok(painel)
    }

    /// Para cada pendência ativa, quem a trava diretamente (só pendências ativas contam).
    fn mapa_bloqueadores(&self) -> BTreeMap<&str, BTreeSet<&str>> {
        let mut mapa: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        for p in self.pendencias.iter().filter(|p| p.estado.ativa()) {
            for alvo in &p.bloqueia {
                if let Some(d) = self.ativa(alvo) {
                    if d.id != p.id {
                        mapa.entry(d.id.as_str()).or_default().insert(p.id.as_str());
                    }
                }
            }
        }
        mapa
    }

    /// Pendências ativas que dependem de `origem`, direta ou indiretamente (sem ela mesma).
    fn dependentes_ativos<'a>(&'a self, origem: &'a Pendencia) -> Vec<&'a Pendencia> {
        let mut vistos: BTreeSet<&str> = BTreeSet::from([origem.id.as_str()]);
        let mut fila = VecDeque::from([origem]);
        let mut saida = Vec::new();
        while let Some(p) = fila.pop_front() {
            for alvo in &p.bloqueia {
                if let Some(d) = self.ativa(alvo) {
                    if vistos.insert(d.id.as_str()) {
                        saida.push(d);
                        fila.push_back(d);
                    }
                }
            }
        }
        saida
    }

    /// Quantas pendências ativas travam `id`, direta ou indiretamente (sem ela mesma).
    fn quantos_travam(bloqueadores: &BTreeMap<&str, BTreeSet<&str>>, id: &str) -> usize {
        let mut vistos: BTreeSet<&str> = BTreeSet::from([id]);
        let mut fila = VecDeque::from([id]);
        while let Some(n) = fila.pop_front() {
            for &b in bloqueadores.get(n).into_iter().flatten() {
                if vistos.insert(b) {
                    fila.push_back(b);
                }
            }
        }
        vistos.len() - 1
    }

    /// As pendências ativas, da mais urgente para a menos urgente (ver [`Prioridade`]). No empate,
    /// vem antes a que tem menos pendências acima dela na cadeia de bloqueios (a não travada
    /// primeiro; em EFAPE -> farol -> atribuição com o mesmo prazo herdado, nessa ordem), depois
    /// as que dependem do professor (antes das que aguardam órgão), depois o `id`.
    pub fn proximas_acoes(&self, hoje: &str) -> Result<Vec<ItemAgenda<'_>>> {
        let hoje_d = dia_juliano(hoje)?;
        let bloqueadores = self.mapa_bloqueadores();
        let mut travas: BTreeMap<&str, usize> = BTreeMap::new();
        let mut itens = Vec::new();
        for p in self.pendencias.iter().filter(|p| p.estado.ativa()) {
            let dependentes = self.dependentes_ativos(p);
            let prazo_efetivo = p
                .prazo_proprio()
                .into_iter()
                .chain(dependentes.iter().filter_map(|d| d.prazo_proprio()))
                .min()
                .map(str::to_string);
            let dias = prazo_efetivo
                .as_deref()
                .map(|d| dia_juliano(d).map(|x| x - hoje_d))
                .transpose()?;
            let prioridade = match dias {
                Some(d) if d <= LIMIAR_PRAZO_VENCENDO_DIAS => {
                    Prioridade::PrazoVencendo { dias_restantes: d }
                }
                _ if !dependentes.is_empty() => Prioridade::Bloqueia {
                    dependentes: dependentes.len(),
                },
                _ => Prioridade::Normal {
                    dias_restantes: dias,
                },
            };
            let bloqueada_por = bloqueadores
                .get(p.id.as_str())
                .map(|s| s.iter().map(|x| x.to_string()).collect())
                .unwrap_or_default();
            travas.insert(
                p.id.as_str(),
                Self::quantos_travam(&bloqueadores, p.id.as_str()),
            );
            itens.push(ItemAgenda {
                pendencia: p,
                prioridade,
                prazo_efetivo,
                bloqueada_por,
            });
        }
        itens.sort_by(|a, b| {
            let chave = |i: &ItemAgenda<'_>| {
                (
                    i.prioridade,
                    travas.get(i.pendencia.id.as_str()).copied().unwrap_or(0),
                    i.pendencia.estado.ordem_na_agenda(),
                    i.pendencia.id.clone(),
                )
            };
            chave(a).cmp(&chave(b))
        });
        Ok(itens)
    }

    /// Percentual regularizado (resolvidas + descartadas sobre o total), geral e por área.
    pub fn percentual_regularizado(&self) -> ResumoRegularizacao {
        let mut resumo = ResumoRegularizacao::default();
        for p in &self.pendencias {
            let encerrada = usize::from(p.estado.encerrada());
            for prog in [
                &mut resumo.geral,
                resumo.por_area.entry(p.area).or_default(),
            ] {
                prog.total += 1;
                prog.encerradas += encerrada;
            }
        }
        resumo
    }

    /// As pendências ativas travadas por outras ativas, com a cadeia desde a raiz
    /// (ex.: farol vermelho -> atribuição online).
    pub fn bloqueios(&self) -> Vec<Bloqueio> {
        let mapa = self.mapa_bloqueadores();
        let mut saida = Vec::new();
        for p in self.pendencias.iter().filter(|p| p.estado.ativa()) {
            let alvo = p.id.as_str();
            let Some(diretos) = mapa.get(alvo) else {
                continue;
            };
            // Busca em largura "para cima": de quem trava quem trava... até quem ninguém trava.
            let mut filho_de: BTreeMap<&str, &str> = BTreeMap::new();
            let mut vistos: BTreeSet<&str> = BTreeSet::from([alvo]);
            let mut fila = VecDeque::from([alvo]);
            let mut raizes: Vec<&str> = Vec::new();
            while let Some(n) = fila.pop_front() {
                match mapa.get(n) {
                    None => raizes.push(n),
                    Some(acima) => {
                        for &b in acima {
                            if vistos.insert(b) {
                                filho_de.insert(b, n);
                                fila.push_back(b);
                            }
                        }
                    }
                }
            }
            let cadeia = match raizes.first() {
                Some(raiz) => {
                    let mut c = vec![raiz.to_string()];
                    let mut atual = *raiz;
                    while let Some(&f) = filho_de.get(atual) {
                        c.push(f.to_string());
                        atual = f;
                    }
                    c
                }
                None => vec![alvo.to_string()],
            };
            raizes.sort_unstable();
            saida.push(Bloqueio {
                bloqueada: alvo.to_string(),
                bloqueada_por: diretos.iter().map(|x| x.to_string()).collect(),
                raizes: raizes.into_iter().map(str::to_string).collect(),
                cadeia,
            });
        }
        saida
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::presenca_professor::{pendencias, AulaDiario};
    use serde_json::json;

    // Fixtures 100% sintéticas: nenhum dado real de professor, aluno ou escola.
    fn pend(area: AreaPendencia, chave: &str, titulo: &str) -> Pendencia {
        Pendencia::nova(
            area,
            chave,
            SistemaOrigem::Efape,
            titulo,
            Acao::LevantarSozinho,
        )
        .unwrap()
    }

    fn ids(itens: &[ItemAgenda<'_>]) -> Vec<String> {
        itens.iter().map(|i| i.pendencia.id.clone()).collect()
    }

    #[test]
    fn id_estavel_e_separado_por_area() {
        let a = gerar_id(AreaPendencia::Farol, "2026-2");
        assert_eq!(a, gerar_id(AreaPendencia::Farol, " 2026-2 "));
        assert!(a.starts_with("farol-"));
        assert_eq!(a.len(), "farol-".len() + 16);
        assert_ne!(a, gerar_id(AreaPendencia::Pagamento, "2026-2"));
        assert_ne!(a, gerar_id(AreaPendencia::Farol, "2026-3"));
        assert!(Pendencia::nova(
            AreaPendencia::Farol,
            " ",
            SistemaOrigem::Efape,
            "Farol",
            Acao::LevantarSozinho
        )
        .is_err());
    }

    #[test]
    fn upsert_idempotente_sem_perder_metadata() {
        let mut painel = PainelSituacao::new();
        let mut p = pend(
            AreaPendencia::Pagamento,
            "holerite-2026-10",
            "Revisar holerite de outubro",
        );
        p.evidencias = vec!["Holerite de outubro baixado".into()];
        p.metadata_bruta = json!({"competencia": "2026-10", "fonte": {"tela": "A"}});
        assert!(painel.adicionar(p.clone()).unwrap());
        assert!(!painel.adicionar(p.clone()).unwrap());
        assert_eq!(painel.pendencias().len(), 1);
        assert_eq!(painel.pendencias()[0], p);

        painel
            .transicionar(&p.id, EstadoPendencia::EmAndamento)
            .unwrap();
        let mut nova = p.clone();
        nova.titulo = "Revisar holerite de outubro (aulas pagas)".into();
        nova.evidencias = vec![
            "Holerite de outubro baixado".into(),
            "Aulas pagas: 30".into(),
        ];
        nova.metadata_bruta = json!({"fonte": {"linha": 7}, "valor_extra": true});
        assert!(!painel.atualizar(nova.clone()).unwrap());
        assert!(!painel.atualizar(nova).unwrap());

        let atual = painel.obter(&p.id).unwrap();
        assert_eq!(atual.estado, EstadoPendencia::EmAndamento); // estado local preservado
        assert_eq!(atual.titulo, "Revisar holerite de outubro (aulas pagas)");
        assert_eq!(atual.evidencias.len(), 2);
        assert_eq!(
            atual.metadata_bruta,
            json!({"competencia": "2026-10", "fonte": {"tela": "A", "linha": 7}, "valor_extra": true})
        );
    }

    #[test]
    fn transicoes_validas_e_invalidas() {
        let mut p = pend(
            AreaPendencia::DiaFalta,
            "2026-09-03",
            "Dia vermelho de 03/09",
        );
        p.transicionar(EstadoPendencia::Aberta).unwrap(); // mesmo estado: nada muda
        p.transicionar(EstadoPendencia::EmAndamento).unwrap();
        p.transicionar(EstadoPendencia::AguardandoOrgao).unwrap();
        p.transicionar(EstadoPendencia::Aberta).unwrap(); // órgão devolveu
        p.transicionar(EstadoPendencia::Resolvida).unwrap();

        assert_eq!(
            p.transicionar(EstadoPendencia::Aberta),
            Err(CoreError::TransicaoInvalida {
                de: "RESOLVIDA",
                para: "ABERTA"
            })
        );
        assert!(p.transicionar(EstadoPendencia::EmAndamento).is_err());
        assert!(p
            .transicionar(EstadoPendencia::Descartada { motivo: "x".into() })
            .is_err());
        assert_eq!(p.estado, EstadoPendencia::Resolvida);

        assert!(p.reabrir(" ").is_err());
        p.reabrir("Voltou a aparecer branco no Professor Presente")
            .unwrap();
        assert_eq!(p.estado, EstadoPendencia::Aberta);
        assert!(p.evidencias.last().unwrap().starts_with("Reaberta:"));
        assert!(p.reabrir("de novo").is_err()); // já está aberta

        assert!(p
            .transicionar(EstadoPendencia::Descartada { motivo: " ".into() })
            .is_err());
        p.transicionar(EstadoPendencia::Descartada {
            motivo: "Abonado pela escola".into(),
        })
        .unwrap();
        assert!(p.transicionar(EstadoPendencia::AguardandoOrgao).is_err());
    }

    #[test]
    fn ordena_por_prazo_depois_bloqueio_depois_resto() {
        let hoje = "2026-11-14";
        let sem_prazo = pend(AreaPendencia::Pagamento, "a", "Sem prazo");
        let mut vencendo = pend(AreaPendencia::DiaFalta, "b", "Prazo em 2 dias");
        vencendo.prazo = Some("2026-11-16".into());
        let mut atribuicao = pend(
            AreaPendencia::CadastroAtribuicao,
            "atrib",
            "Atribuição online",
        );
        let mut farol = pend(AreaPendencia::Farol, "farol", "Farol vermelho");
        farol.bloqueia = vec![atribuicao.id.clone()];
        let mut longe = pend(AreaPendencia::FormacaoEfape, "e", "Curso até o fim do ano");
        longe.prazo = Some("2026-12-31".into());
        let mut resolvida = pend(AreaPendencia::Pagamento, "r", "Já resolvida");
        resolvida.prazo = Some("2026-11-14".into());
        resolvida.estado = EstadoPendencia::Resolvida;
        atribuicao.estado = EstadoPendencia::AguardandoOrgao;

        let mut painel = PainelSituacao::new();
        for p in [
            &sem_prazo,
            &vencendo,
            &atribuicao,
            &farol,
            &longe,
            &resolvida,
        ] {
            painel.adicionar(p.clone()).unwrap();
        }
        let agenda = painel.proximas_acoes(hoje).unwrap();
        assert_eq!(
            ids(&agenda),
            [
                vencendo.id.as_str(),
                farol.id.as_str(),
                longe.id.as_str(),
                sem_prazo.id.as_str(),
                atribuicao.id.as_str()
            ]
        );
        assert_eq!(
            agenda[0].prioridade,
            Prioridade::PrazoVencendo { dias_restantes: 2 }
        );
        assert_eq!(
            agenda[1].prioridade,
            Prioridade::Bloqueia { dependentes: 1 }
        );
        assert_eq!(
            agenda[2].prioridade,
            Prioridade::Normal {
                dias_restantes: Some(47)
            }
        );
        assert_eq!(agenda[4].bloqueada_por, [farol.id.clone()]);

        // Prazo vencido vem antes de prazo vencendo.
        let mut vencida = pend(AreaPendencia::DiaFalta, "v", "Prazo vencido");
        vencida.prazo = Some("2026-11-10".into());
        painel.adicionar(vencida.clone()).unwrap();
        assert_eq!(
            painel.proximas_acoes(hoje).unwrap()[0].pendencia.id,
            vencida.id
        );
        assert!(painel.proximas_acoes("2026-02-30").is_err());
    }

    #[test]
    fn bloqueador_herda_o_prazo_de_quem_ele_trava() {
        let mut atribuicao = pend(
            AreaPendencia::CadastroAtribuicao,
            "atrib",
            "Inscrição na atribuição",
        );
        atribuicao.prazo = Some("2026-11-17".into());
        let mut farol = pend(AreaPendencia::Farol, "farol", "Farol vermelho");
        farol.bloqueia = vec![atribuicao.id.clone()];
        let outra = pend(AreaPendencia::Pagamento, "p", "Outra");
        let mut painel = PainelSituacao::new();
        for p in [&outra, &atribuicao, &farol] {
            painel.adicionar(p.clone()).unwrap();
        }
        let agenda = painel.proximas_acoes("2026-11-14").unwrap();
        assert_eq!(
            ids(&agenda),
            [farol.id.as_str(), atribuicao.id.as_str(), outra.id.as_str()]
        );
        assert_eq!(
            agenda[0].prioridade,
            Prioridade::PrazoVencendo { dias_restantes: 3 }
        );
        assert_eq!(agenda[0].prazo_efetivo.as_deref(), Some("2026-11-17"));
    }

    #[test]
    fn cadeia_com_o_mesmo_prazo_herdado_segue_a_ordem_da_cadeia() {
        // Os ids em ordem alfabética seriam atribuição < farol < EFAPE: a ordem certa vem da
        // cadeia, não do id.
        let mut atribuicao = pend(
            AreaPendencia::CadastroAtribuicao,
            "atrib",
            "Inscrição na atribuição",
        );
        atribuicao.prazo = Some("2026-11-17".into());
        let mut farol = pend(AreaPendencia::Farol, "farol", "Farol vermelho");
        farol.bloqueia = vec![atribuicao.id.clone()];
        let mut efape = pend(AreaPendencia::FormacaoEfape, "curso", "Curso obrigatório");
        efape.bloqueia = vec![farol.id.clone()];
        assert!(atribuicao.id < farol.id && farol.id < efape.id);
        let mut painel = PainelSituacao::new();
        for p in [&atribuicao, &farol, &efape] {
            painel.adicionar(p.clone()).unwrap();
        }
        let agenda = painel.proximas_acoes("2026-11-14").unwrap();
        assert_eq!(
            ids(&agenda),
            [efape.id.as_str(), farol.id.as_str(), atribuicao.id.as_str()]
        );
        assert!(agenda
            .iter()
            .all(|i| i.prioridade == Prioridade::PrazoVencendo { dias_restantes: 3 }));
    }

    #[test]
    fn reimportar_aula_ainda_branca_reabre_a_resolvida() {
        let janelas = JanelasEdicao::default();
        let aulas = vec![
            aula(
                "2026-11-12",
                LancamentoDiario::Proprio,
                LancamentoDiario::Ausente,
            ),
            aula(
                "2026-11-13",
                LancamentoDiario::Ausente,
                LancamentoDiario::Ausente,
            ),
        ];
        let pp = pendencias(&aulas, "2026-11-14", &janelas).unwrap();
        let mut painel = PainelSituacao::de_presenca(&pp, &janelas).unwrap();
        let ids: Vec<String> = painel.pendencias().iter().map(|p| p.id.clone()).collect();
        painel
            .transicionar(&ids[0], EstadoPendencia::Resolvida)
            .unwrap();
        painel
            .transicionar(
                &ids[1],
                EstadoPendencia::Descartada {
                    motivo: "Aula não aconteceu (sintético)".into(),
                },
            )
            .unwrap();

        assert_eq!(painel.importar_presenca(&pp, &janelas).unwrap(), 0);
        let reaberta = painel.obter(&ids[0]).unwrap();
        assert_eq!(reaberta.estado, EstadoPendencia::Aberta);
        assert!(reaberta.evidencias.last().unwrap().starts_with("Reaberta:"));
        assert!(matches!(
            painel.obter(&ids[1]).unwrap().estado,
            EstadoPendencia::Descartada { .. }
        ));

        // Importar de novo não reabre outra vez nem duplica a evidência.
        let antes = painel.clone();
        painel.importar_presenca(&pp, &janelas).unwrap();
        assert_eq!(painel, antes);
    }

    #[test]
    fn percentual_geral_e_por_area() {
        let mut painel = PainelSituacao::new();
        assert_eq!(painel.percentual_regularizado().geral.percentual(), 100.0);
        let a = pend(AreaPendencia::DiaFalta, "1", "Dia 1");
        let b = pend(AreaPendencia::DiaFalta, "2", "Dia 2");
        let c = pend(AreaPendencia::Pagamento, "3", "Pagamento");
        let d = pend(AreaPendencia::Farol, "4", "Farol");
        for p in [&a, &b, &c, &d] {
            painel.adicionar(p.clone()).unwrap();
        }
        painel
            .transicionar(&a.id, EstadoPendencia::Resolvida)
            .unwrap();
        painel
            .transicionar(
                &c.id,
                EstadoPendencia::Descartada {
                    motivo: "Não se aplica".into(),
                },
            )
            .unwrap();
        let r = painel.percentual_regularizado();
        assert_eq!(
            r.geral,
            Progresso {
                total: 4,
                encerradas: 2
            }
        );
        assert_eq!(r.geral.percentual(), 50.0);
        assert_eq!(r.por_area[&AreaPendencia::DiaFalta].percentual(), 50.0);
        assert_eq!(r.por_area[&AreaPendencia::Pagamento].percentual(), 100.0);
        assert_eq!(r.por_area[&AreaPendencia::Farol].percentual(), 0.0);
        assert!(!r.por_area.contains_key(&AreaPendencia::Protocolo));
        let j = serde_json::to_value(&r).unwrap();
        assert_eq!(j["por_area"]["FAROL"]["total"], 1);
    }

    #[test]
    fn cadeia_efape_farol_atribuicao() {
        let atribuicao = pend(
            AreaPendencia::CadastroAtribuicao,
            "atrib",
            "Atribuição online",
        );
        let mut farol = pend(AreaPendencia::Farol, "farol", "Farol vermelho");
        farol.bloqueia = vec![atribuicao.id.clone(), "id-inexistente".into()];
        let mut efape = pend(AreaPendencia::FormacaoEfape, "curso", "Curso obrigatório");
        efape.bloqueia = vec![farol.id.clone()];
        let mut painel = PainelSituacao::new();
        for p in [&atribuicao, &farol, &efape] {
            painel.adicionar(p.clone()).unwrap();
        }

        let b = painel.bloqueios();
        assert_eq!(b.len(), 2);
        let da_atribuicao = b.iter().find(|x| x.bloqueada == atribuicao.id).unwrap();
        assert_eq!(
            da_atribuicao.cadeia,
            [efape.id.as_str(), farol.id.as_str(), atribuicao.id.as_str()]
        );
        assert_eq!(da_atribuicao.raizes, [efape.id.clone()]);
        assert_eq!(da_atribuicao.bloqueada_por, [farol.id.clone()]);
        let do_farol = b.iter().find(|x| x.bloqueada == farol.id).unwrap();
        assert_eq!(do_farol.cadeia, [efape.id.as_str(), farol.id.as_str()]);
        let agenda = painel.proximas_acoes("2026-11-14").unwrap();
        assert_eq!(
            agenda[0].prioridade,
            Prioridade::Bloqueia { dependentes: 2 }
        );

        painel
            .transicionar(&efape.id, EstadoPendencia::Resolvida)
            .unwrap();
        let b = painel.bloqueios();
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].cadeia, [farol.id.as_str(), atribuicao.id.as_str()]);

        painel
            .transicionar(&farol.id, EstadoPendencia::Resolvida)
            .unwrap();
        assert!(painel.bloqueios().is_empty());
    }

    #[test]
    fn dependencia_circular_nao_trava() {
        let mut a = pend(AreaPendencia::Pagamento, "a", "A");
        let mut b = pend(AreaPendencia::Pagamento, "b", "B");
        a.bloqueia = vec![b.id.clone()];
        b.bloqueia = vec![a.id.clone()];
        let mut painel = PainelSituacao::new();
        painel.adicionar(a.clone()).unwrap();
        painel.adicionar(b).unwrap();
        let bl = painel.bloqueios();
        assert_eq!(bl.len(), 2);
        assert!(bl.iter().all(|x| x.raizes.is_empty()));
        assert_eq!(painel.proximas_acoes("2026-11-14").unwrap().len(), 2);
        // Bloquear a si mesma é recusado.
        a.bloqueia = vec![a.id.clone()];
        assert!(painel.adicionar(a).is_err());
    }

    fn aula(data: &str, f: LancamentoDiario, r: LancamentoDiario) -> AulaDiario {
        AulaDiario {
            data: data.into(),
            inicio: "07:00".into(),
            turma_id: "T1".into(),
            disciplina: "Disciplina X".into(),
            frequencia: f,
            registro: r,
        }
    }

    #[test]
    fn converte_pendencias_de_presenca() {
        use LancamentoDiario::{Ausente, Proprio};
        let eventual = LancamentoDiario::Outro {
            perfil: "Professor".into(),
            tipo: "Aula Eventual".into(),
            data_lancamento: None,
        };
        let aulas = vec![
            aula("2026-11-12", Proprio, Ausente), // professor ainda lança até 16/11
            aula("2026-11-05", Ausente, Ausente), // 9 dias: gestão, até 20/11
            aula("2026-10-20", Ausente, Proprio), // 25 dias: dirigente, até 18/11
            aula("2026-08-03", Ausente, Ausente), // fora de todas as janelas
            aula("2026-11-11", eventual.clone(), eventual), // lançada por outro, gestão até 26/11
            aula("2026-11-13", Proprio, Proprio), // verde: some
        ];
        let janelas = JanelasEdicao::default();
        let pp = pendencias(&aulas, "2026-11-14", &janelas).unwrap();
        let mut painel = PainelSituacao::de_presenca(&pp, &janelas).unwrap();
        assert_eq!(painel.pendencias().len(), 5);
        let por_data = |d: &str| {
            painel
                .pendencias()
                .iter()
                .find(|p| p.metadata_bruta["aula"]["data"] == d)
                .unwrap()
                .clone()
        };

        let p = por_data("2026-11-12");
        assert_eq!(p.area, AreaPendencia::LancamentoDiario);
        assert_eq!(p.origem, SistemaOrigem::DiarioSalaDoFuturo);
        assert_eq!(
            p.acao,
            Acao::LancarComConfirmacao {
                prazo: Some("2026-11-16".into())
            }
        );
        assert_eq!(p.titulo, "Aula de 12/11 às 07:00: falta o registro de aula");
        assert_eq!(
            p.id,
            gerar_id(AreaPendencia::LancamentoDiario, "T1|2026-11-12|07:00")
        );

        let p = por_data("2026-11-05");
        assert!(matches!(&p.acao, Acao::PedirAoOrgao { orgao, .. } if orgao.starts_with("Gestão")));
        assert_eq!(p.prazo.as_deref(), Some("2026-11-20"));

        let p = por_data("2026-10-20");
        assert!(
            matches!(&p.acao, Acao::PedirAoOrgao { orgao, .. } if orgao == "Dirigente de ensino")
        );
        assert_eq!(p.prazo.as_deref(), Some("2026-11-18"));

        let p = por_data("2026-08-03");
        assert!(
            matches!(&p.acao, Acao::PedirAoOrgao { orgao, .. } if orgao == "Diretoria de Ensino")
        );
        assert_eq!(p.prazo, None);

        let p = por_data("2026-11-11");
        assert_eq!(p.area, AreaPendencia::PresencaAula);
        assert_eq!(p.origem, SistemaOrigem::SedProfessorPresente);
        assert!(matches!(&p.acao, Acao::PedirAoOrgao { orgao, .. } if orgao.starts_with("Gestão")));
        assert_eq!(p.prazo.as_deref(), Some("2026-11-26"));
        assert!(p.evidencias[0].contains("Aula Eventual"));

        // Importar de novo não duplica nem muda nada.
        let antes = painel.clone();
        assert_eq!(painel.importar_presenca(&pp, &janelas).unwrap(), 0);
        assert_eq!(painel, antes);

        // Ordem: a do professor (vence em 2 dias) primeiro; sem prazo por último.
        let agenda = painel.proximas_acoes("2026-11-14").unwrap();
        assert_eq!(
            agenda[0].pendencia.metadata_bruta["aula"]["data"],
            "2026-11-12"
        );
        assert_eq!(
            agenda.last().unwrap().pendencia.metadata_bruta["aula"]["data"],
            "2026-08-03"
        );

        // Aula completa não vira pendência.
        let mut completa = pp[0].clone();
        completa.estado = EstadoAulaProfessor::Completa;
        assert!(Pendencia::de_presenca(&completa, &janelas).is_err());
    }

    #[test]
    fn protocolo_poe_em_espera_e_sobrevive_a_nova_coleta() {
        let mut painel = PainelSituacao::new();
        let mut p = Pendencia::nova(
            AreaPendencia::Pagamento,
            "revisao-2026",
            SistemaOrigem::FalaSp,
            "Revisão de pagamento das aulas",
            Acao::PedirAoOrgao {
                canal: SistemaOrigem::FalaSp,
                orgao: "Diretoria de Ensino".into(),
            },
        )
        .unwrap();
        p.metadata_bruta = json!({"origem": "sintetica"});
        painel.adicionar(p.clone()).unwrap();
        let protocolo = Protocolo {
            canal: SistemaOrigem::FalaSp,
            numero: "PROTO-0001".into(),
            aberto_em: "2026-11-10".into(),
            assunto: "Revisão de pagamento".into(),
            situacao: "Em análise".into(),
            prazo_resposta: Some("2026-11-18".into()),
        };
        painel
            .registrar_protocolo(&p.id, protocolo.clone())
            .unwrap();
        // Coleta repetida (ainda dizendo "pedir") não apaga o protocolo nem a espera.
        painel.adicionar(p.clone()).unwrap();
        let atual = painel.obter(&p.id).unwrap();
        assert_eq!(atual.estado, EstadoPendencia::AguardandoOrgao);
        assert_eq!(
            atual.acao,
            Acao::AguardarOrgao {
                protocolo: "PROTO-0001".into()
            }
        );
        assert_eq!(atual.protocolos, std::slice::from_ref(&protocolo));
        let agenda = painel.proximas_acoes("2026-11-14").unwrap();
        assert_eq!(
            agenda[0].prioridade,
            Prioridade::PrazoVencendo { dias_restantes: 4 }
        );

        // Situação atualizada substitui o registro do mesmo protocolo.
        let mut respondido = protocolo;
        respondido.situacao = "Respondido".into();
        painel.registrar_protocolo(&p.id, respondido).unwrap();
        assert_eq!(painel.obter(&p.id).unwrap().protocolos.len(), 1);
        painel
            .transicionar(&p.id, EstadoPendencia::Resolvida)
            .unwrap();
        let mut outro = painel.obter(&p.id).unwrap().protocolos[0].clone();
        outro.numero = "PROTO-0002".into();
        assert!(painel.registrar_protocolo(&p.id, outro).is_err());
        assert!(painel
            .transicionar("inexistente", EstadoPendencia::Resolvida)
            .is_err());
    }

    #[test]
    fn recusa_titulo_com_numero_longo_e_datas_invalidas() {
        let r = Pendencia::nova(
            AreaPendencia::Protocolo,
            "x",
            SistemaOrigem::SuporteSed,
            "Chamado sobre o aluno 000.123.456.789-0",
            Acao::LevantarSozinho,
        );
        assert_eq!(r, Err(CoreError::DadoPessoalSuspeito("pendencia.titulo")));
        assert!(!format!("{r:?}").contains("123"));
        assert!(Pendencia::nova(
            AreaPendencia::Protocolo,
            "x",
            SistemaOrigem::SuporteSed,
            "Prazo ruim",
            Acao::LancarComConfirmacao {
                prazo: Some("2026-11-31".into())
            },
        )
        .is_err());
        let mut p = pend(AreaPendencia::Farol, "f", "Farol");
        p.prazo = Some("14/11/2026".into());
        assert!(PainelSituacao::new().adicionar(p).is_err());
    }

    #[test]
    fn serializa_com_tags_estaveis() {
        let mut painel = PainelSituacao::new();
        let mut p = pend(AreaPendencia::Farol, "f", "Farol");
        p.acao = Acao::PedirAoOrgao {
            canal: SistemaOrigem::Outro("Secretaria".into()),
            orgao: "Gestão da escola".into(),
        };
        painel.adicionar(p).unwrap();
        let j = serde_json::to_value(&painel).unwrap();
        let p = &j["pendencias"][0];
        assert_eq!(p["area"], "FAROL");
        assert_eq!(p["acao"]["tipo"], "PEDIR_AO_ORGAO");
        assert_eq!(p["acao"]["canal"]["OUTRO"], "Secretaria");
        assert_eq!(p["estado"]["estado"], "ABERTA");
        let volta: PainelSituacao = serde_json::from_value(j).unwrap();
        assert_eq!(volta, painel);
    }
}
