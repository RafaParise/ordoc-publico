//! Interface do Ordoc (Leptos CSR). Só desenha o que os comandos do back devolvem
//! (`ordoc_core::visao`); não acessa rede, banco nem disco. Nada aqui escreve em portal: as ações
//! abrem a janela do portal oficial, e quem confirma é o professor.

mod correcao;
mod ponte;

use leptos::prelude::*;
use leptos::task::spawn_local;
use ordoc_core::visao::{
    EncontroVisao, ItemPainelVisao, ProgressoVisao, SemanaVisao, Urgencia, VisaoDuvida, VisaoEfape,
    VisaoEntender, VisaoEstadoIa, VisaoPainel, VisaoPedido, VisaoPedidos, VisaoPonte, VisaoEp, VisaoPresenca, VisaoCaixa, VisaoOportunidades,
    VisaoPortalAlunos, VisaoTelemetria,
};
use serde::{Deserialize, Serialize};

use ponte::{chamar, copiar};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tela {
    Painel,
    Pedidos,
    Ia,
    Efape,
    Portais,
    Ep,
    Corrigir,
    Oportunidades,
    PainelAlunos,
    Notas,
    Revisar,
}

impl Tela {
    const MENU: [(Tela, &'static str, &'static str); 11] = [
        (Tela::Painel, "Painel", "o que falta para ficar 100%"),
        (Tela::Pedidos, "Pedidos e respostas", "protocolos, respostas e réplicas"),
        (Tela::Ia, "IA do Ordoc", "peça em português"),
        (Tela::Efape, "EFAPE e Multiplica", "semanas, vídeo flutuante, encontros"),
        (Tela::Portais, "Portais", "sistemas oficiais na conta certa"),
        (Tela::Ep, "Plataforma EP", "entregas e gabaritos"),
        (Tela::Corrigir, "Corrigir", "Tarefa SP: a resposta na tela, IA ajuda, lança com o seu sim"),
        (Tela::Oportunidades, "Oportunidades", "concursos, seleções, aplicação de provas, atribuição"),
        (Tela::PainelAlunos, "Painel do aluno", "um link cifrado por aluno: o que fez e o que falta"),
        (Tela::Notas, "Notas e frequência", "síntese e risco"),
        (Tela::Revisar, "Revisar", "telemetria: o que o app fez e o que falhou"),
    ];

    fn do_id(id: &str) -> Option<Tela> {
        Some(match id {
            "painel" => Tela::Painel,
            "pedidos" => Tela::Pedidos,
            "ia" => Tela::Ia,
            "efape" => Tela::Efape,
            "portais" => Tela::Portais,
            "ep" => Tela::Ep,
            "corrigir" => Tela::Corrigir,
            "oportunidades" => Tela::Oportunidades,
            "painel-alunos" => Tela::PainelAlunos,
            "notas" => Tela::Notas,
            "revisar" => Tela::Revisar,
            _ => return None,
        })
    }
}

#[derive(Serialize)]
struct Nada {}

/// Carrega um comando sem argumentos num sinal (None = carregando).
fn carregar<T>(cmd: &'static str) -> ReadSignal<Option<Result<T, String>>>
where
    T: for<'de> Deserialize<'de> + Send + Sync + 'static,
{
    let (s, set) = signal(None);
    spawn_local(async move { set.set(Some(chamar::<T>(cmd, Nada {}).await)) });
    s
}

fn abrir_portal(portal: &str, url: Option<String>, aviso: WriteSignal<String>) {
    #[derive(Serialize)]
    struct Args {
        portal: String,
        url: Option<String>,
    }
    let args = Args {
        portal: portal.to_owned(),
        url,
    };
    spawn_local(async move {
        match chamar::<()>("abrir_portal", args).await {
            Ok(()) => aviso.set(String::new()),
            Err(e) => aviso.set(e),
        }
    });
}

fn aviso_fonte(fonte: String) -> impl IntoView {
    let exemplo = !fonte.starts_with("seus dados");
    view! { <p class="fonte" class:exemplo=exemplo>"Fonte: " {fonte}</p> }
}

fn barra(p: &ProgressoVisao) -> impl IntoView {
    let pct = p.percentual.clamp(0.0, 100.0);
    view! {
        <div class="area">
            <div class="area-topo">
                <span>{p.rotulo.clone()}</span>
                <span class="mono">{format!("{}/{} · {:.0}%", p.encerradas, p.total, pct)}</span>
            </div>
            <div class="barra"><div class="barra-cheia" style=format!("width:{pct:.1}%")></div></div>
        </div>
    }
}

// ---------------------------------------------------------------------------------------------
// Painel
// ---------------------------------------------------------------------------------------------

fn classe_urgencia(u: Urgencia) -> &'static str {
    match u {
        Urgencia::Vencido => "item vencido",
        Urgencia::Vencendo => "item vencendo",
        Urgencia::Bloqueia => "item bloqueia",
        Urgencia::Normal => "item",
    }
}

fn item_painel(i: ItemPainelVisao) -> impl IntoView {
    let travada = (!i.travada_por.is_empty()).then(|| {
        view! { <p class="travada">"Travada por: " {i.travada_por.join("; ")}</p> }
    });
    let detalhe = (!i.detalhe.is_empty()).then(|| view! { <p class="detalhe">{i.detalhe.clone()}</p> });
    let protocolos = i
        .protocolos
        .iter()
        .map(|p| view! { <li class="mono">{p.clone()}</li> })
        .collect::<Vec<_>>();
    view! {
        <article class=classe_urgencia(i.urgencia)>
            <header>
                <h3>{i.titulo.clone()}</h3>
                <span class="selo">{i.prioridade.clone()}</span>
            </header>
            <p class="meta">{format!("{} · {} · {}", i.area, i.sistema, i.estado)}</p>
            {detalhe}
            <p class="acao">"Próximo passo: " {i.acao.clone()}</p>
            <ul class="protocolos">{protocolos}</ul>
            {travada}
        </article>
    }
}

fn painel_view(v: VisaoPainel) -> impl IntoView {
    let pct = v.geral.percentual.clamp(0.0, 100.0);
    let areas = v.areas.iter().map(barra).collect::<Vec<_>>();
    let agenda = v.agenda.into_iter().map(item_painel).collect::<Vec<_>>();
    let cadeias = v
        .cadeias
        .into_iter()
        .map(|c| view! { <li>{c.join(" → ")}</li> })
        .collect::<Vec<_>>();
    let tem_cadeias = !cadeias.is_empty();
    view! {
        {aviso_fonte(v.fonte)}
        <section class="resumo">
            <div class="anel" style=format!("--pct:{pct:.1}")>
                <span class="anel-num">{format!("{pct:.0}%")}</span>
                <span class="anel-txt">"regularizado"</span>
            </div>
            <div class="areas">{areas}</div>
        </section>
        <h2>{format!("Próximas ações ({}) · hoje {}", agenda.len(), v.hoje)}</h2>
        <div class="lista">{agenda}</div>
        <Show when=move || tem_cadeias>
            <h2>"Cadeias de bloqueio"</h2>
        </Show>
        <ul class="cadeias">{cadeias}</ul>
    }
}

#[component]
fn TelaPainel() -> impl IntoView {
    let dados = carregar::<VisaoPainel>("painel");
    let painel = move || match dados.get() {
        None => view! { <p class="carregando">"Carregando o painel…"</p> }.into_any(),
        Some(Err(e)) => view! { <p class="erro">{e}</p> }.into_any(),
        Some(Ok(v)) => painel_view(v).into_any(),
    };
    view! {
        <CartaoPresenca/>
        {painel}
    }
}

