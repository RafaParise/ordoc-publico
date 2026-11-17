//! Contrato entre o back (comandos Tauri) e a interface (Leptos/WASM): dados já prontos para mostrar.
//!
//! Tudo aqui é dado simples (texto, números), sem regra: o back calcula com o domínio e a tela só
//! desenha. Este crate é puro e compila em `wasm32-unknown-unknown`, então os dois lados usam os
//! MESMOS tipos (nada de contrato duplicado). Nenhum tipo carrega nome, RA ou nota de aluno: a
//! interface só mostra contagens e textos do próprio professor.
//!
//! Também mora aqui a entrada do painel (`EntradaPainel`): o JSON local que o app lê da pasta de dados
//! (e o exemplo `examples/painel.rs`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::situacao_funcional::{
    gerar_id, Acao, AreaPendencia, EstadoPendencia, PainelSituacao, Pendencia, Prioridade, Protocolo,
    SistemaOrigem,
};

// ---------------------------------------------------------------------------------------------
// Entrada do painel (JSON local)
// ---------------------------------------------------------------------------------------------

/// O JSON de entrada: as pendências se referem umas às outras pela `chave`, não pelo id.
#[derive(Debug, Clone, Deserialize)]
pub struct EntradaPainel {
    pub hoje: String,
    pub pendencias: Vec<EntradaPendencia>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EntradaPendencia {
    pub area: AreaPendencia,
    pub chave: String,
    pub origem: SistemaOrigem,
    pub titulo: String,
    #[serde(default)]
    pub detalhe: String,
    #[serde(default)]
    pub evidencias: Vec<String>,
    pub acao: Acao,
    #[serde(default = "estado_aberta")]
    pub estado: EstadoPendencia,
    #[serde(default)]
    pub prazo: Option<String>,
    #[serde(default)]
    pub bloqueia: Vec<String>,
    #[serde(default)]
    pub protocolos: Vec<Protocolo>,
}

fn estado_aberta() -> EstadoPendencia {
    EstadoPendencia::Aberta
}

fn erro_entrada(chave: &str, motivo: impl std::fmt::Display) -> CoreError {
    CoreError::EntradaPainel(format!("pendência '{chave}': {motivo}"))
}

/// Monta o painel validando cada pendência, os bloqueios e os protocolos.
pub fn montar_painel(entrada: &EntradaPainel) -> Result<PainelSituacao> {
    let ids: BTreeMap<&str, String> = entrada
        .pendencias
        .iter()
        .map(|e| (e.chave.as_str(), gerar_id(e.area, &e.chave)))
        .collect();
    let mut painel = PainelSituacao::new();
    for e in &entrada.pendencias {
        let mut p = Pendencia::nova(e.area, &e.chave, e.origem.clone(), &e.titulo, e.acao.clone())
            .map_err(|err| erro_entrada(&e.chave, err))?;
        p.detalhe = e.detalhe.clone();
        p.evidencias = e.evidencias.clone();
        p.prazo = e.prazo.clone();
        p.bloqueia = e
            .bloqueia
            .iter()
            .map(|c| {
                ids.get(c.as_str())
                    .cloned()
                    .ok_or_else(|| erro_entrada(&e.chave, format!("bloqueia '{c}', que não existe")))
            })
            .collect::<Result<_>>()?;
        // A entrada já traz a ação e o estado atuais: o protocolo entra direto (o
        // `registrar_protocolo` trocaria a ação por "aguardar").
        for pr in &e.protocolos {
            pr.validar().map_err(|err| erro_entrada(&e.chave, err))?;
        }
        p.protocolos = e.protocolos.clone();
        p.validar().map_err(|err| erro_entrada(&e.chave, err))?;
        let id = p.id.clone();
        painel.adicionar(p).map_err(|err| erro_entrada(&e.chave, err))?;
        if e.estado != EstadoPendencia::Aberta {
            painel
                .transicionar(&id, e.estado.clone())
                .map_err(|err| erro_entrada(&e.chave, err))?;
        }
    }
    Ok(painel)
}

// ---------------------------------------------------------------------------------------------
// Textos para a tela
// ---------------------------------------------------------------------------------------------

/// Nome do sistema para a tela.
pub fn nome_sistema(s: &SistemaOrigem) -> String {
    match s {
        SistemaOrigem::SedProfessorPresente => "SED / Professor Presente".into(),
        SistemaOrigem::DiarioSalaDoFuturo => "Diário (Sala do Futuro)".into(),
        SistemaOrigem::FalaSp => "Fala SP".into(),
        SistemaOrigem::SouSp => "SOU SP".into(),
        SistemaOrigem::Efape => "EFAPE".into(),
        SistemaOrigem::Gdae => "GDAE".into(),
        SistemaOrigem::AtribuicaoOnline => "Atribuição online".into(),
        SistemaOrigem::SuporteSed => "Suporte da SED".into(),
        SistemaOrigem::PlataformaEp => "Plataforma EP".into(),
        SistemaOrigem::TarefaSp => "Tarefa SP".into(),
        SistemaOrigem::Outro(n) => n.clone(),
    }
}

/// Nome da área para a tela.
pub fn nome_area(a: AreaPendencia) -> &'static str {
    match a {
        AreaPendencia::PresencaAula => "Professor Presente",
        AreaPendencia::LancamentoDiario => "Lançamentos no Diário",
        AreaPendencia::DiaFalta => "Faltas e abonos",
        AreaPendencia::Protocolo => "Protocolos",
        AreaPendencia::FormacaoEfape => "Formação EFAPE",
        AreaPendencia::Farol => "Farol (avaliação)",
        AreaPendencia::CadastroAtribuicao => "Cadastro e atribuição",
        AreaPendencia::Pagamento => "Pagamento",
        AreaPendencia::AvaliacaoPlataforma => "Correção nas plataformas",
    }
}

