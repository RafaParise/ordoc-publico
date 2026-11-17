//! Padrão Adapter: cada plataforma externa (DC, Sala do Futuro, BI) entra no Ordoc por um conector
//! que fala a língua dela e entrega modelos canônicos. Mudou o layout de um portal, muda só o conector.
//! Aqui também fica a telemetria: o último estado explícito de cada conector.

use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::error::{CoreError, Result};
use crate::modelos::{
    Aluno, AtividadeSdf, Aula, EntregaSdf, IndicadorBi, Matricula, RegistroFrequencia, Turma,
};

/// Plataformas que o Ordoc unifica.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Plataforma {
    /// Diário de Classe (SED): leitura e escrita transacional.
    #[serde(rename = "dc")]
    DiarioClasse,
    /// Sala do Futuro: leitura agregada de engajamento, sincronização diferencial.
    #[serde(rename = "sdf")]
    SalaDoFuturo,
    /// Painéis de BI: leitura analítica em lote.
    #[serde(rename = "bi")]
    PainelBi,
}

impl Plataforma {
    /// Texto curto gravado no banco e usado nas chaves de idempotência.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DiarioClasse => "dc",
            Self::SalaDoFuturo => "sdf",
            Self::PainelBi => "bi",
        }
    }

    pub fn parse(valor: &str) -> Result<Self> {
        match valor {
            "dc" => Ok(Self::DiarioClasse),
            "sdf" => Ok(Self::SalaDoFuturo),
            "bi" => Ok(Self::PainelBi),
            outro => Err(CoreError::ValorDesconhecido {
                campo: "plataforma",
                valor: outro.to_string(),
            }),
        }
    }
}

/// Estado explícito de um conector, exibido no Modo Diagnóstico.
/// Os quatro primeiros são os do contrato; `Falha` cobre rede fora do ar e defeito do próprio
/// conector, que não são sessão expirada, layout alterado nem timeout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StatusConector {
    Ok,
    SessaoExpirada,
    LayoutAlterado,
    Timeout,
    Falha,
}

impl StatusConector {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::SessaoExpirada => "SESSAO_EXPIRADA",
            Self::LayoutAlterado => "LAYOUT_ALTERADO",
            Self::Timeout => "TIMEOUT",
            Self::Falha => "FALHA",
        }
    }

    pub fn parse(valor: &str) -> Result<Self> {
        match valor {
            "OK" => Ok(Self::Ok),
            "SESSAO_EXPIRADA" => Ok(Self::SessaoExpirada),
            "LAYOUT_ALTERADO" => Ok(Self::LayoutAlterado),
            "TIMEOUT" => Ok(Self::Timeout),
            "FALHA" => Ok(Self::Falha),
            outro => Err(CoreError::ValorDesconhecido {
                campo: "status_conector",
                valor: outro.to_string(),
            }),
        }
    }
}

/// O que fazer depois de um erro (é por aqui que o worker da Outbox decide).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AcaoAposErro {
    /// Falha passageira (timeout, rede): tentar de novo mais tarde, com backoff.
    TentarDeNovo,
    /// Sessão expirada: pausar a fila da plataforma, sem gastar tentativas, até o professor
    /// entrar de novo (repetir contra o login vencido só cansa o SSO).
    AguardarLogin,
    /// Não adianta repetir sozinho (recusa, layout alterado, defeito): o item vai para `FAILED`.
    Desistir,
}

/// Erro devolvido por um conector, já classificado para a telemetria e para o worker.
/// Cada conector traduz o erro do seu portal (ex.: `ExtractError`) para um destes.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum ErroConector {
    #[error("Sessão expirada: {0}")]
    SessaoExpirada(String),

    #[error("Layout ou contrato da plataforma mudou: {0}")]
    LayoutAlterado(String),

    #[error("Tempo esgotado: {0}")]
    Timeout(String),

    #[error("Falha de rede: {0}")]
    Rede(String),

    /// A plataforma recebeu e recusou a operação (validação do lado dela).
    #[error("Operação recusada pela plataforma: {0}")]
    Recusada(String),

    #[error("Operação não suportada por este conector: {0}")]
    NaoSuportada(String),

    #[error("Falha interna do conector: {0}")]
    Interno(String),
}