/// Professor Presente: a cor de cada dia do mês (verde = frequência e registro próprios lançados).
#[component]
fn CartaoPresenca() -> impl IntoView {
    let (dados, set_dados) = signal::<Option<Result<VisaoPresenca, String>>>(None);
    let recarregar = move || {
        spawn_local(async move { set_dados.set(Some(chamar::<VisaoPresenca>("presenca", Nada {}).await)) });
    };
    recarregar();
    let (atualizando, set_atualizando) = signal(false);
    let (situacao, set_situacao) = signal(String::new());
    let atualizar = move |_| {
        set_atualizando.set(true);
        set_situacao.set("Lendo o calendário na SED…".to_owned());
        spawn_local(async move {
            match chamar::<String>("atualizar_presenca", Nada {}).await {
                Ok(m) => {
                    set_situacao.set(m);
                    recarregar();
                }
                Err(e) => set_situacao.set(e),
            }
            set_atualizando.set(false);
        });
    };
    let corpo = move || match dados.get() {
        Some(Ok(v)) => {
            let fora_do_verde: Vec<String> = v
                .dias
                .iter()
                .filter(|d| d.cor != "verde" && d.cor != "cinza")
                .map(|d| format!("{} {}", d.data.get(8..10).unwrap_or_default(), d.cor))
                .collect();
            let verdes = v.dias.iter().filter(|d| d.cor == "verde").count();
            let resumo = if fora_do_verde.is_empty() {
                format!("{verdes} dias verdes, nenhum dia com aula fora do verde")
            } else {
                format!("{verdes} verdes; fora do verde: {}", fora_do_verde.join(", "))
            };
            view! {
                {aviso_fonte(v.fonte)}
                <p class="detalhe">{format!("Mês {}: {resumo}", v.mes)}</p>
            }
            .into_any()
        }
        Some(Err(e)) => view! { <p class="erro">{e}</p> }.into_any(),
        None => ().into_any(),
    };
    view! {
        <article class="item">
            <header>
                <h3>"Professor Presente"</h3>
                <button class="botao" on:click=atualizar disabled=move || atualizando.get()>
                    {move || if atualizando.get() { "Atualizando…" } else { "Atualizar da SED" }}
                </button>
            </header>
            {corpo}
            <p class="meta">{move || situacao.get()}</p>
            <p class="meta">"Abra antes, na SED, a página do Professor Presente. Dia amarelo ou branco: veja Pedidos e a IA (\"minha aula ficou branca\")."</p>
        </article>
    }
}

// ---------------------------------------------------------------------------------------------
// Pedidos e respostas
// ---------------------------------------------------------------------------------------------

fn pedido_view(p: VisaoPedido) -> impl IntoView {
    let resposta = p.resposta.clone().map(|r| {
        view! {
            <blockquote>
                <span class="meta">{format!("Resposta de {}", p.respondido_em.clone().unwrap_or_default())}</span>
                <p>{r}</p>
            </blockquote>
        }
    });
    let replica = match p.replica.clone() {
        None if p.resposta.is_some() && ponte::tem_tauri() => Some(
            view! { <p class="detalhe">"A regra não reconheceu esta resposta: leia e escolha a réplica."</p> }
                .into_any(),
        ),
        None => None,
        Some(r) => {
            let (texto, set_texto) = signal(r.modelo.clone().unwrap_or_default());
            let tem_modelo = r.modelo.is_some();
            Some(
                view! {
                    <div class="replica">
                        <p><span class="selo destaque">{r.codigo.clone()}</span> " " {r.rotulo.clone()}</p>
                        <p class="detalhe">{r.orientacao.clone()}</p>
                        <Show when=move || tem_modelo>
                            <label class="rotulo">"Modelo (troque os [MARCADORES] antes de enviar):"</label>
                            <textarea
                                rows="7"
                                prop:value=move || texto.get()
                                on:input=move |ev| set_texto.set(event_target_value(&ev))
                            ></textarea>
                            <button class="botao" on:click=move |_| copiar(&texto.get_untracked())>
                                "Copiar texto"
                            </button>
                        </Show>
                        <details>
                            <summary>"Limites de toda réplica"</summary>
                            <p class="detalhe">{r.limites.clone()}</p>
                        </details>
                    </div>
                }
                .into_any(),
            )
        }
    };
    view! {
        <article class="item">
            <header>
                <h3>{p.assunto.clone()}</h3>
                <span class="selo">{p.situacao.clone()}</span>
            </header>
            <p class="meta mono">{format!("{} · {} · aberto em {}", p.canal, p.protocolo, p.aberto_em)}</p>
            {resposta}
            {replica}
        </article>
    }
}

#[component]
fn TelaPedidos() -> impl IntoView {
    let (dados, set_dados) = signal::<Option<Result<VisaoPedidos, String>>>(None);
    let recarregar = move || {
        spawn_local(async move { set_dados.set(Some(chamar::<VisaoPedidos>("pedidos", Nada {}).await)) });
    };
    recarregar();
    // "Atualizar": recarrega só a aba da Minha área do Fala.SP e lê a lista (só leitura).
    let (atualizando, set_atualizando) = signal(false);
    let (situacao, set_situacao) = signal(String::new());
    let atualizar = move |_| {
        set_atualizando.set(true);
        set_situacao.set("Lendo a Minha área do Fala.SP (o painel é lento: até 1 minuto)…".to_owned());
        spawn_local(async move {
            match chamar::<String>("atualizar_pedidos", Nada {}).await {
                Ok(m) => {
                    set_situacao.set(m);
                    recarregar();
                }
                Err(e) => set_situacao.set(e),
            }
            set_atualizando.set(false);
        });
    };
    let conteudo = move || match dados.get() {
        None => view! { <p class="carregando">"Carregando…"</p> }.into_any(),
        Some(Err(e)) => view! { <p class="erro">{e}</p> }.into_any(),
        Some(Ok(v)) => {
            let lista = v.pedidos.into_iter().map(pedido_view).collect::<Vec<_>>();
            view! {
                {aviso_fonte(v.fonte)}
                <p class="explica">
                    "Cada resposta recebida (Fala SP ou e-mail) vem com o tipo (R1 a R7) e a réplica sugerida. "
                    "Você revisa, completa e envia pela janela do portal: o Ordoc não envia sozinho."
                </p>
                <div class="lista">{lista}</div>
            }
            .into_any()
        }
    };
    view! {
        <p class="meta">
            <button class="botao primario" on:click=atualizar disabled=move || atualizando.get()>
                {move || if atualizando.get() { "Atualizando…" } else { "Atualizar do Fala.SP" }}
            </button>
            " " {move || situacao.get()}
        </p>
        <CaixaOutlook/>
        {conteudo}
    }
}

