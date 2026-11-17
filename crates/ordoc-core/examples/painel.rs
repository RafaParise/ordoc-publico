//! Painel "o que falta para ficar 100%" a partir de um JSON local (fora do git).
//!
//! Uso: `cargo run -p ordoc-core --example painel -- <entrada.json>`
//!
//! Formato da entrada (as pendências se referem umas às outras pela `chave`, não pelo id):
//! ```json
//! {"hoje": "2026-11-14", "pendencias": [
//!   {"area": "FAROL", "chave": "farol-2026-1sem", "origem": "FALA_SP", "titulo": "...",
//!    "detalhe": "...", "acao": {"tipo": "AGUARDAR_ORGAO", "protocolo": "123"},
//!    "estado": {"estado": "AGUARDANDO_ORGAO"}, "prazo": null, "bloqueia": ["atribuicao"],
//!    "protocolos": [{"canal": "FALA_SP", "numero": "123", "aberto_em": "2026-11-14",
//!                    "assunto": "...", "situacao": "Cadastrada", "prazo_resposta": "2026-12-14"}]}
//! ]}
//! ```
//! Sai em Markdown no stdout. Só lê; não fala com nenhum sistema. A mesma montagem
//! (`ordoc_core::visao`) alimenta a tela Painel do app.

use std::process::ExitCode;

use ordoc_core::visao::{montar_painel, visao_painel, EntradaPainel, VisaoPainel};

fn imprimir(v: &VisaoPainel) {
    println!("# O que falta para ficar 100% ({})\n", v.hoje);
    println!(
        "**Regularizado: {:.0}%** ({} de {} itens encerrados)\n",
        v.geral.percentual, v.geral.encerradas, v.geral.total
    );
    println!("| Área | Encerradas | % |\n|---|---|---|");
    for a in &v.areas {
        println!("| {} | {}/{} | {:.0}% |", a.rotulo, a.encerradas, a.total, a.percentual);
    }
    println!("\n## Próximas ações (em ordem)\n");
    for (n, item) in v.agenda.iter().enumerate() {
        println!(
            "{}. **{}** — {} · _{}_ · {}",
            n + 1,
            item.titulo,
            item.acao,
            item.estado,
            item.prioridade
        );
        if !item.detalhe.is_empty() {
            println!("   - {}", item.detalhe);
        }
        for pr in &item.protocolos {
            println!("   - protocolo {pr}");
        }
        if !item.travada_por.is_empty() {
            println!("   - travada por: {}", item.travada_por.join("; "));
        }
    }
    if !v.cadeias.is_empty() {
        println!("\n## Cadeias de bloqueio\n");
        for c in &v.cadeias {
            println!("- {}", c.join(" → "));
        }
    }
}

fn main() -> ExitCode {
    let Some(caminho) = std::env::args().nth(1) else {
        eprintln!("uso: painel <entrada.json>");
        return ExitCode::from(2);
    };
    let resultado = std::fs::read_to_string(&caminho)
        .map_err(|e| format!("não li {caminho}: {e}"))
        .and_then(|t| serde_json::from_str::<EntradaPainel>(&t).map_err(|e| format!("JSON: {e}")))
        .and_then(|e| {
            let p = montar_painel(&e).map_err(|err| err.to_string())?;
            visao_painel(&p, &e.hoje, "seus dados locais").map_err(|err| err.to_string())
        });
    match resultado {
        Ok(v) => {
            imprimir(&v);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("erro: {e}");
            ExitCode::FAILURE
        }
    }
}