impl ErroConector {
    /// A saúde do conector que este erro revela. `None` quando o erro não diz nada sobre ela
    /// (a plataforma respondeu e recusou o dado, ou a operação não existe neste conector):
    /// a telemetria fica como estava e o erro aparece só no item da Outbox.
    pub fn status(&self) -> Option<StatusConector> {
        match self {
            Self::SessaoExpirada(_) => Some(StatusConector::SessaoExpirada),
            Self::LayoutAlterado(_) => Some(StatusConector::LayoutAlterado),
            Self::Timeout(_) => Some(StatusConector::Timeout),
            Self::Rede(_) | Self::Interno(_) => Some(StatusConector::Falha),
            Self::Recusada(_) | Self::NaoSuportada(_) => None,
        }
    }

    pub fn acao(&self) -> AcaoAposErro {
        match self {
            Self::Timeout(_) | Self::Rede(_) => AcaoAposErro::TentarDeNovo,
            Self::SessaoExpirada(_) => AcaoAposErro::AguardarLogin,
            Self::LayoutAlterado(_)
            | Self::Recusada(_)
            | Self::NaoSuportada(_)
            | Self::Interno(_) => AcaoAposErro::Desistir,
        }
    }
}

/// Tamanho máximo (em caracteres) do detalhe gravado na telemetria.
pub const LIMITE_DETALHE: usize = 300;

/// Mínimo de dígitos para um número ser ocultado no detalhe (o RA de SP tem 12, com zeros à esquerda).
pub const DIGITOS_NUMERO_OCULTO: usize = 9;

/// Troca números longos (9 dígitos ou mais, mesmo com `.`, `-` ou `/` no meio, como
/// `000.123.456.789-0`) por `[nº oculto]`: a plataforma pode ecoar RA ou CPF numa mensagem de erro
/// (LGPD). Datas (`2026-11-14`), códigos HTTP e ids curtos ficam.
pub(crate) fn ocultar_numeros_longos(texto: &str) -> String {
    let chars: Vec<char> = texto.chars().collect();
    let mut saida = String::with_capacity(texto.len());
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_ascii_digit() {
            saida.push(chars[i]);
            i += 1;
            continue;
        }
        let inicio = i;
        let mut digitos = 0;
        while i < chars.len() {
            if chars[i].is_ascii_digit() {
                digitos += 1;
                i += 1;
            } else if matches!(chars[i], '.' | '-' | '/')
                && chars.get(i + 1).is_some_and(char::is_ascii_digit)
            {
                i += 1;
            } else {
                break;
            }
        }
        if digitos >= DIGITOS_NUMERO_OCULTO {
            saida.push_str("[nº oculto]");
        } else {
            saida.extend(&chars[inicio..i]);
        }
    }
    saida
}

/// Limpa um texto de erro antes de gravar: corta a consulta (`?...`) e o fragmento (`#...`) de toda
/// URL, onde portais de login costumam pôr `code`, `ticket` ou `token`, oculta números longos (RA,
/// CPF) e limita o tamanho.
pub fn sanitizar_detalhe(texto: &str) -> String {
    let mut saida = String::with_capacity(texto.len());
    for pedaco in texto.split_inclusive(char::is_whitespace) {
        let (palavra, espaco) = match pedaco.char_indices().last() {
            Some((i, c)) if c.is_whitespace() => (&pedaco[..i], &pedaco[i..]),
            _ => (pedaco, ""),
        };
        if palavra.contains("://") {
            let fim = palavra.find(['?', '#']).unwrap_or(palavra.len());
            saida.push_str(&palavra[..fim]);
        } else {
            saida.push_str(palavra);
        }
        saida.push_str(espaco);
    }
    let saida = ocultar_numeros_longos(&saida);
    if saida.chars().count() <= LIMITE_DETALHE {
        saida
    } else {
        let mut curto: String = saida.chars().take(LIMITE_DETALHE).collect();
        curto.push('…');
        curto
    }
}