/// Caixa do Outlook (@professor): o que veio de órgão oficial, para não perder resposta de pedido.
#[component]
fn CaixaOutlook() -> impl IntoView {
    let (dados, set_dados) = signal::<Option<Result<VisaoCaixa, String>>>(None);
    let recarregar = move || {
        spawn_local(async move { set_dados.set(Some(chamar::<VisaoCaixa>("caixa", Nada {}).await)) });
    };
    recarregar();
    let (atualizando, set_atualizando) = signal(false);
    let (situacao, set_situacao) = signal(String::new());
    let atualizar = move |_| {
        set_atualizando.set(true);
        set_situacao.set("Lendo a lista da caixa (sem abrir mensagem)…".to_owned());
        spawn_local(async move {
            match chamar::<String>("atualizar_caixa", Nada {}).await {
                Ok(m) => {
                    set_situacao.set(m);
                    recarregar();
                }
                Err(e) => set_situacao.set(e),
            }
            set_atualizando.set(false);
        });
    };
    let corpo = move || match dados.get() {
        Some(Ok(v)) => {
            let oficiais = v
                .mensagens
                .iter()
                .filter(|m| m.oficial)
                .map(|m| {
                    let marca = if m.nao_lida { "● " } else { "" };
                    view! { <li>{format!("{marca}{} · {} · {}", m.data, m.dominio, m.assunto)}</li> }
                })
                .collect::<Vec<_>>();
            let nao_lidas = v.mensagens.iter().filter(|m| m.nao_lida).count();
            view! {
                {aviso_fonte(v.fonte)}
                <p class="detalhe">{format!("{} mensagens na lista, {nao_lidas} não lidas.", v.mensagens.len())}</p>
                {if oficiais.is_empty() {
                    view! { <p class="meta">"Nada de órgão oficial (*.sp.gov.br) na lista."</p> }.into_any()
                } else {
                    view! { <ul>{oficiais}</ul> }.into_any()
                }}
            }
            .into_any()
        }
        Some(Err(e)) => view! { <p class="erro">{e}</p> }.into_any(),
        None => ().into_any(),
    };
    view! {
        <article class="item">
            <header>
                <h3>"E-mail @professor (Outlook)"</h3>
                <button class="botao" on:click=atualizar disabled=move || atualizando.get()>
                    {move || if atualizando.get() { "Atualizando…" } else { "Atualizar do Outlook" }}
                </button>
            </header>
            {corpo}
            <p class="meta">{move || situacao.get()}</p>
            <p class="meta">"Só a lista visível: nada é aberto nem marcado como lido. Do remetente fica só o domínio."</p>
        </article>
    }
}

// ---------------------------------------------------------------------------------------------
// IA do Ordoc
// ---------------------------------------------------------------------------------------------

#[component]
fn TelaIa(ir_para: WriteSignal<Tela>) -> impl IntoView {
    let (texto, set_texto) = signal(String::new());
    let (resposta, set_resposta) = signal::<Option<Result<VisaoEntender, String>>>(None);
    let perguntar = move || {
        #[derive(Serialize)]
        struct Args {
            texto: String,
        }
        let t = texto.get_untracked();
        set_resposta.set(None);
        spawn_local(async move { set_resposta.set(Some(chamar("entender", Args { texto: t }).await)) });
    };
    // Dúvida respondida pela IA (online se houver; senão o modelo local).
    let (duvida, set_duvida) = signal::<Option<Result<VisaoDuvida, String>>>(None);
    let (pensando, set_pensando) = signal(false);
    let estado_ia = carregar::<VisaoEstadoIa>("estado_ia");
    let (aviso_ia, set_aviso_ia) = signal(String::new());
    let responder = move || {
        #[derive(Serialize)]
        struct Args {
            texto: String,
        }
        let t = texto.get_untracked();
        set_duvida.set(None);
        set_pensando.set(true);
        spawn_local(async move {
            set_duvida.set(Some(chamar("perguntar", Args { texto: t }).await));
            set_pensando.set(false);
        });
    };
    let ligar_local = move |_| {
        spawn_local(async move {
            match chamar::<String>("ligar_ia_local", Nada {}).await {
                Ok(m) | Err(m) => set_aviso_ia.set(m),
            }
        });
    };
    let escolher = move |intencao: String| {
        #[derive(Serialize)]
        struct Args {
            intencao: String,
        }
        spawn_local(async move { set_resposta.set(Some(chamar("escolher", Args { intencao }).await)) });
    };
    // Ponte com o Univo: pedidos recebidos (nunca executam sozinhos) e pareamento do pipe.
    let (ponte, set_ponte) = signal::<Option<Result<VisaoPonte, String>>>(None);
    let recarregar_ponte = move || {
        spawn_local(async move { set_ponte.set(Some(chamar::<VisaoPonte>("estado_ponte", Nada {}).await)) });
    };
    recarregar_ponte();
    let (codigo, set_codigo) = signal(String::new());
    let parear = move |_| {
        spawn_local(async move {
            match chamar::<String>("parear_ponte", Nada {}).await {
                Ok(c) => set_codigo.set(format!("Código para digitar no Univo: {c} (vale 2 minutos, 3 tentativas)")),
                Err(e) => set_codigo.set(e),
            }
        });
    };
    let dispensar = move |id: String| {
        #[derive(Serialize)]
        struct Args {
            id: String,
        }
        spawn_local(async move {
            let _ = chamar::<()>("dispensar_pedido_ponte", Args { id }).await;
            recarregar_ponte();
        });
    };
    let pedidos_ponte = move || match ponte.get() {
        Some(Ok(p)) if !p.pedidos.is_empty() => p
            .pedidos
            .into_iter()
            .map(|x| {
                let (intencao, id) = (x.intencao.clone(), x.id.clone());
                let params = x
                    .parametros
                    .iter()
                    .map(|(k, v)| view! { <li class="mono">{format!("{k}: {v}")}</li> })
                    .collect::<Vec<_>>();
                view! {
                    <article class="item">
                        <header>
                            <h3>{format!("Pedido de {}: {}", x.origem, x.intencao)}</h3>
                            <span class="selo mono">{x.id.clone()}</span>
                        </header>
                        <ul>{params}</ul>
                        <p class="meta">"Nada roda sem o seu toque; cada escrita no portal ainda pede o seu \"sim\"."</p>
                        <button class="botao primario" on:click=move |_| escolher(intencao.clone())>"Revisar"</button>
                        <button class="botao" on:click=move |_| dispensar(id.clone())>"Dispensar"</button>
                    </article>
                }
            })
            .collect::<Vec<_>>()
            .into_any(),
        Some(Ok(_)) => view! { <p class="meta">"Nenhum pedido esperando."</p> }.into_any(),
        _ => ().into_any(),
    };
    let exemplos = [
        "minha aula ficou branca",
        "o que falta para eu ficar 100%",
        "responde o indeferimento do protocolo",
        "quais semanas da efape faltam",
        "entrar na reunião do multiplica",
    ];
    view! {
        <p class="meta">
            {move || match estado_ia.get() {
                None => "Motores: verificando…".to_owned(),
                Some(Err(e)) => format!("Motores: {e}"),
                Some(Ok(s)) => format!(
                    "Motores: {} ({})",
                    match s.estado.as_str() {
                        "online" => "online, respostas mais rápidas",
                        "local" => "só o modelo local (offline), texto mais simples",
                        _ => "nenhum de pé: entender pedidos funciona; dúvidas precisam de um motor",
                    },
                    s.motores.iter().map(|(id, a)| format!("{id}: {a}")).collect::<Vec<_>>().join(", ")
                ),
            }}
            <Show when=move || estado_ia.get().and_then(Result::ok).is_some_and(|s| s.pode_ligar_local && s.estado != "local")>
                " " <button class="chip" on:click=ligar_local>"Ligar o modelo local"</button>
            </Show>
            " " {move || aviso_ia.get()}
        </p>
        <Show when=move || estado_ia.get().and_then(Result::ok).is_some_and(|s| s.estado == "online" && s.nomes_protegidos == 0)>
            <p class="fonte exemplo">
                "Nenhum nome protegido: CPF, e-mail e RA saem trocados por marcadores, mas nome de aluno não. "
                "Não escreva nomes na pergunta, ou ponha a lista (um por linha) em "
                <span class="mono">{move || estado_ia.get().and_then(Result::ok).map(|s| s.arquivo_nomes).unwrap_or_default()}</span>
                " e reabra o app."
            </p>
        </Show>
        <p class="explica">
            "Escreva o que precisa. A IA do Ordoc entende aqui no aparelho (igual com ou sem internet), "
            "diz o que entendeu e leva à tela que resolve. Toda escrita em portal pede o seu \"sim\"."
        </p>
        <div class="pergunta">
            <textarea
                rows="3"
                placeholder="Ex.: minha aula de ontem ficou branca no Professor Presente"
                prop:value=move || texto.get()
                on:input=move |ev| set_texto.set(event_target_value(&ev))
            ></textarea>
            <div>
                <button class="botao primario" on:click=move |_| perguntar()>"Entender"</button>
                <button class="botao" on:click=move |_| responder()>"Perguntar à IA"</button>
            </div>
        </div>
        {move || pensando.get().then(|| view! { <p class="carregando">"A IA está respondendo…"</p> })}
        {move || match duvida.get() {
            None => ().into_any(),
            Some(Err(e)) => view! { <p class="erro">{e}</p> }.into_any(),
            Some(Ok(d)) => {
                let selo = if d.online { "online" } else { "offline" };
                let aviso = d.modo_offline.then(|| view! { <p class="meta">"Modo offline: texto mais simples."</p> });
                view! {
                    <article class="item entendi">
                        <header>
                            <h3>"Resposta da IA do Ordoc"</h3>
                            <span class="selo mono">{format!("{} · {}", selo, d.motor)}</span>
                        </header>
                        <p class="resposta-ia">{d.texto.clone()}</p>
                        {aviso}
                        <p class="meta">"É consulta, não decisão: confira norma, prazo e número na fonte antes de usar."</p>
                    </article>
                }
                .into_any()
            }
        }}
        <div class="chips">
            {exemplos
                .into_iter()
                .map(|e| {
                    view! {
                        <button class="chip" on:click=move |_| set_texto.set(e.to_owned())>{e}</button>
                    }
                })
                .collect::<Vec<_>>()}
        </div>
        <h2>"Pedidos do Univo"</h2>
        <p class="meta">
            {move || match ponte.get() {
                None => "Ponte: verificando…".to_owned(),
                Some(Err(e)) => format!("Ponte: {e}"),
                Some(Ok(p)) => format!("Ponte: {}", p.situacao),
            }}
            " " <button class="chip" on:click=parear>"Parear com o Univo"</button>
            " " <button class="chip" on:click=move |_| recarregar_ponte()>"Ver de novo"</button>
            " " <span class="mono">{move || codigo.get()}</span>
        </p>
        <div class="lista">{pedidos_ponte}</div>
        {move || match resposta.get() {
            None => ().into_any(),
            Some(Err(e)) => view! { <p class="erro">{e}</p> }.into_any(),
            Some(Ok(r)) if r.intencao.is_some() => {
                let tela = r.tela.as_deref().and_then(Tela::do_id);
                let params = r
                    .parametros
                    .iter()
                    .map(|(k, v)| view! { <li class="mono">{format!("{k}: {v}")}</li> })
                    .collect::<Vec<_>>();
                view! {
                    <article class="item entendi">
                        <h3>"Entendi: " {r.descricao.clone()} ". Certo?"</h3>
                        <p class="meta">{format!("decidido {}", r.como)}</p>
                        <ul>{params}</ul>
                        <p class="acao">{r.proximo_passo.clone()}</p>
                        {tela
                            .map(|t| {
                                view! {
                                    <button class="botao primario" on:click=move |_| ir_para.set(t)>
                                        "Sim, abrir"
                                    </button>
                                }
                            })}
                        <button class="botao" on:click=move |_| set_resposta.set(None)>"Não, vou reescrever"</button>
                    </article>
                }
                .into_any()
            }
            Some(Ok(r)) => {
                let opcoes = r
                    .candidatos
                    .into_iter()
                    .map(|(id, desc)| {
                        view! {
                            <button class="chip" on:click=move |_| escolher(id.clone())>{desc}</button>
                        }
                    })
                    .collect::<Vec<_>>();
                view! {
                    <article class="item">
                        <p>{r.proximo_passo.clone()}</p>
                        <div class="chips">{opcoes}</div>
                    </article>
                }
                .into_any()
            }
        }}
    }
}