/// A ação em uma frase.
pub fn descrever_acao(a: &Acao) -> String {
    match a {
        Acao::LevantarSozinho => "o Ordoc levanta sozinho (só leitura)".into(),
        Acao::LancarComConfirmacao { prazo: Some(p) } => format!("você lança, com confirmação, até {p}"),
        Acao::LancarComConfirmacao { prazo: None } => "você lança, com confirmação".into(),
        Acao::PedirAoOrgao { canal, orgao } => format!("pedir a {orgao} ({})", nome_sistema(canal)),
        Acao::AguardarOrgao { protocolo } => format!("aguardar a resposta do protocolo {protocolo}"),
        Acao::PerguntarAoProfessor { pergunta } => format!("falta você responder: {pergunta}"),
    }
}

fn nome_estado(e: &EstadoPendencia) -> &'static str {
    match e {
        EstadoPendencia::Aberta => "aberta",
        EstadoPendencia::EmAndamento => "em andamento",
        EstadoPendencia::AguardandoOrgao => "aguardando o órgão",
        EstadoPendencia::Resolvida => "resolvida",
        EstadoPendencia::Descartada { .. } => "descartada",
    }
}

// ---------------------------------------------------------------------------------------------
// Painel
// ---------------------------------------------------------------------------------------------

/// Nível de urgência para a cor na tela.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Urgencia {
    Vencido,
    Vencendo,
    Bloqueia,
    Normal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProgressoVisao {
    pub rotulo: String,
    pub total: usize,
    pub encerradas: usize,
    pub percentual: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemPainelVisao {
    pub id: String,
    pub titulo: String,
    pub detalhe: String,
    pub area: String,
    pub sistema: String,
    pub acao: String,
    pub estado: String,
    pub urgencia: Urgencia,
    /// "vence em 3 dia(s)", "VENCIDO há 2 dia(s)", "trava 2 outra(s)", "sem prazo".
    pub prioridade: String,
    pub prazo: Option<String>,
    /// Protocolos em uma linha cada: "Fala SP 123..., aberto em ...: situação".
    pub protocolos: Vec<String>,
    /// Títulos de quem trava este item.
    pub travada_por: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VisaoPainel {
    pub hoje: String,
    /// "seus dados locais" ou "exemplo sintético" (a tela avisa).
    pub fonte: String,
    pub geral: ProgressoVisao,
    pub areas: Vec<ProgressoVisao>,
    /// Pendências ativas, na ordem em que devem ser feitas.
    pub agenda: Vec<ItemPainelVisao>,
    /// Cadeias "farol → atribuição" (títulos).
    pub cadeias: Vec<Vec<String>>,
}

fn prioridade_txt(p: &Prioridade) -> (Urgencia, String) {
    match *p {
        Prioridade::PrazoVencendo { dias_restantes } if dias_restantes < 0 => {
            (Urgencia::Vencido, format!("VENCIDO há {} dia(s)", -dias_restantes))
        }
        Prioridade::PrazoVencendo { dias_restantes } => {
            (Urgencia::Vencendo, format!("vence em {dias_restantes} dia(s)"))
        }
        Prioridade::Bloqueia { dependentes } => (Urgencia::Bloqueia, format!("trava {dependentes} outra(s)")),
        Prioridade::Normal { dias_restantes: Some(d) } => (Urgencia::Normal, format!("{d} dia(s) de prazo")),
        Prioridade::Normal { dias_restantes: None } => (Urgencia::Normal, "sem prazo".into()),
    }
}

/// Monta o que a tela Painel mostra.
pub fn visao_painel(painel: &PainelSituacao, hoje: &str, fonte: &str) -> Result<VisaoPainel> {
    let resumo = painel.percentual_regularizado();
    let titulo_de = |id: &str| painel.obter(id).map(|p| p.titulo.clone()).unwrap_or_else(|| id.to_owned());
    let agenda = painel
        .proximas_acoes(hoje)?
        .into_iter()
        .map(|item| {
            let p = item.pendencia;
            let (urgencia, prioridade) = prioridade_txt(&item.prioridade);
            ItemPainelVisao {
                id: p.id.clone(),
                titulo: p.titulo.clone(),
                detalhe: p.detalhe.clone(),
                area: nome_area(p.area).to_owned(),
                sistema: nome_sistema(&p.origem),
                acao: descrever_acao(&p.acao),
                estado: nome_estado(&p.estado).to_owned(),
                urgencia,
                prioridade,
                prazo: item.prazo_efetivo.clone(),
                protocolos: p
                    .protocolos
                    .iter()
                    .map(|pr| {
                        format!(
                            "{} {}, aberto em {}: {}{}",
                            nome_sistema(&pr.canal),
                            pr.numero,
                            pr.aberto_em,
                            pr.situacao,
                            pr.prazo_resposta
                                .as_deref()
                                .map(|d| format!(" (cobrar depois de {d})"))
                                .unwrap_or_default()
                        )
                    })
                    .collect(),
                travada_por: item.bloqueada_por.iter().map(|i| titulo_de(i)).collect(),
            }
        })
        .collect();
    let progresso = |rotulo: String, total: usize, encerradas: usize, pct: f64| ProgressoVisao {
        rotulo,
        total,
        encerradas,
        percentual: pct,
    };
    Ok(VisaoPainel {
        hoje: hoje.to_owned(),
        fonte: fonte.to_owned(),
        geral: progresso(
            "Geral".into(),
            resumo.geral.total,
            resumo.geral.encerradas,
            resumo.geral.percentual(),
        ),
        areas: resumo
            .por_area
            .iter()
            .map(|(a, p)| progresso(nome_area(*a).to_owned(), p.total, p.encerradas, p.percentual()))
            .collect(),
        agenda,
        cadeias: painel
            .bloqueios()
            .into_iter()
            .map(|b| b.cadeia.iter().map(|i| titulo_de(i)).collect())
            .collect(),
    })
}

// ---------------------------------------------------------------------------------------------
// IA do Ordoc
// ---------------------------------------------------------------------------------------------

/// Resposta de "entender um pedido" (passos sem modelo: iguais offline e online).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VisaoEntender {
    /// Intenção em snake_case, quando entendeu.
    pub intencao: Option<String>,
    /// "Entendi: <descrição>. Certo?" (vazio quando não entendeu).
    pub descricao: String,
    /// Como decidiu: "regras" ou "busca nos exemplos".
    pub como: String,
    pub parametros: Vec<(String, String)>,
    /// Quando ficou em dúvida: até 3 (intenção, descrição) para o professor escolher.
    pub candidatos: Vec<(String, String)>,
    /// Tela do app que resolve (id da tela) e o que fazer lá.
    pub tela: Option<String>,
    pub proximo_passo: String,
}

/// Resposta da IA a uma dúvida (texto já com os marcadores restaurados, só no aparelho).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisaoDuvida {
    pub texto: String,
    pub motor: String,
    pub online: bool,
    /// Respondida pelo modelo local: "modo offline, texto mais simples".
    pub modo_offline: bool,
}

