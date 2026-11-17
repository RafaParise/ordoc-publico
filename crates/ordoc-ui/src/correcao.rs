//! Tela "Corrigir" (Tarefa SP; a EP entra no mesmo desenho): toca no envio e a resposta aparece; "IA ajudar a
//! avaliar" sugere nota e comentário com os trechos que sustentam; "Revisar de novo" pede outra avaliação (nunca
//! repete); o professor ajusta e lança por tarefa, com o "sim" dele. A resposta do estudante só existe na memória
//! do app enquanto esta tela está aberta (ao sair, o back esquece).

use std::collections::{BTreeMap, HashMap};

use leptos::prelude::*;
use leptos::task::spawn_local;
use ordoc_core::visao::{ItemCorrecaoVisao, NotaDecidida, PreparoLancamento, SugestaoVisao, VisaoCorrecao};
use serde::Serialize;

use crate::ponte::chamar;

#[derive(Serialize)]
struct Nada {}

/// Nota e comentário que o professor vai lançar (nota 0 = ainda não decidiu).
#[derive(Clone, Default, PartialEq)]
struct Decisao {
    nota: u8,
    comentario: String,
    incluir: bool,
}

fn rotulo_motivo(m: &str) -> &'static str {
    match m {
        "aguarda_professor" => "aguarda você",
        "zero_da_ia" => "a IA da plataforma deu 0",
        "nota_zero_professor" => "nota 0 lançada (o critério é mínimo 1)",
        _ => "para corrigir",
    }
}