/// Último estado registrado de um conector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TelemetriaConector {
    pub plataforma: Plataforma,
    pub status: StatusConector,
    /// Texto do erro já sanitizado (ver `sanitizar_detalhe`).
    pub detalhe: Option<String>,
    pub registrado_em_ms: i64,
}

impl TelemetriaConector {
    pub fn ok(plataforma: Plataforma, registrado_em_ms: i64) -> Self {
        Self {
            plataforma,
            status: StatusConector::Ok,
            detalhe: None,
            registrado_em_ms,
        }
    }

    /// `None` quando o erro não muda a saúde do conector (ver `ErroConector::status`).
    pub fn de_erro(
        plataforma: Plataforma,
        erro: &ErroConector,
        registrado_em_ms: i64,
    ) -> Option<Self> {
        erro.status().map(|status| Self {
            plataforma,
            status,
            detalhe: Some(sanitizar_detalhe(&erro.to_string())),
            registrado_em_ms,
        })
    }

    fn do_resultado<T>(
        plataforma: Plataforma,
        resultado: &std::result::Result<T, ErroConector>,
        registrado_em_ms: i64,
    ) -> Option<Self> {
        match resultado {
            Ok(_) => Some(Self::ok(plataforma, registrado_em_ms)),
            Err(e) => Self::de_erro(plataforma, e, registrado_em_ms),
        }
    }
}

/// Tudo o que uma coleta trouxe, já nos modelos canônicos.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LoteCanonico {
    pub turmas: Vec<Turma>,
    pub alunos: Vec<Aluno>,
    pub matriculas: Vec<Matricula>,
    pub aulas: Vec<Aula>,
    pub frequencias: Vec<RegistroFrequencia>,
    pub atividades_sdf: Vec<AtividadeSdf>,
    pub entregas_sdf: Vec<EntregaSdf>,
    pub indicadores_bi: Vec<IndicadorBi>,
    /// Novo cursor da sincronização diferencial (ex.: maior timestamp visto), para a próxima coleta.
    pub cursor: Option<String>,
}

/// Operação de escrita que sai da Outbox para a plataforma.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OperacaoRemota {
    /// Mesma chave para a mesma intenção (ex.: `dc:frequencia:<aula_id>`): base da idempotência.
    pub chave_idempotencia: String,
    pub plataforma: Plataforma,
    /// Tipo da operação (ex.: `frequencia`, `nota`).
    pub tipo: String,
    pub payload: Value,
}

/// Resultado de um envio bem-sucedido.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResultadoEnvio {
    /// A plataforma gravou o dado agora.
    Aplicado,
    /// O conector consultou o servidor antes e o estado já era esse: nada foi reenviado.
    JaAplicado,
}

/// Contrato de todo conector educacional (Padrão Adapter do regras do projeto). Assíncrono: quem
/// implementa escreve `async fn coletar(...)` e `async fn enviar(...)`.
///
/// O conector do DC deve consultar o estado atual da aula no servidor antes de gravar
/// (idempotência), para nunca duplicar chamada ou nota.
pub trait IEducationalConnector: Send + Sync {
    fn plataforma(&self) -> Plataforma;

    /// Coleta dados da plataforma. `cursor` é o da última coleta (`None` = carga completa).
    fn coletar(
        &self,
        cursor: Option<&str>,
    ) -> impl Future<Output = std::result::Result<LoteCanonico, ErroConector>> + Send;