// ---------------------------------------------------------------------------------------------
// EFAPE e Multiplica
// ---------------------------------------------------------------------------------------------

fn semana_view(s: SemanaVisao, aviso: WriteSignal<String>) -> impl IntoView {
    let pct = s.percentual.clamp(0.0, 100.0);
    let completa = pct >= 100.0 && s.faltam.is_empty();
    let faltam = s.faltam.iter().map(|f| view! { <li>{f.clone()}</li> }).collect::<Vec<_>>();
    let link = s.link.clone();
    view! {
        <article class="item" class:completa=completa>
            <header>
                <h3>{s.nome.clone()}</h3>
                <span class="selo mono">{format!("{pct:.0}%")}</span>
            </header>
            <div class="barra"><div class="barra-cheia" style=format!("width:{pct:.1}%")></div></div>
            <ul>{faltam}</ul>
            {link
                .map(|u| {
                    view! {
                        <button
                            class="botao"
                            on:click=move |_| abrir_portal("AvaEfape", Some(u.clone()), aviso)
                        >
                            "Abrir no AVA (o vídeo tem o botão Flutuar)"
                        </button>
                    }
                })}
        </article>
    }
}

fn encontro_view(e: EncontroVisao, hoje: String, aviso: WriteSignal<String>) -> impl IntoView {
    let abre = e.pesquisa_abre.clone().unwrap_or_default();
    let fecha = e.pesquisa_fecha.clone().unwrap_or_default();
    let lida = e.situacao.clone().unwrap_or_default().to_uppercase();
    let (situacao, classe, aberta) = if lida.contains("RESPOND") {
        ("pesquisa respondida (lida na SED)".to_owned(), "item completa", false)
    } else if lida.contains("ENCERR") {
        ("pesquisa ENCERRADA sem resposta (lida na SED): ver Solicitação de Recurso".to_owned(), "item vencido", false)
    } else if !lida.is_empty() {
        (format!("pesquisa: {lida}"), "item vencendo", true)
    } else if e.pesquisa_feita {
        ("pesquisa feita".to_owned(), "item completa", false)
    } else if !abre.is_empty() && hoje < abre {
        (format!("pesquisa abre em {abre}"), "item", false)
    } else if !fecha.is_empty() && hoje > fecha {
        (format!("pesquisa fechou em {fecha} sem resposta"), "item vencido", false)
    } else {
        (format!("PESQUISA ABERTA até {fecha}"), "item vencendo", true)
    };
    let link = e.link.clone();
    view! {
        <article class=classe>
            <header>
                <h3>{e.titulo.clone()}</h3>
                <span class="selo mono">{e.data.clone()}</span>
            </header>
            <p class="acao">{situacao}</p>
            {link
                .map(|u| {
                    view! {
                        <button class="botao" on:click=move |_| abrir_portal("TeamsWeb", Some(u.clone()), aviso)>
                            "Entrar no encontro (Teams web, conta @professor)"
                        </button>
                    }
                })}
            <Show when=move || aberta>
                <button
                    class="botao primario"
                    on:click=move |_| {
                        abrir_portal(
                            "Sed",
                            Some("https://sed.educacao.sp.gov.br/".into()),
                            aviso,
                        )
                    }
                >
                    "Fazer a pesquisa na SED"
                </button>
            </Show>
        </article>
    }
}