#[component]
pub fn TelaCorrecao() -> impl IntoView {
    let itens = RwSignal::new(Vec::<ItemCorrecaoVisao>::new());
    let ia_local = RwSignal::new(false);
    let situacao = RwSignal::new(String::new());
    let lendo = RwSignal::new(false);
    let aberto = RwSignal::new(None::<u32>);
    let sugestoes = RwSignal::new(HashMap::<u32, SugestaoVisao>::new());
    let pensando = RwSignal::new(None::<u32>);
    let decisoes = RwSignal::new(HashMap::<u32, Decisao>::new());
    let preparo = RwSignal::new(None::<(u64, PreparoLancamento)>);
    let enviando = RwSignal::new(false);

    // ao sair da tela, a resposta dos estudantes sai da memória do app
    on_cleanup(move || {
        spawn_local(async move {
            let _ = chamar::<()>("correcao_fechar", Nada {}).await;
        })
    });

    let ler = move |_| {
        lendo.set(true);
        situacao.set("Lendo o Tarefa SP pela Sala do Futuro (o app entra sozinho; se pedir login, entre e toque de novo)…".into());
        spawn_local(async move {
            match chamar::<VisaoCorrecao>("correcao_coletar", Nada {}).await {
                Ok(v) => {
                    situacao.set(format!(
                        "{} envio(s) para corrigir em {} tarefa(s) lida(s); {} já finalizado(s) por você.",
                        v.itens.len(),
                        v.tarefas_lidas,
                        v.ja_finalizadas
                    ));
                    ia_local.set(v.ia_local);
                    itens.set(v.itens);
                    sugestoes.set(HashMap::new());
                    decisoes.set(HashMap::new());
                    preparo.set(None);
                    aberto.set(None);
                }
                Err(e) => situacao.set(e),
            }
            lendo.set(false);
        });
    };

    let sugerir = move |indice: u32, revisar: bool| {
        #[derive(Serialize)]
        struct Args {
            indice: u32,
            revisar: bool,
        }
        pensando.set(Some(indice));
        spawn_local(async move {
            match chamar::<SugestaoVisao>("correcao_sugerir", Args { indice, revisar }).await {
                Ok(s) => {
                    decisoes.update(|d| {
                        d.insert(
                            indice,
                            Decisao { nota: s.nota, comentario: s.comentario.clone(), incluir: s.decisao == "Lancar" },
                        );
                    });
                    sugestoes.update(|m| {
                        m.insert(indice, s);
                    });
                }
                Err(e) => situacao.set(e),
            }
            pensando.set(None);
        });
    };

    let preparar = move |tarefa_id: u64| {
        #[derive(Serialize)]
        struct Args {
            notas: Vec<NotaDecidida>,
        }
        let do_tarefa: Vec<u32> =
            itens.with(|v| v.iter().filter(|i| i.tarefa_id == tarefa_id).map(|i| i.indice).collect());
        let notas: Vec<NotaDecidida> = decisoes.with(|d| {
            do_tarefa
                .iter()
                .filter_map(|i| d.get(i).filter(|x| x.incluir && (1..=10).contains(&x.nota)).map(|x| (i, x)))
                .map(|(i, x)| NotaDecidida { indice: *i, nota: x.nota, comentario: x.comentario.clone() })
                .collect()
        });
        if notas.is_empty() {
            situacao.set("Marque \"lançar\" em pelo menos um envio desta tarefa (nota de 1 a 10).".into());
            return;
        }
        enviando.set(true);
        situacao.set("Conferindo o lote na plataforma (ensaio, nada é gravado)…".into());
        spawn_local(async move {
            match chamar::<PreparoLancamento>("correcao_preparar", Args { notas }).await {
                Ok(p) => {
                    situacao.set(String::new());
                    preparo.set(Some((tarefa_id, p)));
                }
                Err(e) => situacao.set(e),
            }
            enviando.set(false);
        });
    };

    let confirmar = move |_| {
        #[derive(Serialize)]
        struct Args {
            chave: String,
        }
        let Some((_, p)) = preparo.get_untracked() else { return };
        preparo.set(None);
        enviando.set(true);
        situacao.set("Lançando…".into());
        spawn_local(async move {
            match chamar::<(String, Vec<u32>)>("correcao_confirmar", Args { chave: p.chave }).await {
                Ok((msg, feitos)) => {
                    situacao.set(msg);
                    itens.update(|v| v.retain(|i| !feitos.contains(&i.indice)));
                    aberto.set(None);
                }
                Err(e) => situacao.set(e),
            }
            enviando.set(false);
        });
    };

    let detalhe = move |it: ItemCorrecaoVisao| {
        let i = it.indice;
        let resposta = it.partes.iter().map(|p| p.texto.clone()).collect::<Vec<_>>().join("\n\n");
        let vazia = resposta.trim().is_empty();
        let sugestao = move || sugestoes.with(|m| m.get(&i).cloned());
        let decisao = move || decisoes.with(|d| d.get(&i).cloned().unwrap_or_default());
        let mudar = move |f: Box<dyn FnOnce(&mut Decisao)>| {
            decisoes.update(|d| f(d.entry(i).or_default()));
        };
        view! {
            <div class="correcao-detalhe">
                <details>
                    <summary>"Enunciado"</summary>
                    <p class="meta">{it.enunciado.clone()}</p>
                </details>
                <p class="rotulo">"Resposta do estudante"</p>
                <div class="resposta-aluno">
                    {if vazia { "(sem texto)".to_owned() } else { resposta }}
                </div>
                {it.parecer_plataforma.clone().map(|p| view! { <p class="meta">{p}</p> })}
                <div class="acoes">
                    <button
                        class="botao primario"
                        disabled=move || pensando.get().is_some()
                        on:click=move |_| sugerir(i, false)
                    >
                        {move || if pensando.get() == Some(i) { "Avaliando…" } else { "IA ajudar a avaliar" }}
                    </button>
                    {move || sugestao().map(|_| view! {
                        <button
                            class="botao"
                            disabled=move || pensando.get().is_some()
                            on:click=move |_| sugerir(i, true)
                        >
                            "Revisar de novo"
                        </button>
                    })}
                </div>
                {move || sugestao().map(|s| {
                    let classe = if s.decisao == "Lancar" { "sugestao" } else { "sugestao atencao" };
                    let ev = s.evidencias.iter().map(|e| view! { <li><mark>{e.clone()}</mark></li> }).collect::<Vec<_>>();
                    view! {
                        <div class=classe>
                            <p>
                                <strong>{format!("Sugestão: {}", s.nota)}</strong>
                                {if s.novo_envio { " · pedir novo envio" } else { "" }}
                                {format!(" · {}", s.origem)}
                                {(s.revisoes > 0).then(|| format!(" · revisão {}", s.revisoes))}
                            </p>
                            <p class="meta">{s.motivo.clone()}</p>
                            {(!ev.is_empty()).then(|| view! { <ul class="evidencias">{ev}</ul> })}
                        </div>
                    }
                })}
                <div class="decisao">
                    <label>
                        "Nota "
                        <input
                            type="number"
                            min="1"
                            max="10"
                            inputmode="numeric"
                            prop:value=move || { let n = decisao().nota; if n == 0 { String::new() } else { n.to_string() } }
                            on:input=move |ev| {
                                let n = event_target_value(&ev).trim().parse::<u8>().unwrap_or(0).min(10);
                                mudar(Box::new(move |d| { d.nota = n; d.incluir = n >= 1; }));
                            }
                        />
                    </label>
                    <label class="incluir">
                        <input
                            type="checkbox"
                            prop:checked=move || decisao().incluir
                            on:change=move |ev| {
                                let v = event_target_checked(&ev);
                                mudar(Box::new(move |d| d.incluir = v));
                            }
                        />
                        " lançar"
                    </label>
                </div>
                <textarea
                    rows="3"
                    placeholder="Comentário para o estudante (até 2 frases: o que ficou bom e o que faltou)"
                    prop:value=move || decisao().comentario
                    on:input=move |ev| {
                        let t = event_target_value(&ev);
                        mudar(Box::new(move |d| d.comentario = t));
                    }
                ></textarea>
            </div>
        }
    };

    let lista = move || {
        let mut grupos: BTreeMap<(String, u64), Vec<ItemCorrecaoVisao>> = BTreeMap::new();
        for it in itens.get() {
            grupos.entry((format!("{} · {}", it.tarefa, it.turma), it.tarefa_id)).or_default().push(it);
        }
        grupos
            .into_iter()
            .map(|((titulo, tarefa_id), v)| {
                let n = v.len();
                let cartoes = v
                    .into_iter()
                    .map(|it| {
                        let i = it.indice;
                        let it2 = it.clone();
                        let marcado = move || {
                            decisoes.with(|d| d.get(&i).filter(|x| x.incluir).map(|x| format!("nota {}", x.nota)))
                        };
                        view! {
                            <article class="item" class:aberto=move || aberto.get() == Some(i)>
                                <header on:click=move |_| aberto.update(|a| *a = if *a == Some(i) { None } else { Some(i) })>
                                    <h3>{format!("Envio {}", i + 1)}</h3>
                                    <span class="selo">
                                        {move || marcado().unwrap_or_else(|| rotulo_motivo(&it.motivo).to_owned())}
                                    </span>
                                </header>
                                {move || (aberto.get() == Some(i)).then(|| detalhe(it2.clone()))}
                            </article>
                        }
                    })
                    .collect::<Vec<_>>();
                view! {
                    <section class="grupo-tarefa">
                        <h2>{titulo}</h2>
                        <p class="meta">{format!("{n} envio(s)")}</p>
                        {cartoes}
                        <button
                            class="botao primario"
                            disabled=move || enviando.get()
                            on:click=move |_| preparar(tarefa_id)
                        >
                            "Lançar as notas marcadas desta tarefa"
                        </button>
                    </section>
                }
            })
            .collect::<Vec<_>>()
    };

    view! {
        <div class="tela-correcao">
            <p class="meta">
                {move || if ia_local.get() {
                    "IA local ligada: avalia no aparelho, sem mandar a resposta do estudante para fora."
                } else {
                    "Sem o módulo de IA offline: a sugestão vem da rubrica da tarefa (sem modelo) e sempre fica para você decidir. A resposta do estudante nunca vai para IA online."
                }}
            </p>
            <button class="botao primario" on:click=ler disabled=move || lendo.get()>
                {move || if lendo.get() { "Lendo…" } else { "Ler o que falta corrigir (Tarefa SP)" }}
            </button>
            <p class="situacao">{move || situacao.get()}</p>
            {move || preparo.get().map(|(_, p)| view! {
                <div class="confirmar">
                    <p><strong>{p.pergunta.clone()}</strong></p>
                    <div class="acoes">
                        <button class="botao primario" on:click=confirmar disabled=move || enviando.get()>"Sim, lançar"</button>
                        <button class="botao" on:click=move |_| preparo.set(None)>"Não"</button>
                    </div>
                </div>
            })}
            {lista}
        </div>
    }
}