/// Uma mensagem da lista da caixa do Outlook (sem o endereço do remetente, só o domínio).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MensagemCaixaVisao {
    pub dominio: String,
    /// Domínio de órgão do Estado (`*.sp.gov.br`).
    pub oficial: bool,
    pub data: String,
    pub nao_lida: bool,
    pub assunto: String,
}

/// A caixa do Outlook na tela Pedidos.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct VisaoCaixa {
    pub fonte: String,
    pub mensagens: Vec<MensagemCaixaVisao>,
}

/// Um dia do calendário do Professor Presente: `data` AAAA-MM-DD e `cor` (verde, amarelo, branco, vermelho, cinza).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiaPresencaVisao {
    pub data: String,
    pub cor: String,
}

/// Cartão do Professor Presente no painel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct VisaoPresenca {
    pub fonte: String,
    /// AAAA-MM do calendário lido (vazio se ainda não houve leitura).
    pub mes: String,
    pub dias: Vec<DiaPresencaVisao>,
}

/// Tarefa da Educação Profissional com envio esperando avaliação (só contagens; nunca nome nem nota de aluno).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TarefaEpVisao {
    /// Disciplina-turma (nome do curso no Moodle).
    pub curso: String,
    pub tarefa: String,
    pub a_avaliar: u32,
    pub enviados: Option<u32>,
    pub participantes: Option<u32>,
    /// Linhas de data da atividade como o Moodle mostra (abertura, vencimento).
    #[serde(default)]
    pub datas: Vec<String>,
    /// Página da tarefa na plataforma EP.
    pub link: String,
}