#[component]
fn TelaEfape() -> impl IntoView {
    let (dados, set_dados) = signal::<Option<Result<VisaoEfape, String>>>(None);
    let recarregar = move || {
        spawn_local(async move { set_dados.set(Some(chamar::<VisaoEfape>("efape", Nada {}).await)) });
    };
    recarregar();
    let (aviso, set_aviso) = signal(String::new());
    // "Atualizar": lê o AVA na aba já logada do navegador do Ordoc e grava o efape.json (só leitura).
    let (atualizando, set_atualizando) = signal(false);
    let (situacao, set_situacao) = signal(String::new());
    let atualizar = move |_| {
        set_atualizando.set(true);
        set_situacao.set("Lendo o AVA-EFAPE…".to_owned());
        spawn_local(async move {
            match chamar::<String>("atualizar_efape", Nada {}).await {
                Ok(m) => {
                    set_situacao.set(m);
                    recarregar();
                }
                Err(e) => set_situacao.set(e),
            }
            set_atualizando.set(false);
        });
    };
    let atualizar_multiplica = move |_| {
        set_atualizando.set(true);
        set_situacao.set("Lendo as pesquisas do Multiplica na SED…".to_owned());
        spawn_local(async move {
            match chamar::<String>("atualizar_multiplica", Nada {}).await {
                Ok(m) => {
                    set_situacao.set(m);
                    recarregar();
                }
                Err(e) => set_situacao.set(e),
            }
            set_atualizando.set(false);
        });
    };
    let hoje = ponte::hoje();
    let conteudo = move || {
        let hoje = hoje.clone();
        match dados.get() {
            None => view! { <p class="carregando">"Carregando…"</p> }.into_any(),
            Some(Err(e)) => view! { <p class="erro">{e}</p> }.into_any(),
            Some(Ok(v)) => {
                let faltando = v.semanas.iter().filter(|s| s.percentual < 100.0 || !s.faltam.is_empty()).count();
                let semanas = v.semanas.into_iter().map(|s| semana_view(s, set_aviso)).collect::<Vec<_>>();
                let encontros = v
                    .encontros
                    .into_iter()
                    .map(|e| encontro_view(e, hoje.clone(), set_aviso))
                    .collect::<Vec<_>>();
                let links = v
                    .links
                    .into_iter()
                    .map(|(nome, url)| {
                        let portal = if url.contains("avaefape") { "AvaEfape" } else { "Sed" };
                        view! {
                            <button class="chip" on:click=move |_| abrir_portal(portal, Some(url.clone()), set_aviso)>
                                {nome}
                            </button>
                        }
                    })
                    .collect::<Vec<_>>();
                view! {
                    {aviso_fonte(v.fonte)}
                    <p class="erro">{move || aviso.get()}</p>
                    <h2>{format!("Semanas da formação · {faltando} por terminar")}</h2>
                    <p class="explica">
                        "O % é o da página do curso (o painel do AVA fica em cache). Regra da jornada: 100% "
                        "das interações e 98% de cada vídeo. O botão Flutuar põe o MESMO vídeo numa janela por "
                        "cima: o AVA continua contando."
                    </p>
                    <div class="lista">{semanas}</div>
                    <h2>"Multiplica SP"</h2>
                    <p class="meta">
                        <button class="botao" on:click=atualizar_multiplica disabled=move || atualizando.get()>
                            "Atualizar do Multiplica"
                        </button>
                        " (abra antes, na SED, a página da Pesquisa do Multiplica)"
                    </p>
                    <p class="explica">
                        "A pesquisa da SED abre no dia seguinte ao encontro e fecha um dia antes do próximo."
                    </p>
                    <div class="lista">{encontros}</div>
                    <div class="chips">{links}</div>
                }
                .into_any()
            }
        }
    };
    view! {
        <p class="meta">
            <button class="botao primario" on:click=atualizar disabled=move || atualizando.get()>
                {move || if atualizando.get() { "Atualizando…" } else { "Atualizar da EFAPE" }}
            </button>
            " " {move || situacao.get()}
        </p>
        {conteudo}
    }
}

// ---------------------------------------------------------------------------------------------
// Plataforma EP
// ---------------------------------------------------------------------------------------------

#[component]
fn TelaEp() -> impl IntoView {
    let (dados, set_dados) = signal::<Option<Result<VisaoEp, String>>>(None);
    let recarregar = move || {
        spawn_local(async move { set_dados.set(Some(chamar::<VisaoEp>("ep", Nada {}).await)) });
    };
    recarregar();
    let (aviso, set_aviso) = signal(String::new());
    let (atualizando, set_atualizando) = signal(false);
    let (situacao, set_situacao) = signal(String::new());
    let atualizar = move |_| {
        set_atualizando.set(true);
        set_situacao.set("Lendo as tarefas da plataforma EP (uma por uma, com calma: alguns minutos)…".to_owned());
        spawn_local(async move {
            match chamar::<String>("atualizar_ep", Nada {}).await {
                Ok(m) => {
                    set_situacao.set(m);
                    recarregar();
                }
                Err(e) => set_situacao.set(e),
            }
            set_atualizando.set(false);
        });
    };
    let atualizar_tsp = move |_| {
        set_atualizando.set(true);
        set_situacao.set("Lendo o Tarefa SP (Gerenciador de Atividades aberto e logado)…".to_owned());
        spawn_local(async move {
            match chamar::<String>("atualizar_tarefasp", Nada {}).await {
                Ok(m) => {
                    set_situacao.set(m);
                    recarregar();
                }
                Err(e) => set_situacao.set(e),
            }
            set_atualizando.set(false);
        });
    };
    let conteudo = move || match dados.get() {
        None => view! { <p class="carregando">"Carregando…"</p> }.into_any(),
        Some(Err(e)) => view! { <p class="erro">{e}</p> }.into_any(),
        Some(Ok(v)) => {
            let aguardando_tsp: u32 = v.tarefasp.iter().map(|t| t.aguardando).sum();
            let zeradas_tsp: u32 = v.tarefasp.iter().map(|t| t.zeradas_ia).sum();
            let n_tsp = v.tarefasp.len();
            let lista_tsp = v
                .tarefasp
                .iter()
                .map(|t| {
                    let classe = if t.aguardando > 0 { "item vencendo" } else { "item" };
                    let selo = if t.aguardando > 0 {
                        format!("{} aguardando", t.aguardando)
                    } else {
                        "corrigida".to_owned()
                    };
                    let media = t.media.clone().map(|m| format!(" · média {m} pontos")).unwrap_or_default();
                    view! {
                        <article class=classe>
                            <header>
                                <h3>{t.titulo.clone()}</h3>
                                <span class="selo">{selo}</span>
                            </header>
                            <p class="meta">{format!("{} · {} · publicada {} · expira {}", t.turma, t.tipo, t.publicada, t.expira)}</p>
                            <p class="meta">
                                {format!(
                                    "{} entregues, {} só em rascunho, {} questões abertas zeradas pela IA{media}",
                                    t.entregues, t.rascunhos, t.zeradas_ia
                                )}
                            </p>
                        </article>
                    }
                })
                .collect::<Vec<_>>();
            let total: u32 = v.tarefas.iter().map(|t| t.a_avaliar).sum();
            let n = v.tarefas.len();
            let lista = v
                .tarefas
                .into_iter()
                .map(|t| {
                    let link = t.link.clone();
                    let envio = match (t.enviados, t.participantes) {
                        (Some(e), Some(p)) => format!("{e} de {p} enviaram"),
                        (Some(e), None) => format!("{e} enviaram"),
                        _ => String::new(),
                    };
                    let datas = t.datas.join(" · ");
                    view! {
                        <article class="item vencendo">
                            <header>
                                <h3>{t.tarefa.clone()}</h3>
                                <span class="selo destaque">{format!("{} a avaliar", t.a_avaliar)}</span>
                            </header>
                            <p class="meta">{t.curso.clone()}</p>
                            <p class="meta">{envio} " " {datas}</p>
                            <button class="botao" on:click=move |_| abrir_portal("EducacaoProfissional", Some(link.clone()), set_aviso)>
                                "Abrir a tarefa"
                            </button>
                        </article>
                    }
                })
                .collect::<Vec<_>>();
            view! {
                {aviso_fonte(v.fonte)}
                <p class="erro">{move || aviso.get()}</p>
                <h2>{format!("{total} envios esperando avaliação em {n} tarefas")}</h2>
                <p class="explica">
                    "Só contagens: a lista de envios (com nome de aluno) não sai da plataforma. As notas seguem o "
                    "critério que você aprovou; envio duvidoso fica para você decidir."
                </p>
                <div class="lista">{lista}</div>
                <h2>{format!("Tarefa SP: {n_tsp} tarefas publicadas, {aguardando_tsp} respostas aguardando correção")}</h2>
                <p class="meta">
                    <button class="botao" on:click=atualizar_tsp disabled=move || atualizando.get()>
                        "Atualizar do Tarefa SP"
                    </button>
                    " (abra antes o Gerenciador de Atividades pela Sala do Futuro)"
                </p>
                <p class="explica">
                    {format!(
                        "A questão aberta é corrigida pela IA da própria plataforma; {zeradas_tsp} foram zeradas por ela \
                         (em geral resposta copiada da resolução). Revisar é opcional. Só contagens: nome e nota de \
                         aluno não saem do Gerenciador."
                    )}
                </p>
                <div class="lista">{lista_tsp}</div>
            }
            .into_any()
        }
    };
    view! {
        <p class="meta">
            <button class="botao primario" on:click=atualizar disabled=move || atualizando.get()>
                {move || if atualizando.get() { "Atualizando…" } else { "Atualizar da EP" }}
            </button>
            " " {move || situacao.get()}
        </p>
        {conteudo}
    }
}

