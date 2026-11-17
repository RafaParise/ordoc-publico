//! Motores de geração: a trait `Motor`, o `MotorOpenAiCompat` (qualquer servidor com
//! `/v1/chat/completions`: llama-server local, OmniRoute, endpoint do usuário) e o `MotorFalso` dos testes.
//!
//! Privacidade: `PedidoGeracao`, `Mensagem` e `Geracao` têm `Debug` manual que NÃO mostra o texto; a chave
//! de API fica em `Zeroizing<String>` e nunca aparece em `Debug`. Quem garante que só texto higienizado
//! chega a motor online é o `Roteador`.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Deserialize;
use zeroize::Zeroizing;

use crate::error::{ErroIa, Result};
use crate::esquema::EsquemaSaida;

/// Futuro "dyn" dos motores (mesmo padrão do `FuturoConector` do ordoc-core, sem crate async-trait).
pub type FuturoIa<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

/// Onde o motor roda. Só `Local` pode receber texto sem higienizar (e só em tarefa sensível).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alcance {
    /// Processo no próprio aparelho (127.0.0.1).
    Local,
    /// Qualquer outro destino (OmniRoute, nuvem, outro aparelho).
    Online,
}

/// Identificação do motor (vai em erro e telemetria; nunca tem texto do usuário).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerfilMotor {
    pub id: String,
    pub alcance: Alcance,
    pub modelo: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Papel {
    Sistema,
    Usuario,
    Assistente,
}

impl Papel {
    fn api(self) -> &'static str {
        match self {
            Papel::Sistema => "system",
            Papel::Usuario => "user",
            Papel::Assistente => "assistant",
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct Mensagem {
    pub papel: Papel,
    pub conteudo: String,
}

impl Mensagem {
    pub fn nova(papel: Papel, conteudo: impl Into<String>) -> Self {
        Self {
            papel,
            conteudo: conteudo.into(),
        }
    }
}

impl std::fmt::Debug for Mensagem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mensagem")
            .field("papel", &self.papel)
            .field("caracteres", &self.conteudo.chars().count())
            .finish()
    }
}

/// O que pedir ao motor.
#[derive(Clone, Default, PartialEq)]
pub struct PedidoGeracao {
    pub mensagens: Vec<Mensagem>,
    /// JSON Schema da saída (`response_format` json_schema). `None` = texto livre.
    pub esquema: Option<EsquemaSaida>,
    pub max_tokens: Option<u32>,
    pub temperatura: Option<f32>,
}

impl std::fmt::Debug for PedidoGeracao {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PedidoGeracao")
            .field("mensagens", &self.mensagens)
            .field("esquema", &self.esquema.as_ref().map(|e| e.nome.as_str()))
            .field("max_tokens", &self.max_tokens)
            .field("temperatura", &self.temperatura)
            .finish()
    }
}

impl PedidoGeracao {
    /// Pedido com uma mensagem do usuário.
    pub fn novo(usuario: impl Into<String>) -> Self {
        Self {
            mensagens: vec![Mensagem::nova(Papel::Usuario, usuario)],
            ..Self::default()
        }
    }

    /// Põe (ou troca) a mensagem de sistema no começo.
    pub fn com_sistema(mut self, sistema: impl Into<String>) -> Self {
        self.mensagens.retain(|m| m.papel != Papel::Sistema);
        self.mensagens.insert(0, Mensagem::nova(Papel::Sistema, sistema));
        self
    }

    pub fn com_esquema(mut self, esquema: EsquemaSaida) -> Self {
        self.esquema = Some(esquema);
        self
    }

    pub fn com_max_tokens(mut self, max: u32) -> Self {
        self.max_tokens = Some(max);
        self
    }

    pub fn com_temperatura(mut self, t: f32) -> Self {
        self.temperatura = Some(t);
        self
    }
}

/// Resposta crua de um motor.
#[derive(Clone)]
pub struct Geracao {
    pub texto: String,
    /// Id do perfil do motor que respondeu.
    pub motor: String,
    pub tokens_entrada: Option<u32>,
    pub tokens_saida: Option<u32>,
}

impl std::fmt::Debug for Geracao {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Geracao")
            .field("motor", &self.motor)
            .field("caracteres", &self.texto.chars().count())
            .field("tokens_entrada", &self.tokens_entrada)
            .field("tokens_saida", &self.tokens_saida)
            .finish()
    }
}