/// Tela de correção (Tarefa SP agora; EP depois): o que espera o professor, com a resposta do estudante para ler.
/// Fica SÓ na memória do app enquanto a tela está aberta: nunca vai para arquivo, registro nem IA online.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct VisaoCorrecao {
    /// "tarefasp" ou "ep".
    pub plataforma: String,
    pub tarefas_lidas: u32,
    pub ja_finalizadas: u32,
    pub itens: Vec<ItemCorrecaoVisao>,
    /// A IA local (módulo offline) está ligada? Sem ela, a sugestão vem da rubrica, sem modelo.
    pub ia_local: bool,
}

/// Um envio para corrigir. `indice` é a posição na coleta em memória (não sai do aparelho).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemCorrecaoVisao {
    pub indice: u32,
    /// Tarefa, para agrupar e para o "sim" por tarefa.
    pub tarefa_id: u64,
    pub tarefa: String,
    pub turma: String,
    pub enunciado: String,
    /// "aguarda_professor", "zero_da_ia" ou "nota_zero_professor".
    pub motivo: String,
    /// Partes do envio, na ordem (texto, imagem, arquivo, link).
    pub partes: Vec<ParteEnvioVisao>,
    pub nota_atual: Option<f64>,
    pub nota_max: f64,
    /// Parecer da IA da própria plataforma, quando houve.
    pub parecer_plataforma: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParteEnvioVisao {
    /// "texto", "imagem", "arquivo" ou "link".
    pub tipo: String,
    pub texto: String,
    #[serde(default)]
    pub url: Option<String>,
}