// ---------------------------------------------------------------------------------------------
// Painel do aluno (link cifrado por aluno; publicar é outro passo, com o sim do professor)
// ---------------------------------------------------------------------------------------------

#[component]
fn TelaPainelAlunos() -> impl IntoView {
    let (dados, set_dados) = signal::<Option<Result<VisaoPortalAlunos, String>>>(None);
    let (ocupado, set_ocupado) = signal(false);
    let (situacao, set_situacao) = signal(String::new());
    let rodar = move |comando: &'static str, msg: &'static str| {
        set_ocupado.set(true);
        set_situacao.set(msg.to_owned());
        spawn_local(async move {
            let r = chamar::<VisaoPortalAlunos>(comando, Nada {}).await;
            set_situacao.set(String::new());
            set_dados.set(Some(r));
            set_ocupado.set(false);
        });
    };
    rodar("gerar_painel_alunos", "Gerando os painéis…");
    let resultado = move || match dados.get() {
        None => view! { <p class="carregando">"Carregando…"</p> }.into_any(),
        Some(Err(e)) => view! { <p class="erro">{e}</p> }.into_any(),
        Some(Ok(v)) => {
            let exemplo = v.fonte == "exemplo";
            let publicado = !v.url_base.is_empty();
            let erros = v.erros.clone();
            let avisos = v.avisos.clone();
            let todos: String = v.links.iter().map(|l| format!("{} ({}): {}", l.nome, l.turma, l.link)).collect::<Vec<_>>().join("\n");
            let linhas = v
                .links
                .iter()
                .map(|l| {
                    let link = l.link.clone();
                    view! {
                        <li>
                            <span class="nome-aluno">{l.nome.clone()}</span>" · " {l.turma.clone()} " "
                            <button class="chip" on:click=move |_| copiar(&link)>"Copiar link"</button>
                        </li>
                    }
                })
                .collect_view();
            view! {
                <div class="cartao">
                    <p><b>{v.alunos}</b>" painel(is), versão " {v.versao} "."
                        {exemplo.then(|| " São alunos FICTÍCIOS do exemplo: use “Ler a plataforma EP” para os dados reais.")}</p>
                    <p>{if publicado {
                        format!("Endereço do site: {}", v.url_base)
                    } else {
                        "Ainda não publicado: os links só abrem depois que o site for hospedado (só com o seu sim). Até lá, servem para conferir.".to_owned()
                    }}</p>
                    <p class="dica">"Site cifrado (o que se hospeda): " <code>{v.pasta_site.clone()}</code></p>
                    {(!avisos.is_empty()).then(|| view! {
                        <div class="aviso"><b>"Avisos da coleta:"</b><ul>{avisos.into_iter().map(|e| view! { <li>{e}</li> }).collect_view()}</ul></div>
                    })}
                    {(!erros.is_empty()).then(|| view! {
                        <div class="aviso"><b>"Não gerados:"</b><ul>{erros.into_iter().map(|e| view! { <li>{e}</li> }).collect_view()}</ul></div>
                    })}
                    <details>
                        <summary>"Links dos alunos (só nesta tela; não mostre em tela compartilhada)"</summary>
                        <button class="chip" on:click=move |_| copiar(&todos)>"Copiar todos"</button>
                        <ul class="lista-links">{linhas}</ul>
                    </details>
                </div>
            }.into_any()
        }
    };
    view! {
        <section>
            <h2>"Painel do aluno"</h2>
            <p class="dica">"Cada aluno recebe um link único. Por matéria e por número do registro de aula, mostra: verde "
                "(entregou), amarelo (liberado para reenviar), vermelho (ainda não fez), cinza (ainda não liberada), a nota "
                "e a avaliação, os Pause e Responda (se acertou de primeira), as faltas e uma estimativa de média. O site "
                "guarda só texto cifrado e sem nome; a chave vai no link, depois do #, e nunca chega ao servidor."</p>
            <p class="dica">"A coleta fica guardada CIFRADA neste computador. No arquivo portal-alunos-config.json (pasta de "
                "dados): a versão dos links (aumente se um link vazar), o endereço do site e a semana até onde as "
                "atividades já foram dadas (ate_semana)."</p>
            <button class="primario" disabled=move || ocupado.get()
                on:click=move |_| rodar("atualizar_painel_alunos", "Lendo a plataforma EP, disciplina por disciplina (de 5 a 15 minutos)…")>
                "Ler a plataforma EP e gerar"
            </button>
            " "
            <button class="chip" disabled=move || ocupado.get()
                on:click=move |_| rodar("gerar_painel_alunos", "Gerando os painéis…")>
                "Gerar de novo (sem ler a plataforma)"
            </button>
            <p class="dica">{move || situacao.get()}</p>
            {resultado}
        </section>
    }
}

// ---------------------------------------------------------------------------------------------
// Revisar: telemetria do app (o Univo lê o mesmo resumo para aprender e prestar contas)
// ---------------------------------------------------------------------------------------------