    /// Envia uma operação da Outbox. Conectores só de leitura (SDF, BI) ficam com este padrão.
    fn enviar(
        &self,
        operacao: &OperacaoRemota,
    ) -> impl Future<Output = std::result::Result<ResultadoEnvio, ErroConector>> + Send {
        let erro = ErroConector::NaoSuportada(format!(
            "{} não aceita a operação '{}'",
            self.plataforma().as_str(),
            operacao.tipo
        ));
        async move { Err(erro) }
    }
}

/// Futuro devolvido pela versão dinâmica do conector.
pub type FuturoConector<'a, T> =
    Pin<Box<dyn Future<Output = std::result::Result<T, ErroConector>> + Send + 'a>>;

/// Versão "dyn" de `IEducationalConnector`, para guardar conectores diferentes numa mesma lista
/// (`Vec<Box<dyn ConectorDinamico>>`) no worker de sincronização. Todo `IEducationalConnector` já é
/// um `ConectorDinamico` (implementação automática abaixo): não implemente esta trait à mão.
pub trait ConectorDinamico: Send + Sync {
    fn plataforma_dyn(&self) -> Plataforma;
    fn coletar_dyn<'a>(&'a self, cursor: Option<&'a str>) -> FuturoConector<'a, LoteCanonico>;
    fn enviar_dyn<'a>(&'a self, operacao: &'a OperacaoRemota)
        -> FuturoConector<'a, ResultadoEnvio>;
}

impl<C: IEducationalConnector> ConectorDinamico for C {
    fn plataforma_dyn(&self) -> Plataforma {
        self.plataforma()
    }

    fn coletar_dyn<'a>(&'a self, cursor: Option<&'a str>) -> FuturoConector<'a, LoteCanonico> {
        Box::pin(self.coletar(cursor))
    }

    fn enviar_dyn<'a>(
        &'a self,
        operacao: &'a OperacaoRemota,
    ) -> FuturoConector<'a, ResultadoEnvio> {
        Box::pin(self.enviar(operacao))
    }
}

/// Roda uma coleta e devolve, junto com o resultado, a telemetria a gravar (`None`: não mexer).
pub async fn coletar_com_telemetria<C: IEducationalConnector>(
    conector: &C,
    cursor: Option<&str>,
    agora_ms: i64,
) -> (
    std::result::Result<LoteCanonico, ErroConector>,
    Option<TelemetriaConector>,
) {
    let resultado = conector.coletar(cursor).await;
    let telemetria = TelemetriaConector::do_resultado(conector.plataforma(), &resultado, agora_ms);
    (resultado, telemetria)
}

/// Envia uma operação e devolve, junto com o resultado, a telemetria a gravar (`None`: não mexer).
pub async fn enviar_com_telemetria<C: IEducationalConnector>(
    conector: &C,
    operacao: &OperacaoRemota,
    agora_ms: i64,
) -> (
    std::result::Result<ResultadoEnvio, ErroConector>,
    Option<TelemetriaConector>,
) {
    let resultado = conector.enviar(operacao).await;
    let telemetria = TelemetriaConector::do_resultado(conector.plataforma(), &resultado, agora_ms);
    (resultado, telemetria)
}

#[cfg(test)]
mod testes {
    use super::*;
    use serde_json::json;

    /// Conector de mentira: a Sala do Futuro com o login vencido.
    struct SdfSessaoVencida;

    impl IEducationalConnector for SdfSessaoVencida {
        fn plataforma(&self) -> Plataforma {
            Plataforma::SalaDoFuturo
        }

        async fn coletar(
            &self,
            _cursor: Option<&str>,
        ) -> std::result::Result<LoteCanonico, ErroConector> {
            Err(ErroConector::SessaoExpirada(
                "redirecionado para https://login.exemplo.org/entrar?ticket=SEGREDO&state=1".into(),
            ))
        }
    }