/// Sugestão da IA (ou da rubrica) para um envio. Nunca vai ao portal sem o "sim" do professor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SugestaoVisao {
    pub indice: u32,
    pub nota: u8,
    pub novo_envio: bool,
    pub comentario: String,
    pub motivo: String,
    /// "Lancar", "Perguntar" ou "Pular".
    pub decisao: String,
    /// "Alta", "Media" ou "Baixa".
    pub confianca: String,
    /// Trechos do envio que sustentam a nota.
    pub evidencias: Vec<String>,
    /// "regra", "rubrica (sem IA)" ou o motor local.
    pub origem: String,
    /// Quantas vezes o professor já pediu para revisar este envio.
    pub revisoes: u32,
}

/// Nota decidida pelo professor na tela.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotaDecidida {
    pub indice: u32,
    pub nota: u8,
    pub comentario: String,
}

/// Lote ensaiado e esperando o "sim" (um por tarefa).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparoLancamento {
    /// Chave de uso único do "sim" (vale 10 minutos).
    pub chave: String,
    pub tarefa: String,
    pub turma: String,
    pub notas: u32,
    /// Texto da pergunta: "Lançar 12 notas na tarefa X (2A)? ..."
    pub pergunta: String,
}

/// Tarefa publicada pelo professor no Tarefa SP (Gerenciador de Atividades da Sala do Futuro, cmsp.ip.tv).
/// Só contagens por tarefa: nunca nome, login ou nota de aluno.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TarefaSpVisao {
    pub titulo: String,
    pub turma: String,
    /// "tarefa", "redacao" ou "prova".
    pub tipo: String,
    /// AAAA-MM-DD.
    pub publicada: String,
    /// AAAA-MM-DD (depois disso o estudante não entrega mais).
    pub expira: String,
    pub entregues: u32,
    /// Começaram e não entregaram.
    pub rascunhos: u32,
    /// Entregues cuja correção ainda não fechou na plataforma.
    pub aguardando: u32,
    /// Questões abertas que a IA da plataforma zerou (vale a revisão do professor).
    pub zeradas_ia: u32,
    /// Média dos pontos das entregues, como a plataforma mostra ("41.5").
    #[serde(default)]
    pub media: Option<String>,
}

/// Tela da plataforma EP: o que falta corrigir.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisaoEp {
    pub fonte: String,
    pub tarefas: Vec<TarefaEpVisao>,
    /// Tarefa SP (CMSP): tarefas publicadas pelo professor, com o que falta corrigir.
    #[serde(default)]
    pub tarefasp: Vec<TarefaSpVisao>,
}

/// Concurso, processo seletivo ou cadastro aberto em página pública do Estado (sem dado pessoal).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OportunidadeVisao {
    pub fonte: String,
    /// "concurso", "processo_seletivo" ou "banco_de_talentos".
    pub tipo: String,
    pub titulo: String,
    pub orgao: String,
    #[serde(default)]
    pub local: String,
    /// AAAA-MM-DD.
    #[serde(default)]
    pub inicio: Option<String>,
    /// AAAA-MM-DD: último dia de inscrição.
    #[serde(default)]
    pub fim: Option<String>,
    /// Taxa como a fonte informa ("gratuita", "com taxa (valor no edital)").
    pub taxa: String,
    #[serde(default)]
    pub salario: Option<String>,
    #[serde(default)]
    pub vagas: Option<u32>,
    #[serde(default)]
    pub escolaridade: Option<String>,
    /// Página oficial (edital e inscrição).
    pub link: String,
    /// Site para onde o botão leva, lido pelo parser de URL do app (o professor vê antes de clicar).
    #[serde(default)]
    pub destino: String,
    /// Bate com as áreas do professor (perfil local).
    #[serde(default)]
    pub da_area: bool,
    /// Na cidade de alguma escola do professor (perfil local).
    #[serde(default)]
    pub perto: bool,
}

/// Canal fixo para o professor trabalhar em provas (aplicador, fiscal, chefe de sala) ou se cadastrar, com a
/// regra que vale para ele e o grau de certeza da informação.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanalVisao {
    pub nome: String,
    pub para_que: String,
    pub link: String,
    /// Site para onde o botão leva (parser de URL do app).
    #[serde(default)]
    pub destino: String,
    pub observacao: String,
    /// "certo", "provável" ou "conferir".
    pub confianca: String,
}