#[component]
fn TelaRevisar() -> impl IntoView {
    let (dados, set_dados) = signal::<Option<Result<VisaoTelemetria, String>>>(None);
    spawn_local(async move { set_dados.set(Some(chamar::<VisaoTelemetria>("telemetria", Nada {}).await)) });
    let conteudo = move || match dados.get() {
        None => view! { <p class="carregando">"Carregando…"</p> }.into_any(),
        Some(Err(e)) => view! { <p class="erro">{e}</p> }.into_any(),
        Some(Ok(v)) => {
            let atencao = v.atencao.clone();
            let linhas = v
                .eventos
                .iter()
                .map(|e| {
                    view! {
                        <tr>
                            <td><code>{e.evento.clone()}</code></td>
                            <td>{e.vezes}</td>
                            <td>{e.erros}</td>
                            <td>{e.ultima_vez.clone()}</td>
                            <td>{e.ultimo_erro.clone()}</td>
                        </tr>
                    }
                })
                .collect_view();
            view! {
                <div class="cartao">
                    <p>"Últimos " {v.dias} " dias: " <b>{v.total}</b> " usos, " <b>{v.erros}</b> " com erro."</p>
                    {(!atencao.is_empty()).then(|| view! {
                        <div class="aviso"><b>"Olhar primeiro:"</b><ul>{atencao.into_iter().map(|e| view! { <li>{e}</li> }).collect_view()}</ul></div>
                    })}
                    <table class="tabela">
                        <thead><tr><th>"Evento"</th><th>"Vezes"</th><th>"Erros"</th><th>"Última vez"</th><th>"Último erro"</th></tr></thead>
                        <tbody>{linhas}</tbody>
                    </table>
                    <p class="dica">"Resumo para o Univo (sem dado de aluno): " <code>{v.arquivo_univo.clone()}</code></p>
                </div>
            }.into_any()
        }
    };
    view! {
        <section>
            <h2>"Revisar: o que o app fez"</h2>
            <p class="dica">"Telemetria do próprio Ordoc: cada botão de atualizar, cada pedido à IA (só a intenção entendida, "
                "nunca o texto) e cada painel gerado, com tempo e erro. Nada de aluno, nome, nota ou senha. O Univo lê o "
                "resumo para aprender e prestar contas; nada sai do computador por aqui."</p>
            {conteudo}
        </section>
    }
}

// ---------------------------------------------------------------------------------------------
// Oportunidades (concursos, processos seletivos, Banco de Talentos, aplicação de provas, atribuição)
// ---------------------------------------------------------------------------------------------

fn abrir_oportunidade(link: String, aviso: WriteSignal<String>) {
    #[derive(Serialize)]
    struct Args {
        link: String,
    }
    spawn_local(async move {
        match chamar::<()>("abrir_oportunidade", Args { link }).await {
            Ok(()) => aviso.set(String::new()),
            Err(e) => aviso.set(e),
        }
    });
}

