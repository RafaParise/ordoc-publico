//! Testes de rede do `MotorOpenAiCompat` + `Roteador` com um servidor HTTP falso em 127.0.0.1:0
//! (tokio puro, sem crate de mock). Só dados inventados.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use ia_nucleo::{
    Alcance, ConfigOpenAiCompat, ErroIa, EsquemaSaida, Higienizador, Motor, MotorOpenAiCompat,
    PedidoGeracao, Resposta, Roteador, TipoTarefa,
};
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Clone)]
enum Comportamento {
    Demorar(Duration),
    Status(u16),
    Conteudo(String),
}

struct ServidorFalso {
    base: String,
    /// Corpos dos POST /v1/chat/completions recebidos.
    pedidos: Arc<Mutex<Vec<String>>>,
}

impl ServidorFalso {
    fn pedidos(&self) -> Vec<String> {
        self.pedidos.lock().unwrap().clone()
    }
}

fn envelope(conteudo: &str) -> String {
    serde_json::json!({
        "choices": [{ "index": 0, "message": { "role": "assistant", "content": conteudo } }],
        "usage": { "prompt_tokens": 3, "completion_tokens": 2 }
    })
    .to_string()
}

async fn ler_pedido(s: &mut TcpStream) -> (String, String) {
    let mut buf = Vec::new();
    let mut pedaco = [0u8; 4096];
    let fim_cab = loop {
        let n = s.read(&mut pedaco).await.unwrap_or(0);
        if n == 0 {
            return (String::new(), String::new());
        }
        buf.extend_from_slice(&pedaco[..n]);
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p + 4;
        }
    };
    let cab = String::from_utf8_lossy(&buf[..fim_cab]).to_string();
    let tamanho = cab
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    while buf.len() < fim_cab + tamanho {
        let n = s.read(&mut pedaco).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&pedaco[..n]);
    }
    let corpo = String::from_utf8_lossy(&buf[fim_cab..]).to_string();
    (cab, corpo)
}