/// Um motor de geração. Versão "dyn" direta: dá para guardar `Arc<dyn Motor>` numa lista.
pub trait Motor: Send + Sync {
    fn perfil(&self) -> &PerfilMotor;
    fn gerar<'a>(&'a self, pedido: &'a PedidoGeracao) -> FuturoIa<'a, Geracao>;
    /// O motor está de pé agora? (rápido; pode usar cache curto)
    fn disponivel(&self) -> Pin<Box<dyn Future<Output = bool> + Send + '_>>;
}

// ---------------------------------------------------------------------------------------------------
// MotorOpenAiCompat
// ---------------------------------------------------------------------------------------------------

/// Configuração de um motor OpenAI-compatível. Endereços e chaves vêm de configuração local, nunca do código.
#[derive(Clone)]
pub struct ConfigOpenAiCompat {
    pub id: String,
    pub alcance: Alcance,
    /// Ex.: `http://127.0.0.1:8081/v1` ou `http://127.0.0.1:8081` (o `/v1` é acrescentado se faltar).
    pub base_url: String,
    pub modelo: String,
    pub chave: Option<Zeroizing<String>>,
    /// Tempo máximo de cada chamada de geração (envio + corpo da resposta).
    pub tempo_limite: Duration,
    /// Tempo máximo do `GET /v1/models` da checagem de disponibilidade.
    pub tempo_limite_disponivel: Duration,
    /// Quanto tempo o resultado da checagem vale (desenho: "no último minuto").
    pub validade_disponivel: Duration,
}

impl std::fmt::Debug for ConfigOpenAiCompat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfigOpenAiCompat")
            .field("id", &self.id)
            .field("alcance", &self.alcance)
            .field("base_url", &self.base_url)
            .field("modelo", &self.modelo)
            .field("chave", &self.chave.as_ref().map(|_| "<oculta>"))
            .field("tempo_limite", &self.tempo_limite)
            .finish_non_exhaustive()
    }
}

impl ConfigOpenAiCompat {
    /// Configuração com os padrões: 60 s por chamada, 800 ms na checagem, checagem vale 60 s.
    pub fn novo(
        id: impl Into<String>,
        alcance: Alcance,
        base_url: impl Into<String>,
        modelo: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            alcance,
            base_url: base_url.into(),
            modelo: modelo.into(),
            chave: None,
            tempo_limite: Duration::from_secs(60),
            tempo_limite_disponivel: Duration::from_millis(800),
            validade_disponivel: Duration::from_secs(60),
        }
    }

    pub fn com_chave(mut self, chave: Zeroizing<String>) -> Self {
        self.chave = Some(chave);
        self
    }

    pub fn com_tempo_limite(mut self, t: Duration) -> Self {
        self.tempo_limite = t;
        self
    }

    pub fn com_validade_disponivel(mut self, t: Duration) -> Self {
        self.validade_disponivel = t;
        self
    }
}

/// Motor para qualquer servidor com a API de chat da OpenAI (`POST /v1/chat/completions`).
pub struct MotorOpenAiCompat {
    perfil: PerfilMotor,
    url_chat: String,
    url_modelos: String,
    chave: Option<Zeroizing<String>>,
    tempo_limite: Duration,
    tempo_limite_disponivel: Duration,
    validade_disponivel: Duration,
    cliente: reqwest::Client,
    ultima_checagem: Mutex<Option<(Instant, bool)>>,
}

impl std::fmt::Debug for MotorOpenAiCompat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MotorOpenAiCompat")
            .field("perfil", &self.perfil)
            .field("url_chat", &self.url_chat)
            .field("chave", &self.chave.as_ref().map(|_| "<oculta>"))
            .finish_non_exhaustive()
    }
}

fn host_e_loopback(url: &reqwest::Url) -> bool {
    matches!(
        url.host_str(),
        Some("127.0.0.1") | Some("localhost") | Some("[::1]") | Some("::1")
    )
}

fn erro_de_rede(e: &reqwest::Error, motor: &str) -> ErroIa {
    if e.is_timeout() {
        ErroIa::TempoEsgotado {
            motor: motor.to_owned(),
        }
    } else if e.is_connect() {
        ErroIa::Http("conexão recusada ou destino inacessível")
    } else if e.is_body() || e.is_decode() {
        ErroIa::Http("falha ao ler o corpo da resposta")
    } else {
        ErroIa::Http("falha na requisição")
    }
}