/// Dias de hoje até "AAAA-MM-DD" (sem calendário: conta pela data civil).
fn dias_ate(fim: &str, hoje: &str) -> Option<i64> {
    let civil = |s: &str| -> Option<i64> {
        let a: i64 = s.get(0..4)?.parse().ok()?;
        let m: i64 = s.get(5..7)?.parse().ok()?;
        let d: i64 = s.get(8..10)?.parse().ok()?;
        let (a2, m2) = if m <= 2 { (a - 1, m + 9) } else { (a, m - 3) };
        let era = a2.div_euclid(400);
        let yoe = a2 - era * 400;
        let doy = (153 * m2 + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        Some(era * 146_097 + doe)
    };
    Some(civil(fim)? - civil(hoje)?)
}

#[component]
fn TelaOportunidades() -> impl IntoView {
    let (dados, set_dados) = signal::<Option<Result<VisaoOportunidades, String>>>(None);
    let recarregar = move || {
        spawn_local(async move { set_dados.set(Some(chamar::<VisaoOportunidades>("oportunidades", Nada {}).await)) });
    };
    recarregar();
    let (aviso, set_aviso) = signal(String::new());
    let (atualizando, set_atualizando) = signal(false);
    let (situacao, set_situacao) = signal(String::new());
    let (so_area, set_so_area) = signal(true);
    let atualizar = move |_| {
        set_atualizando.set(true);
        set_situacao.set("Lendo o Portal de Concursos, o Centro Paula Souza e o Banco de Talentos…".to_owned());
        spawn_local(async move {
            match chamar::<String>("atualizar_oportunidades", Nada {}).await {
                Ok(m) => {
                    set_situacao.set(m);
                    recarregar();
                }
                Err(e) => set_situacao.set(e),
            }
            set_atualizando.set(false);
        });
    };
    let conteudo = move || match dados.get() {
        None => view! { <p class="carregando">"Carregando…"</p> }.into_any(),
        Some(Err(e)) => view! { <p class="erro">{e}</p> }.into_any(),
        Some(Ok(v)) => {
            let hoje = v.lido_em.get(0..10).unwrap_or_default().to_owned();
            let total = v.itens.len();
            let da_area = v.itens.iter().filter(|o| o.da_area).count();
            let perto = v.itens.iter().filter(|o| o.da_area && o.perto).count();
            let cidades = if v.cidades.is_empty() {
                "nenhuma cidade no perfil ainda (ponha as cidades das suas escolas no arquivo abaixo)".to_owned()
            } else {
                v.cidades.join(", ")
            };
            let arquivo = v.arquivo_perfil.clone();
            let mostrar = so_area.get();
            let lista = v
                .itens
                .into_iter()
                .filter(|o| !mostrar || o.da_area)
                .take(150)
                .map(|o| {
                    let prazo = o
                        .fim
                        .as_deref()
                        .map(|f| match dias_ate(f, &hoje) {
                            Some(0) => format!("inscrição até HOJE ({f})"),
                            Some(1) => format!("inscrição até amanhã ({f})"),
                            Some(n) if n > 1 => format!("inscrição até {f} ({n} dias)"),
                            _ => format!("inscrição até {f}"),
                        })
                        .unwrap_or_else(|| "sem data na lista (ver o edital)".to_owned());
                    let urgente = o.fim.as_deref().and_then(|f| dias_ate(f, &hoje)).is_some_and(|n| n <= 3);
                    let classe = if urgente { "item vencendo" } else { "item" };
                    let selo = match o.tipo.as_str() {
                        "concurso" => "concurso",
                        "banco_de_talentos" => "Banco de Talentos",
                        _ => "processo seletivo",
                    };
                    let mut meta = vec![o.orgao.clone()];
                    if !o.local.is_empty() {
                        meta.push(o.local.clone());
                    }
                    if o.perto {
                        meta.push("perto de uma escola sua".to_owned());
                    }
                    let extra = [
                        Some(format!("taxa: {}", o.taxa)),
                        o.salario.clone().map(|s| format!("salário {s}")),
                        o.vagas.map(|n| format!("{n} vagas")),
                        o.escolaridade.clone(),
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" · ");
                    let link = o.link.clone();
                    let destino = o.destino.clone();
                    view! {
                        <article class=classe>
                            <header>
                                <h3>{o.titulo.clone()}</h3>
                                <span class="selo">{selo}</span>
                            </header>
                            <p class="meta">{meta.join(" · ")}</p>
                            <p class="meta">{prazo} " · " {extra}</p>
                            <p class="meta">{format!("Fonte: {}", o.fonte)}</p>
                            <button class="botao" on:click=move |_| abrir_oportunidade(link.clone(), set_aviso)>
                                "Abrir edital e inscrição"
                            </button>
                            <span class="meta mono">{format!(" vai para {destino}")}</span>
                        </article>
                    }
                })
                .collect::<Vec<_>>();
            let canais = v
                .canais
                .into_iter()
                .map(|c| {
                    let link = c.link.clone();
                    let destino = c.destino.clone();
                    view! {
                        <article class="item">
                            <header>
                                <h3>{c.nome.clone()}</h3>
                                <span class="selo">{c.confianca.clone()}</span>
                            </header>
                            <p class="meta">{c.para_que.clone()}</p>
                            <p class="explica">{c.observacao.clone()}</p>
                            <button class="botao" on:click=move |_| abrir_oportunidade(link.clone(), set_aviso)>
                                "Abrir o site oficial"
                            </button>
                            <span class="meta mono">{format!(" vai para {destino}")}</span>
                        </article>
                    }
                })
                .collect::<Vec<_>>();
            view! {
                {aviso_fonte(v.fonte)}
                <p class="erro">{move || aviso.get()}</p>
                <h2>{format!("{total} inscrições abertas · {da_area} na sua área · {perto} perto das suas escolas")}</h2>
                <p class="meta">{format!("Lido em {}", if v.lido_em.is_empty() { "nunca".to_owned() } else { v.lido_em.clone() })}</p>
                <p class="meta">
                    <label>
                        <input type="checkbox" prop:checked=move || so_area.get() on:change=move |_| set_so_area.update(|x| *x = !*x) />
                        " só da minha área"
                    </label>
                </p>
                <p class="explica">
                    {format!("Cidades das suas escolas: {cidades}. Perfil (áreas e cidades): {arquivo}")}
                </p>
                <p class="explica">
                    "A inscrição é sempre sua, no site oficial: o Ordoc mostra prazo, taxa e o link e abre o site numa "
                    "janela separada, sem a sessão da SED. Pagar taxa ou enviar documento só com você na tela."
                </p>
                <div class="lista">{lista}</div>
                <h2>"Aplicação de provas, Banco de Talentos e atribuição"</h2>
                <p class="explica">
                    "Regras para quem trabalha na rede estadual. \"provável\" = notícia ou edital antigo: confira no site."
                </p>
                <div class="lista">{canais}</div>
            }
            .into_any()
        }
    };
    view! {
        <p class="meta">
            <button class="botao primario" on:click=atualizar disabled=move || atualizando.get()>
                {move || if atualizando.get() { "Atualizando…" } else { "Atualizar oportunidades" }}
            </button>
            " " {move || situacao.get()}
        </p>
        {conteudo}
    }
}

// ---------------------------------------------------------------------------------------------
// Portais
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
struct PortalVisao {
    id: String,
    nome: String,
    perfil: String,
    url: String,
}

#[component]
fn TelaPortais() -> impl IntoView {
    let dados = carregar::<Vec<PortalVisao>>("portais");
    let pasta = carregar::<String>("pasta_dados");
    let (aviso, set_aviso) = signal(String::new());
    let grupo = move |lista: &[PortalVisao], perfil: &str| {
        lista
            .iter()
            .filter(|p| p.perfil == perfil)
            .map(|p| {
                let id = p.id.clone();
                view! {
                    <button class="portal" on:click=move |_| abrir_portal(&id, None, set_aviso)>
                        <strong>{p.nome.clone()}</strong>
                        <span class="mono">{p.url.clone()}</span>
                    </button>
                }
            })
            .collect::<Vec<_>>()
    };
    view! {
        <p class="explica">
            "Cada sistema abre numa janela do Ordoc com o site oficial; o login é seu (o Ordoc nunca guarda senha). "
            "Dois perfis separados para não misturar contas."
        </p>
        <p class="erro">{move || aviso.get()}</p>
        {move || match dados.get() {
            None => view! { <p class="carregando">"Carregando…"</p> }.into_any(),
            Some(Err(e)) => view! { <p class="erro">{e}</p> }.into_any(),
            Some(Ok(lista)) => {
                view! {
                    <h2>"Conta da SED / gov.br"</h2>
                    <div class="grade">{grupo(&lista, "sed")}</div>
                    <h2>"Conta Microsoft @professor"</h2>
                    <div class="grade">{grupo(&lista, "microsoft")}</div>
                }
                    .into_any()
            }
        }}
        <p class="meta">
            "Pasta de dados (os leitores e scripts gravam aqui painel.json, pedidos.json e efape.json): "
            <span class="mono">{move || pasta.get().and_then(Result::ok).unwrap_or_default()}</span>
        </p>
    }
}

// ---------------------------------------------------------------------------------------------
// Telas da próxima versão
// ---------------------------------------------------------------------------------------------

fn em_breve(titulo: &'static str, hoje: &'static str, proximo: &'static str) -> impl IntoView {
    view! {
        <article class="item">
            <h3>{titulo}</h3>
            <p class="detalhe">"Hoje: " {hoje}</p>
            <p class="acao">"Na próxima versão: " {proximo}</p>
        </article>
    }
}

// ---------------------------------------------------------------------------------------------
// Casca
// ---------------------------------------------------------------------------------------------

#[component]
fn App() -> impl IntoView {
    let (tela, set_tela) = signal(Tela::Painel);
    let (tema, set_tema) = signal(ponte::tema_salvo());
    ponte::aplicar_tema(&tema.get_untracked());
    let trocar_tema = move |_| {
        let novo = match tema.get_untracked().as_str() {
            "auto" => "escuro",
            "escuro" => "claro",
            _ => "auto",
        };
        ponte::aplicar_tema(novo);
        set_tema.set(novo.to_owned());
    };
    let menu = Tela::MENU
        .into_iter()
        .map(|(t, nome, dica)| {
            view! {
                <button
                    class="menu-item"
                    class:ativo=move || tela.get() == t
                    title=dica
                    on:click=move |_| set_tela.set(t)
                >
                    {nome}
                </button>
            }
        })
        .collect::<Vec<_>>();
    let titulo = move || {
        Tela::MENU
            .iter()
            .find(|(t, _, _)| *t == tela.get())
            .map(|(_, n, d)| format!("{n} · {d}"))
            .unwrap_or_default()
    };
    view! {
        <div class="casca">
            <nav class="lateral">
                <div class="marca">
                    <img src="icone.svg" alt="" width="32" height="32" />
                    <span>"Ordoc"</span>
                </div>
                {menu}
                <button class="tema" on:click=trocar_tema>
                    {move || format!("Tema: {}", tema.get())}
                </button>
            </nav>
            <main class="conteudo">
                <h1>{titulo}</h1>
                {move || match tela.get() {
                    Tela::Painel => view! { <TelaPainel /> }.into_any(),
                    Tela::Pedidos => view! { <TelaPedidos /> }.into_any(),
                    Tela::Ia => view! { <TelaIa ir_para=set_tela /> }.into_any(),
                    Tela::Efape => view! { <TelaEfape /> }.into_any(),
                    Tela::Portais => view! { <TelaPortais /> }.into_any(),
                    Tela::Ep => view! { <TelaEp/> }.into_any(),
                    Tela::Corrigir => view! { <correcao::TelaCorrecao/> }.into_any(),
                    Tela::Oportunidades => view! { <TelaOportunidades/> }.into_any(),
                    Tela::PainelAlunos => view! { <TelaPainelAlunos/> }.into_any(),
                    Tela::Notas => em_breve(
                        "Notas e frequência",
                        "o domínio já calcula a síntese bimestral, a frequência e o risco final (ordoc-core).",
                        "a tela por turma, sem nome de aluno em tela compartilhada.",
                    ).into_any(),
                    Tela::Revisar => view! { <TelaRevisar/> }.into_any(),
                }}
            </main>
        </div>
    }
}

fn main() {
    console_error_panic_hook::set_once();
    mount_to_body(App);
}