async fn subir(comp: Comportamento) -> ServidorFalso {
    let ouvinte = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endereco = ouvinte.local_addr().unwrap();
    let pedidos = Arc::new(Mutex::new(Vec::new()));
    let registro = pedidos.clone();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = ouvinte.accept().await {
            let comp = comp.clone();
            let registro = registro.clone();
            tokio::spawn(async move {
                let (cab, corpo) = ler_pedido(&mut s).await;
                let (status, body) = if cab.starts_with("GET /v1/models") {
                    (200, r#"{"object":"list","data":[]}"#.to_owned())
                } else if cab.starts_with("POST /v1/chat/completions") {
                    registro.lock().unwrap().push(corpo);
                    match comp {
                        Comportamento::Demorar(d) => {
                            tokio::time::sleep(d).await;
                            (200, envelope("{}"))
                        }
                        Comportamento::Status(c) => (c, r#"{"error":"falso"}"#.to_owned()),
                        Comportamento::Conteudo(t) => (200, envelope(&t)),
                    }
                } else {
                    (404, "{}".to_owned())
                };
                let resp = format!(
                    "HTTP/1.1 {status} Falso\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                     Connection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(resp.as_bytes()).await;
                let _ = s.shutdown().await;
            });
        }
    });
    ServidorFalso {
        base: format!("http://{endereco}/v1"),
        pedidos,
    }
}

fn motor(id: &str, alcance: Alcance, base: &str) -> Arc<MotorOpenAiCompat> {
    let cfg = ConfigOpenAiCompat::novo(id, alcance, base, "modelo-falso")
        .com_tempo_limite(Duration::from_millis(400));
    Arc::new(MotorOpenAiCompat::novo(cfg).unwrap())
}

fn hig() -> Higienizador {
    Higienizador::novo().unwrap().com_nomes(["Joana Teste Silva"])
}

#[derive(Debug, Deserialize)]
struct Rotulo {
    intencao: String,
}

fn pedido_rotulo(texto: &str) -> PedidoGeracao {
    PedidoGeracao::novo(texto).com_esquema(EsquemaSaida::novo(
        "rotulo",
        serde_json::json!({
            "type": "object",
            "properties": { "intencao": { "type": "string" } },
            "required": ["intencao"]
        }),
    ))
}

const OK: &str = r#"{"intencao":"PresencaDiagnosticar"}"#;

#[tokio::test]
async fn timeout_troca_para_o_segundo_motor() {
    let lento = subir(Comportamento::Demorar(Duration::from_secs(5))).await;
    let bom = subir(Comportamento::Conteudo(OK.into())).await;
    let r = Roteador::novo(hig())
        .com_motor(motor("lento", Alcance::Local, &lento.base))
        .com_motor(motor("bom", Alcance::Local, &bom.base));
    let inicio = std::time::Instant::now();
    let resp: Resposta<Rotulo> = r
        .gerar_estruturado(TipoTarefa::Estreita, &pedido_rotulo("minha aula ficou branca"))
        .await
        .unwrap();
    assert_eq!(resp.motor, "bom");
    assert_eq!(resp.valor.intencao, "PresencaDiagnosticar");
    assert_eq!(lento.pedidos().len(), 1);
    assert!(inicio.elapsed() < Duration::from_secs(3), "o timeout por chamada não funcionou");
}

#[tokio::test]
async fn timeout_do_motor_vira_tempo_esgotado() {
    let lento = subir(Comportamento::Demorar(Duration::from_secs(5))).await;
    let m = motor("lento", Alcance::Local, &lento.base);
    let erro = m.gerar(&PedidoGeracao::novo("oi")).await.unwrap_err();
    assert!(matches!(erro, ErroIa::TempoEsgotado { ref motor } if motor == "lento"));
}

#[tokio::test]
async fn http_500_troca_para_o_segundo_motor() {
    let quebrado = subir(Comportamento::Status(500)).await;
    let bom = subir(Comportamento::Conteudo(OK.into())).await;
    let m = motor("quebrado", Alcance::Online, &quebrado.base);
    assert!(matches!(
        m.gerar(&PedidoGeracao::novo("oi")).await,
        Err(ErroIa::Motor { status: 500, .. })
    ));
    let r = Roteador::novo(hig())
        .com_motor(m)
        .com_motor(motor("bom", Alcance::Online, &bom.base));
    let resp: Resposta<Rotulo> = r
        .gerar_estruturado(TipoTarefa::TextoLivre, &pedido_rotulo("x"))
        .await
        .unwrap();
    assert_eq!(resp.motor, "bom");
    assert_eq!(quebrado.pedidos().len(), 2); // 1 direto acima + 1 pelo roteador
}

#[tokio::test]
async fn saida_fora_do_esquema_duas_vezes_troca_de_motor() {
    let ruim = subir(Comportamento::Conteudo("não sei responder em JSON".into())).await;
    let bom = subir(Comportamento::Conteudo(format!("```json\n{OK}\n```"))).await;
    let r = Roteador::novo(hig())
        .com_motor(motor("ruim", Alcance::Local, &ruim.base))
        .com_motor(motor("bom", Alcance::Local, &bom.base));
    let resp: Resposta<Rotulo> = r
        .gerar_estruturado(TipoTarefa::Estreita, &pedido_rotulo("x"))
        .await
        .unwrap();
    assert_eq!(resp.motor, "bom");
    assert_eq!(resp.tentativas, 3);

    let pedidos = ruim.pedidos();
    assert_eq!(pedidos.len(), 2);
    let primeiro: serde_json::Value = serde_json::from_str(&pedidos[0]).unwrap();
    assert_eq!(primeiro["model"], "modelo-falso");
    assert_eq!(primeiro["response_format"]["type"], "json_schema");
    assert_eq!(primeiro["response_format"]["json_schema"]["name"], "rotulo");
    let segundo: serde_json::Value = serde_json::from_str(&pedidos[1]).unwrap();
    let msgs = segundo["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 3);
    assert_eq!(msgs[1]["role"], "assistant");
    assert!(msgs[2]["content"].as_str().unwrap().contains("esquema JSON"));
}

#[tokio::test]
async fn corpo_http_do_motor_online_so_tem_texto_higienizado() {
    let online = subir(Comportamento::Conteudo(r#"{"intencao":"[ALUNO_1]"}"#.into())).await;
    let r = Roteador::novo(hig()).com_motor(motor("online", Alcance::Online, &online.base));
    let resp: Resposta<Rotulo> = r
        .gerar_estruturado(
            TipoTarefa::TextoLivre,
            &pedido_rotulo("Carta para JOANA TESTE SILVA, e-mail joana.inventada@exemplo.test, RA 123456789"),
        )
        .await
        .unwrap();
    let corpo = online.pedidos().join("\n").to_lowercase();
    assert!(!corpo.contains("joana"));
    assert!(!corpo.contains("123456789"));
    assert!(corpo.contains("[aluno_1]") && corpo.contains("[email_1]") && corpo.contains("[ra_1]"));
    assert_eq!(resp.restaurar().as_str(), r#"{"intencao":"JOANA TESTE SILVA"}"#);
}

#[tokio::test]
async fn motor_fora_do_ar_fica_indisponivel_e_o_roteador_desiste() {
    // Porta que acabou de ser liberada: ninguém escuta.
    let porta = {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        l.local_addr().unwrap().port()
    };
    let base = format!("http://127.0.0.1:{porta}/v1");
    let m = motor("morto", Alcance::Local, &base);
    assert!(!m.disponivel().await);
    let r = Roteador::novo(hig()).com_motor(m);
    let erro = r
        .gerar_texto(TipoTarefa::Estreita, &PedidoGeracao::novo("x"))
        .await
        .unwrap_err();
    assert!(matches!(erro, ErroIa::SemMotorDisponivel));

    let vivo = subir(Comportamento::Conteudo("ok".into())).await;
    assert!(motor("vivo", Alcance::Local, &vivo.base).disponivel().await);
}