#[derive(Deserialize)]
struct RespostaChat {
    #[serde(default)]
    choices: Vec<EscolhaChat>,
    usage: Option<UsoChat>,
}

#[derive(Deserialize)]
struct EscolhaChat {
    message: MensagemChat,
}

#[derive(Deserialize)]
struct MensagemChat {
    content: Option<String>,
}

#[derive(Deserialize)]
struct UsoChat {
    prompt_tokens: Option<u32>,
    completion_tokens: Option<u32>,
}

impl MotorOpenAiCompat {
    /// Cria o motor. Recusa motor `Local` fora de 127.0.0.1/localhost (o local pode ver dado sensível).
    pub fn novo(config: ConfigOpenAiCompat) -> Result<Self> {
        let base = config.base_url.trim().trim_end_matches('/').to_owned();
        let url = reqwest::Url::parse(&base).map_err(|_| ErroIa::Configuracao("endereço do motor inválido"))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(ErroIa::Configuracao("o endereço do motor precisa ser http ou https"));
        }
        let loopback = host_e_loopback(&url);
        if config.alcance == Alcance::Local && !loopback {
            return Err(ErroIa::Configuracao(
                "motor local precisa estar em 127.0.0.1 ou localhost",
            ));
        }
        let raiz = if base.ends_with("/v1") {
            base
        } else {
            format!("{base}/v1")
        };

        let mut construtor = reqwest::Client::builder().use_rustls_tls();
        if loopback {
            // Nada de proxy do sistema para o próprio aparelho.
            construtor = construtor.no_proxy();
        }
        let cliente = construtor
            .build()
            .map_err(|_| ErroIa::Configuracao("não foi possível criar o cliente HTTP"))?;

        Ok(Self {
            perfil: PerfilMotor {
                id: config.id,
                alcance: config.alcance,
                modelo: config.modelo,
            },
            url_chat: format!("{raiz}/chat/completions"),
            url_modelos: format!("{raiz}/models"),
            chave: config.chave,
            tempo_limite: config.tempo_limite,
            tempo_limite_disponivel: config.tempo_limite_disponivel,
            validade_disponivel: config.validade_disponivel,
            cliente,
            ultima_checagem: Mutex::new(None),
        })
    }

    fn montar_corpo(&self, p: &PedidoGeracao) -> serde_json::Value {
        let mensagens: Vec<serde_json::Value> = p
            .mensagens
            .iter()
            .map(|m| serde_json::json!({ "role": m.papel.api(), "content": m.conteudo }))
            .collect();
        let mut corpo = serde_json::json!({
            "model": self.perfil.modelo,
            "messages": mensagens,
            "stream": false,
        });
        if let Some(obj) = corpo.as_object_mut() {
            if let Some(t) = p.temperatura {
                obj.insert("temperature".into(), serde_json::json!(t));
            }
            if let Some(m) = p.max_tokens {
                obj.insert("max_tokens".into(), serde_json::json!(m));
            }
            if let Some(e) = &p.esquema {
                obj.insert(
                    "response_format".into(),
                    serde_json::json!({
                        "type": "json_schema",
                        "json_schema": { "name": e.nome, "strict": true, "schema": e.esquema },
                    }),
                );
            }
        }
        corpo
    }

    async fn gerar_interno(&self, p: &PedidoGeracao) -> Result<Geracao> {
        let id = self.perfil.id.as_str();
        let mut req = self.cliente.post(&self.url_chat).json(&self.montar_corpo(p));
        if let Some(chave) = &self.chave {
            req = req.bearer_auth(chave.as_str());
        }
        let envio = async {
            let resp = req.send().await.map_err(|e| erro_de_rede(&e, id))?;
            let status = resp.status();
            if !status.is_success() {
                return Err(ErroIa::Motor {
                    motor: id.to_owned(),
                    status: status.as_u16(),
                });
            }
            resp.bytes().await.map_err(|e| erro_de_rede(&e, id))
        };
        let bytes = tokio::time::timeout(self.tempo_limite, envio)
            .await
            .map_err(|_| ErroIa::TempoEsgotado {
                motor: id.to_owned(),
            })??;

        let resposta: RespostaChat = serde_json::from_slice(&bytes)
            .map_err(|_| ErroIa::RespostaInvalida("o corpo não é o JSON da API de chat"))?;
        let texto = resposta
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .ok_or(ErroIa::RespostaInvalida("resposta sem choices[0].message.content"))?;
        Ok(Geracao {
            texto,
            motor: id.to_owned(),
            tokens_entrada: resposta.usage.as_ref().and_then(|u| u.prompt_tokens),
            tokens_saida: resposta.usage.as_ref().and_then(|u| u.completion_tokens),
        })
    }

    async fn checar(&self) -> bool {
        let em_cache = self
            .ultima_checagem
            .lock()
            .ok()
            .and_then(|g| *g)
            .filter(|(quando, _)| quando.elapsed() < self.validade_disponivel)
            .map(|(_, ok)| ok);
        if let Some(ok) = em_cache {
            return ok;
        }
        let mut req = self.cliente.get(&self.url_modelos);
        if let Some(chave) = &self.chave {
            req = req.bearer_auth(chave.as_str());
        }
        let ok = matches!(
            tokio::time::timeout(self.tempo_limite_disponivel, req.send()).await,
            Ok(Ok(resp)) if resp.status().is_success()
        );
        if let Ok(mut g) = self.ultima_checagem.lock() {
            *g = Some((Instant::now(), ok));
        }
        ok
    }

    /// Esquece o resultado da última checagem de disponibilidade.
    pub fn esquecer_checagem(&self) {
        if let Ok(mut g) = self.ultima_checagem.lock() {
            *g = None;
        }
    }
}