/// Tela "Oportunidades": concursos, processos seletivos, Banco de Talentos, aplicação de provas e atribuição.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct VisaoOportunidades {
    pub fonte: String,
    /// Quando as páginas foram lidas (AAAA-MM-DD HH:MM, horário de Brasília); vazio se nunca.
    pub lido_em: String,
    pub itens: Vec<OportunidadeVisao>,
    pub canais: Vec<CanalVisao>,
    pub areas: Vec<String>,
    pub cidades: Vec<String>,
    /// Caminho do arquivo local do perfil (áreas e cidades), para o professor ajustar.
    pub arquivo_perfil: String,
}

/// Pedido que chegou pela ponte (Univo). Só intenção e parâmetros validados; nunca texto livre.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisaoPedidoPonte {
    pub id: String,
    /// Rótulo de quem pediu ("Univo (PC)").
    pub origem: String,
    /// Intenção em snake_case (a mesma do `escolher`).
    pub intencao: String,
    pub parametros: Vec<(String, String)>,
}

/// Situação da ponte e os pedidos esperando o professor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct VisaoPonte {
    pub situacao: String,
    pub pedidos: Vec<VisaoPedidoPonte>,
}

/// Estado dos motores para o topo da tela da IA.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisaoEstadoIa {
    /// "online", "local" ou "nenhum".
    pub estado: String,
    /// (id, "online"/"local") de cada motor configurado.
    pub motores: Vec<(String, String)>,
    /// Há como ligar o modelo local pelo app.
    pub pode_ligar_local: bool,
    /// Quantos nomes a lista local protege (só a contagem sai do host).
    #[serde(default)]
    pub nomes_protegidos: usize,
    /// Onde fica a lista (para a tela explicar).
    #[serde(default)]
    pub arquivo_nomes: String,
}

