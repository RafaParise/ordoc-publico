//! Ponte com o back: `window.__TAURI__.core.invoke` (o Tauri expõe com `withGlobalTauri`).
//!
//! Fora do app (navegador comum, `trunk serve`, testes de tela) não há `__TAURI__`: o Painel e a EFAPE
//! são montados aqui mesmo com o exemplo SINTÉTICO (o domínio `ordoc-core` compila em WASM); Pedidos
//! vem sem réplica e a IA pede para abrir o app.

use ordoc_core::visao::{
    montar_painel, visao_painel, EntradaPainel, EncontroVisao, SemanaVisao, VisaoEfape, VisaoPedido,
    VisaoPedidos,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "core"], js_name = invoke, catch)]
    async fn invoke_tauri(cmd: &str, args: JsValue) -> Result<JsValue, JsValue>;
}

pub const SO_NO_APP: &str = "Isto só funciona dentro do app Ordoc (aqui é a prévia no navegador).";
const FONTE_PREVIA: &str = "prévia no navegador com o exemplo sintético";

const EXEMPLO_PAINEL: &str = include_str!("../exemplos/painel.json");
const EXEMPLO_PEDIDOS: &str = include_str!("../exemplos/pedidos.json");
const EXEMPLO_EFAPE: &str = include_str!("../exemplos/efape.json");

pub fn tem_tauri() -> bool {
    js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("__TAURI__"))
        .map(|v| !v.is_undefined() && !v.is_null())
        .unwrap_or(false)
}

/// Chama um comando do back. Os argumentos viram objeto JS simples (o Tauri não aceita `Map`).
pub async fn chamar<T: DeserializeOwned>(cmd: &str, args: impl Serialize) -> Result<T, String> {
    if !tem_tauri() {
        return previa(cmd);
    }
    let a = args
        .serialize(&serde_wasm_bindgen::Serializer::json_compatible())
        .map_err(|e| e.to_string())?;
    match invoke_tauri(cmd, a).await {
        Ok(v) => serde_wasm_bindgen::from_value(v).map_err(|e| format!("resposta fora do contrato: {e}")),
        Err(e) => Err(e.as_string().unwrap_or_else(|| "o Ordoc não respondeu".into())),
    }
}

#[derive(Deserialize)]
struct PedidosExemplo {
    pedidos: Vec<PedidoExemplo>,
}

#[derive(Deserialize)]
struct PedidoExemplo {
    canal: String,
    protocolo: String,
    assunto: String,
    aberto_em: String,
    situacao: String,
    #[serde(default)]
    respondido_em: Option<String>,
    #[serde(default)]
    resposta: Option<String>,
}

#[derive(Deserialize)]
struct EfapeExemplo {
    #[serde(default)]
    semanas: Vec<SemanaVisao>,
    #[serde(default)]
    encontros: Vec<EncontroVisao>,
}

fn json<T: DeserializeOwned>(texto: &str) -> Result<T, String> {
    serde_json::from_str(texto).map_err(|e| e.to_string())
}

/// Converte para `T` passando por JSON (os tipos da prévia são os mesmos do contrato).
fn como<T: DeserializeOwned>(valor: impl Serialize) -> Result<T, String> {
    serde_json::to_value(valor)
        .and_then(serde_json::from_value)
        .map_err(|e| e.to_string())
}

fn previa<T: DeserializeOwned>(cmd: &str) -> Result<T, String> {
    match cmd {
        "painel" => {
            let e: EntradaPainel = json(EXEMPLO_PAINEL)?;
            let p = montar_painel(&e).map_err(|e| e.to_string())?;
            como(visao_painel(&p, &e.hoje, FONTE_PREVIA).map_err(|e| e.to_string())?)
        }
        "pedidos" => {
            let e: PedidosExemplo = json(EXEMPLO_PEDIDOS)?;
            como(VisaoPedidos {
                fonte: format!("{FONTE_PREVIA} (as réplicas só aparecem no app)"),
                pedidos: e
                    .pedidos
                    .into_iter()
                    .map(|p| VisaoPedido {
                        canal: p.canal,
                        protocolo: p.protocolo,
                        assunto: p.assunto,
                        aberto_em: p.aberto_em,
                        situacao: p.situacao,
                        respondido_em: p.respondido_em,
                        resposta: p.resposta,
                        replica: None,
                    })
                    .collect(),
            })
        }
        "efape" => {
            let e: EfapeExemplo = json(EXEMPLO_EFAPE)?;
            como(VisaoEfape {
                fonte: FONTE_PREVIA.into(),
                semanas: e.semanas,
                encontros: e.encontros,
                links: Vec::new(),
            })
        }
        "portais" => como(serde_json::json!([
            {"id": "Sed", "nome": "SED", "perfil": "sed", "url": "https://sed.educacao.sp.gov.br/"},
            {"id": "AvaEfape", "nome": "AVA-EFAPE", "perfil": "sed", "url": "https://avaefape.educacao.sp.gov.br/my/"},
            {"id": "Outlook", "nome": "Outlook", "perfil": "microsoft", "url": "https://outlook.cloud.microsoft/mail/"},
            {"id": "TeamsWeb", "nome": "Teams (web)", "perfil": "microsoft", "url": "https://teams.microsoft.com/"}
        ])),
        "pasta_dados" => como("(no app: %APPDATA%\\com.ordoc.app\\dados)"),
        _ => Err(SO_NO_APP.into()),
    }
}

/// Copia para a área de transferência (navigator.clipboard.writeText), sem esperar o resultado.
pub fn copiar(texto: &str) {
    let global = js_sys::global();
    let Ok(nav) = js_sys::Reflect::get(&global, &JsValue::from_str("navigator")) else {
        return;
    };
    let Ok(clip) = js_sys::Reflect::get(&nav, &JsValue::from_str("clipboard")) else {
        return;
    };
    if let Ok(f) = js_sys::Reflect::get(&clip, &JsValue::from_str("writeText")) {
        if let Ok(f) = f.dyn_into::<js_sys::Function>() {
            let _ = f.call1(&clip, &JsValue::from_str(texto));
        }
    }
}

/// Hoje (relógio do aparelho), AAAA-MM-DD.
pub fn hoje() -> String {
    let d = js_sys::Date::new_0();
    format!("{:04}-{:02}-{:02}", d.get_full_year(), d.get_month() + 1, d.get_date())
}

/// Tema salvo ("auto", "claro", "escuro"); qualquer falha do armazenamento = "auto".
pub fn tema_salvo() -> String {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item("ordoc.tema").ok().flatten())
        .filter(|t| t == "claro" || t == "escuro")
        .unwrap_or_else(|| "auto".into())
}

pub fn aplicar_tema(tema: &str) {
    if let Some(w) = web_sys::window() {
        if let Some(el) = w.document().and_then(|d| d.document_element()) {
            let _ = if tema == "auto" {
                el.remove_attribute("data-tema")
            } else {
                el.set_attribute("data-tema", tema)
            };
        }
        if let Some(s) = w.local_storage().ok().flatten() {
            let _ = s.set_item("ordoc.tema", tema);
        }
    }
}