impl Motor for MotorOpenAiCompat {
    fn perfil(&self) -> &PerfilMotor {
        &self.perfil
    }

    fn gerar<'a>(&'a self, pedido: &'a PedidoGeracao) -> FuturoIa<'a, Geracao> {
        Box::pin(self.gerar_interno(pedido))
    }

    fn disponivel(&self) -> Pin<Box<dyn Future<Output = bool> + Send + '_>> {
        Box::pin(self.checar())
    }
}

// ---------------------------------------------------------------------------------------------------
// MotorFalso
// ---------------------------------------------------------------------------------------------------

/// O que o `MotorFalso` devolve em cada chamada.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RespostaFalsa {
    Texto(String),
    ErroHttp(u16),
    TempoEsgotado,
}

/// Motor de mentira para testes: devolve as respostas da fila, em ordem, e grava cada pedido recebido.
/// Fila vazia = `ErroIa::Motor { status: 503 }`.
pub struct MotorFalso {
    perfil: PerfilMotor,
    respostas: Mutex<VecDeque<RespostaFalsa>>,
    recebidos: Mutex<Vec<PedidoGeracao>>,
    de_pe: AtomicBool,
}

impl std::fmt::Debug for MotorFalso {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MotorFalso")
            .field("perfil", &self.perfil)
            .field("chamadas", &self.chamadas())
            .finish_non_exhaustive()
    }
}

impl MotorFalso {
    pub fn novo(id: impl Into<String>, alcance: Alcance) -> Self {
        Self {
            perfil: PerfilMotor {
                id: id.into(),
                alcance,
                modelo: "falso".to_owned(),
            },
            respostas: Mutex::new(VecDeque::new()),
            recebidos: Mutex::new(Vec::new()),
            de_pe: AtomicBool::new(true),
        }
    }

    pub fn com_respostas(self, respostas: impl IntoIterator<Item = RespostaFalsa>) -> Self {
        if let Ok(mut fila) = self.respostas.lock() {
            fila.extend(respostas);
        }
        self
    }

    pub fn com_disponivel(self, de_pe: bool) -> Self {
        self.definir_disponivel(de_pe);
        self
    }

    pub fn definir_disponivel(&self, de_pe: bool) {
        self.de_pe.store(de_pe, Ordering::SeqCst);
    }

    /// Cópia dos pedidos recebidos, na ordem.
    pub fn recebidos(&self) -> Vec<PedidoGeracao> {
        self.recebidos.lock().map(|r| r.clone()).unwrap_or_default()
    }

    pub fn chamadas(&self) -> usize {
        self.recebidos.lock().map(|r| r.len()).unwrap_or(0)
    }
}

impl Motor for MotorFalso {
    fn perfil(&self) -> &PerfilMotor {
        &self.perfil
    }