    /// Conector de mentira do DC: devolve uma turma e diz que a chamada já estava no servidor.
    struct DcFalso;

    impl IEducationalConnector for DcFalso {
        fn plataforma(&self) -> Plataforma {
            Plataforma::DiarioClasse
        }

        async fn coletar(
            &self,
            cursor: Option<&str>,
        ) -> std::result::Result<LoteCanonico, ErroConector> {
            if cursor != Some("2026-11-01T00:00:00Z") {
                return Err(ErroConector::Interno("cursor inesperado".into()));
            }
            Ok(LoteCanonico {
                turmas: vec![Turma {
                    id: "T-3B".into(),
                    nome: "3B DS".into(),
                    ano_letivo: 2026,
                    escola: None,
                    codigo_sdf: None,
                    metadata_bruta: json!({"campo_extra": 1}),
                }],
                cursor: Some("2026-11-14T00:00:00Z".into()),
                ..Default::default()
            })
        }

        async fn enviar(
            &self,
            _operacao: &OperacaoRemota,
        ) -> std::result::Result<ResultadoEnvio, ErroConector> {
            Ok(ResultadoEnvio::JaAplicado)
        }
    }

    fn operacao() -> OperacaoRemota {
        OperacaoRemota {
            chave_idempotencia: "dc:frequencia:A1".into(),
            plataforma: Plataforma::DiarioClasse,
            tipo: "frequencia".into(),
            payload: json!({}),
        }
    }

    #[tokio::test]
    async fn telemetria_classifica_sessao_expirada_sem_guardar_o_ticket() {
        let (resultado, tele) = coletar_com_telemetria(&SdfSessaoVencida, None, 1_000).await;
        let erro = resultado.unwrap_err();
        assert_eq!(erro.acao(), AcaoAposErro::AguardarLogin);
        let tele = tele.unwrap();
        assert_eq!(tele.status, StatusConector::SessaoExpirada);
        assert_eq!(tele.plataforma, Plataforma::SalaDoFuturo);
        assert_eq!(tele.registrado_em_ms, 1_000);
        assert_eq!(
            tele.detalhe.as_deref(),
            Some("Sessão expirada: redirecionado para https://login.exemplo.org/entrar")
        );
    }

    #[tokio::test]
    async fn coleta_ok_preserva_metadata_e_cursor() {
        let (resultado, tele) =
            coletar_com_telemetria(&DcFalso, Some("2026-11-01T00:00:00Z"), 5).await;
        let lote = resultado.unwrap();
        assert_eq!(
            tele,
            Some(TelemetriaConector::ok(Plataforma::DiarioClasse, 5))
        );
        assert_eq!(lote.turmas[0].metadata_bruta["campo_extra"], 1);
        assert_eq!(lote.cursor.as_deref(), Some("2026-11-14T00:00:00Z"));
    }

    #[tokio::test]
    async fn operacao_nao_suportada_nao_mexe_na_saude_do_conector() {
        let (resultado, tele) = enviar_com_telemetria(&SdfSessaoVencida, &operacao(), 7).await;
        let erro = resultado.unwrap_err();
        assert!(matches!(erro, ErroConector::NaoSuportada(_)));
        assert_eq!(erro.acao(), AcaoAposErro::Desistir);
        assert_eq!(tele, None);

        let (resultado, tele) = enviar_com_telemetria(&DcFalso, &operacao(), 8).await;
        assert_eq!(resultado, Ok(ResultadoEnvio::JaAplicado));
        assert_eq!(tele.map(|t| t.status), Some(StatusConector::Ok));
    }