// ---------------------------------------------------------------------------------------------
// Pedidos e respostas
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisaoReplica {
    pub codigo: String,
    pub rotulo: String,
    pub orientacao: String,
    /// Modelo com marcadores ([DATA], [PROTOCOLO]...); o professor preenche e envia.
    pub modelo: Option<String>,
    pub limites: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisaoPedido {
    pub canal: String,
    pub protocolo: String,
    pub assunto: String,
    pub aberto_em: String,
    pub situacao: String,
    pub respondido_em: Option<String>,
    /// Trecho da resposta do órgão (sem nome de pessoa).
    pub resposta: Option<String>,
    /// `None` = a regra não reconheceu: ler e escolher a réplica.
    pub replica: Option<VisaoReplica>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisaoPedidos {
    pub fonte: String,
    pub pedidos: Vec<VisaoPedido>,
}

// ---------------------------------------------------------------------------------------------
// EFAPE e Multiplica
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SemanaVisao {
    pub nome: String,
    /// % da página do curso (a fonte que vale; o painel do AVA fica em cache).
    pub percentual: f64,
    pub faltam: Vec<String>,
    pub link: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncontroVisao {
    pub titulo: String,
    pub data: String,
    /// Link do Teams (vem do convite no e-mail @professor).
    pub link: Option<String>,
    /// A pesquisa da SED abre no dia seguinte ao encontro e fecha um dia antes do próximo.
    pub pesquisa_abre: Option<String>,
    pub pesquisa_fecha: Option<String>,
    pub pesquisa_feita: bool,
    /// Situação que a própria SED mostra ("RESPONDIDA", "ENCERRADA"...), quando lida; vale mais que as datas.
    #[serde(default)]
    pub situacao: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VisaoEfape {
    pub fonte: String,
    pub semanas: Vec<SemanaVisao>,
    pub encontros: Vec<EncontroVisao>,
    /// Endereços públicos fixos (AVA, pesquisa do Multiplica na SED).
    pub links: Vec<(String, String)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENTRADA: &str = r#"{"hoje": "2026-11-15", "pendencias": [
      {"area": "FAROL", "chave": "farol-teste", "origem": "FALA_SP", "titulo": "Farol do semestre",
       "acao": {"tipo": "AGUARDAR_ORGAO", "protocolo": "1234"}, "estado": {"estado": "AGUARDANDO_ORGAO"},
       "bloqueia": ["atrib-teste"],
       "protocolos": [{"canal": "FALA_SP", "numero": "1234", "aberto_em": "2026-11-10",
                        "assunto": "farol", "situacao": "Cadastrada", "prazo_resposta": "2026-12-10"}]},
      {"area": "CADASTRO_ATRIBUICAO", "chave": "atrib-teste", "origem": "ATRIBUICAO_ONLINE",
       "titulo": "Inscrição da atribuição", "acao": {"tipo": "LANCAR_COM_CONFIRMACAO", "prazo": "2026-11-18"}},
      {"area": "FORMACAO_EFAPE", "chave": "efape-ok", "origem": "EFAPE", "titulo": "Semana concluída",
       "acao": {"tipo": "LEVANTAR_SOZINHO"}, "estado": {"estado": "RESOLVIDA"}}
    ]}"#;

    #[test]
    fn entrada_vira_visao() {
        let e: EntradaPainel = serde_json::from_str(ENTRADA).unwrap();
        let p = montar_painel(&e).unwrap();
        let v = visao_painel(&p, &e.hoje, "exemplo sintético").unwrap();
        assert_eq!(v.geral.total, 3);
        assert_eq!(v.geral.encerradas, 1);
        assert_eq!(v.agenda.len(), 2);
        // O farol trava a atribuição, que vence em 3 dias: o prazo efetivo do farol é o da atribuição.
        let farol = v.agenda.iter().find(|i| i.titulo == "Farol do semestre").unwrap();
        assert_eq!(farol.prazo.as_deref(), Some("2026-11-18"));
        assert_eq!(farol.protocolos.len(), 1);
        let atrib = v.agenda.iter().find(|i| i.titulo == "Inscrição da atribuição").unwrap();
        assert_eq!(atrib.travada_por, vec!["Farol do semestre".to_string()]);
        assert_eq!(v.cadeias, vec![vec!["Farol do semestre".to_string(), "Inscrição da atribuição".to_string()]]);
        // Ida e volta pelo JSON (é o que atravessa a ponte Tauri).
        let ida = serde_json::to_string(&v).unwrap();
        assert_eq!(serde_json::from_str::<VisaoPainel>(&ida).unwrap(), v);
    }

    #[test]
    fn bloqueio_para_chave_inexistente_e_erro() {
        let e: EntradaPainel = serde_json::from_str(
            r#"{"hoje": "2026-11-15", "pendencias": [{"area": "FAROL", "chave": "a", "origem": "FALA_SP",
            "titulo": "x", "acao": {"tipo": "LEVANTAR_SOZINHO"}, "bloqueia": ["nao-existe"]}]}"#,
        )
        .unwrap();
        assert!(matches!(montar_painel(&e), Err(CoreError::EntradaPainel(_))));
    }
}

/// Painel do aluno por link cifrado: o que foi gerado no computador do professor (nada publicado).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisaoPortalAlunos {
    pub alunos: usize,
    pub pasta_site: String,
    /// Endereço do site publicado (vazio = ainda não publicado).
    pub url_base: String,
    pub versao: u32,
    /// "coleta" (dados reais, guardados cifrados) ou "exemplo" (alunos fictícios).
    pub fonte: String,
    /// Nome, turma e link de cada aluno: só na tela (nunca em arquivo).
    pub links: Vec<LinkAlunoVisao>,
    pub erros: Vec<String>,
    pub avisos: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkAlunoVisao {
    pub nome: String,
    pub turma: String,
    pub link: String,
}

/// Telemetria do app (sem dado de aluno): o que foi usado e o que deu erro. O Univo lê o mesmo resumo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct VisaoTelemetria {
    pub dias: i64,
    pub total: u32,
    pub erros: u32,
    pub eventos: Vec<EventoTelemetriaVisao>,
    pub atencao: Vec<String>,
    /// Resumo gravado para o Univo ler (pasta de dados do Ordoc).
    pub arquivo_univo: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct EventoTelemetriaVisao {
    pub evento: String,
    pub vezes: u32,
    pub erros: u32,
    pub ultima_vez: String,
    pub ultimo_erro: String,
}