    fn gerar<'a>(&'a self, pedido: &'a PedidoGeracao) -> FuturoIa<'a, Geracao> {
        Box::pin(async move {
            if let Ok(mut r) = self.recebidos.lock() {
                r.push(pedido.clone());
            }
            let proxima = self.respostas.lock().ok().and_then(|mut f| f.pop_front());
            let id = self.perfil.id.clone();
            match proxima {
                Some(RespostaFalsa::Texto(texto)) => Ok(Geracao {
                    texto,
                    motor: id,
                    tokens_entrada: None,
                    tokens_saida: None,
                }),
                Some(RespostaFalsa::ErroHttp(status)) => Err(ErroIa::Motor { motor: id, status }),
                Some(RespostaFalsa::TempoEsgotado) => Err(ErroIa::TempoEsgotado { motor: id }),
                None => Err(ErroIa::Motor {
                    motor: id,
                    status: 503,
                }),
            }
        })
    }

    fn disponivel(&self) -> Pin<Box<dyn Future<Output = bool> + Send + '_>> {
        let de_pe = self.de_pe.load(Ordering::SeqCst);
        Box::pin(async move { de_pe })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn motor_local_fora_do_aparelho_e_recusado() {
        let cfg = ConfigOpenAiCompat::novo("x", Alcance::Local, "http://10.0.0.5:8081/v1", "m");
        assert!(matches!(MotorOpenAiCompat::novo(cfg), Err(ErroIa::Configuracao(_))));
        let cfg = ConfigOpenAiCompat::novo("x", Alcance::Local, "http://127.0.0.1:8081", "m");
        let m = MotorOpenAiCompat::novo(cfg).unwrap();
        assert_eq!(m.url_chat, "http://127.0.0.1:8081/v1/chat/completions");
        assert_eq!(m.url_modelos, "http://127.0.0.1:8081/v1/models");
    }

    #[test]
    fn debug_nao_mostra_chave_nem_texto() {
        let cfg = ConfigOpenAiCompat::novo("on", Alcance::Online, "https://exemplo.invalid/v1", "m")
            .com_chave(Zeroizing::new("sk-segredo-inventado".to_owned()));
        let m = MotorOpenAiCompat::novo(cfg.clone()).unwrap();
        let p = PedidoGeracao::novo("texto do usuario secreto").com_sistema("sistema secreto");
        let g = Geracao {
            texto: "resposta secreta".into(),
            motor: "on".into(),
            tokens_entrada: None,
            tokens_saida: None,
        };
        let dbg = format!("{cfg:?} {m:?} {p:?} {g:?}");
        assert!(!dbg.contains("sk-segredo"));
        assert!(!dbg.contains("secret"));
    }

    #[test]
    fn corpo_leva_o_esquema_em_response_format() {
        let cfg = ConfigOpenAiCompat::novo("x", Alcance::Local, "http://127.0.0.1:1/v1", "qwen");
        let m = MotorOpenAiCompat::novo(cfg).unwrap();
        let p = PedidoGeracao::novo("oi")
            .com_esquema(EsquemaSaida::novo("rotulo", serde_json::json!({"type": "object"})))
            .com_max_tokens(32);
        let corpo = m.montar_corpo(&p);
        assert_eq!(corpo["model"], "qwen");
        assert_eq!(corpo["messages"][0]["role"], "user");
        assert_eq!(corpo["response_format"]["type"], "json_schema");
        assert_eq!(corpo["response_format"]["json_schema"]["name"], "rotulo");
        assert_eq!(corpo["max_tokens"], 32);
    }

    #[tokio::test]
    async fn motor_falso_segue_a_fila_e_grava_o_recebido() {
        let m = MotorFalso::novo("f", Alcance::Online).com_respostas([
            RespostaFalsa::ErroHttp(500),
            RespostaFalsa::Texto("ok".into()),
        ]);
        let p = PedidoGeracao::novo("oi");
        assert!(matches!(m.gerar(&p).await, Err(ErroIa::Motor { status: 500, .. })));
        assert_eq!(m.gerar(&p).await.unwrap().texto, "ok");
        assert!(matches!(m.gerar(&p).await, Err(ErroIa::Motor { status: 503, .. })));
        assert_eq!(m.chamadas(), 3);
        assert_eq!(m.recebidos()[0], p);
    }
}