    #[tokio::test]
    async fn conectores_diferentes_na_mesma_lista() {
        let conectores: Vec<Box<dyn ConectorDinamico>> =
            vec![Box::new(SdfSessaoVencida), Box::new(DcFalso)];
        let plataformas: Vec<Plataforma> = conectores.iter().map(|c| c.plataforma_dyn()).collect();
        assert_eq!(
            plataformas,
            vec![Plataforma::SalaDoFuturo, Plataforma::DiarioClasse]
        );
        assert!(conectores[0].coletar_dyn(None).await.is_err());
        let lote = conectores[1]
            .coletar_dyn(Some("2026-11-01T00:00:00Z"))
            .await
            .unwrap();
        assert_eq!(lote.turmas.len(), 1);
        assert_eq!(
            conectores[1].enviar_dyn(&operacao()).await,
            Ok(ResultadoEnvio::JaAplicado)
        );
    }

    #[test]
    fn classificacao_dos_erros() {
        let casos = [
            (
                ErroConector::SessaoExpirada("x".into()),
                Some(StatusConector::SessaoExpirada),
                AcaoAposErro::AguardarLogin,
            ),
            (
                ErroConector::LayoutAlterado("x".into()),
                Some(StatusConector::LayoutAlterado),
                AcaoAposErro::Desistir,
            ),
            (
                ErroConector::Timeout("x".into()),
                Some(StatusConector::Timeout),
                AcaoAposErro::TentarDeNovo,
            ),
            (
                ErroConector::Rede("x".into()),
                Some(StatusConector::Falha),
                AcaoAposErro::TentarDeNovo,
            ),
            (
                ErroConector::Recusada("x".into()),
                None,
                AcaoAposErro::Desistir,
            ),
            (
                ErroConector::NaoSuportada("x".into()),
                None,
                AcaoAposErro::Desistir,
            ),
            (
                ErroConector::Interno("x".into()),
                Some(StatusConector::Falha),
                AcaoAposErro::Desistir,
            ),
        ];
        for (erro, status, acao) in casos {
            assert_eq!(erro.status(), status, "{erro}");
            assert_eq!(erro.acao(), acao, "{erro}");
        }
    }

    #[test]
    fn textos_de_status_e_plataforma() {
        for s in [
            StatusConector::Ok,
            StatusConector::SessaoExpirada,
            StatusConector::LayoutAlterado,
            StatusConector::Timeout,
            StatusConector::Falha,
        ] {
            assert_eq!(StatusConector::parse(s.as_str()), Ok(s));
            assert_eq!(serde_json::to_value(s).unwrap(), json!(s.as_str()));
        }
        for p in [
            Plataforma::DiarioClasse,
            Plataforma::SalaDoFuturo,
            Plataforma::PainelBi,
        ] {
            assert_eq!(Plataforma::parse(p.as_str()), Ok(p));
            assert_eq!(serde_json::to_value(p).unwrap(), json!(p.as_str()));
        }
        assert!(Plataforma::parse("sed").is_err());
    }

    #[test]
    fn sanitizacao_do_detalhe() {
        assert_eq!(
            sanitizar_detalhe(
                "HTTP 401 ao acessar https://portal.exemplo.org/api/turmas?token=abc#topo\nde novo"
            ),
            "HTTP 401 ao acessar https://portal.exemplo.org/api/turmas\nde novo"
        );
        assert_eq!(
            sanitizar_detalhe("sem url? normal#ok"),
            "sem url? normal#ok"
        );
        assert_eq!(
            sanitizar_detalhe("Aluno 000123456789 recusado; RA 000.123.456.789-0/SP"),
            "Aluno [nº oculto] recusado; RA [nº oculto]/SP"
        );
        assert_eq!(
            sanitizar_detalhe("aula 2026-11-14 turma 12345678 HTTP 403"),
            "aula 2026-11-14 turma 12345678 HTTP 403",
            "data, id curto e código HTTP ficam"
        );
        let longo = "á".repeat(LIMITE_DETALHE + 50);
        let curto = sanitizar_detalhe(&longo);
        assert_eq!(curto.chars().count(), LIMITE_DETALHE + 1);
        assert!(curto.ends_with('…'));
    }
}
