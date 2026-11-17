//! Formato da receita (desenho, seções 4 e 6) e importação de uma pasta de receitas `.md`.
//!
//! ```text
//! ---
//! id: exemplo-de-receita          (minúsculas, dígitos e hífen)
//! versao: 1                       (inteiro >= 1; "versão" também vale)
//! intencao: ExemploIntencao       (letra + letras/dígitos/_; "intenção" também vale)
//! portal: exemplo                 (minúsculas, dígitos e hífen)
//! titulo: Título opcional         (senão vale o primeiro "# ")
//! ---
//! # Título
//! ## Quando usar
//! ## Entradas
//! ## Passos
//! ## Onde precisa do "sim"
//! ## Saída
//! ## Erros e lições
//! ## Privacidade
//! ```
//! As sete seções são obrigatórias e não podem ficar vazias. Seções a mais são aceitas e ficam no
//! `texto_md` (o Markdown integral é sempre guardado, regra 4 do regras do projeto).

use std::path::Path;

use crate::error::{ErroIa, Result};
use crate::higiene::dobrar;
use crate::memoria::MemoriaIa;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receita {
    pub id: String,
    pub versao: u32,
    pub intencao: String,
    pub portal: String,
    pub titulo: String,
    pub quando_usar: String,
    pub entradas: String,
    pub passos: String,
    /// "Onde precisa do sim": os passos que pedem confirmação do usuário.
    pub pontos_sim: String,
    pub saida: String,
    pub erros_licoes: String,
    pub privacidade: String,
    /// O Markdown integral, como veio (normalizado só em fim de linha).
    pub texto_md: String,
}

/// Seções obrigatórias: nome normalizado e mensagem quando falta.
const SECOES: [(&str, &str); 7] = [
    ("quando usar", "falta a seção \"Quando usar\""),
    ("entradas", "falta a seção \"Entradas\""),
    ("passos", "falta a seção \"Passos\""),
    ("onde precisa do sim", "falta a seção \"Onde precisa do sim\""),
    ("saida", "falta a seção \"Saída\""),
    ("erros e licoes", "falta a seção \"Erros e lições\""),
    ("privacidade", "falta a seção \"Privacidade\""),
];

/// Título de seção normalizado: minúsculas, sem acento, só letras/dígitos e um espaço entre palavras.
fn normalizar_titulo(t: &str) -> String {
    dobrar(t)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn slug_ok(s: &str, max: usize) -> bool {
    !s.is_empty()
        && s.len() <= max
        && !s.starts_with('-')
        && !s.ends_with('-')
        && !s.contains("--")
        && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn intencao_ok(s: &str) -> bool {
    let mut cs = s.chars();
    s.len() <= 60
        && cs.next().is_some_and(|c| c.is_ascii_alphabetic())
        && cs.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

impl Receita {
    /// Lê e valida uma receita em Markdown.
    pub fn de_markdown(md: &str) -> Result<Self> {
        let texto = md.trim_start_matches('\u{feff}').replace("\r\n", "\n");
        let corpo_cab = texto
            .strip_prefix("---\n")
            .ok_or(ErroIa::ReceitaInvalida("falta o cabeçalho entre linhas \"---\""))?;
        let (cabecalho, corpo) = match corpo_cab.find("\n---") {
            Some(fim) => {
                let resto = &corpo_cab[fim + 4..];
                let resto = resto.split_once('\n').map_or("", |(_, depois)| depois);
                (&corpo_cab[..fim], resto)
            }
            None => return Err(ErroIa::ReceitaInvalida("o cabeçalho não fecha com \"---\"")),
        };

        let (mut id, mut versao, mut intencao, mut portal, mut titulo) = (None, None, None, None, None);
        for linha in cabecalho.lines() {
            let Some((chave, valor)) = linha.split_once(':') else {
                continue;
            };
            let valor = valor.trim().trim_matches('"').to_owned();
            match normalizar_titulo(chave).as_str() {
                "id" => id = Some(valor),
                "versao" => {
                    versao = Some(
                        valor
                            .parse::<u32>()
                            .map_err(|_| ErroIa::ReceitaInvalida("versão não é um inteiro"))?,
                    )
                }
                "intencao" => intencao = Some(valor),
                "portal" => portal = Some(valor),
                "titulo" => titulo = Some(valor),
                _ => {}
            }
        }

        // Seções "## ...": conteúdo até a próxima "## " ou "# ".
        let mut secoes: Vec<(String, String)> = Vec::new();
        let mut titulo_md: Option<String> = None;
        let mut atual: Option<(String, Vec<&str>)> = None;
        for linha in corpo.lines() {
            if let Some(nome) = linha.strip_prefix("## ") {
                if let Some((n, l)) = atual.take() {
                    secoes.push((n, l.join("\n").trim().to_owned()));
                }
                atual = Some((normalizar_titulo(nome), Vec::new()));
            } else if let Some(t) = linha.strip_prefix("# ") {
                if let Some((n, l)) = atual.take() {
                    secoes.push((n, l.join("\n").trim().to_owned()));
                }
                if titulo_md.is_none() {
                    titulo_md = Some(t.trim().to_owned());
                }
            } else if let Some((_, l)) = atual.as_mut() {
                l.push(linha);
            }
        }
        if let Some((n, l)) = atual.take() {
            secoes.push((n, l.join("\n").trim().to_owned()));
        }

        let mut achadas: [Option<String>; 7] = Default::default();
        for (nome, conteudo) in secoes {
            if let Some(i) = SECOES.iter().position(|(n, _)| *n == nome) {
                if achadas[i].is_some() {
                    return Err(ErroIa::ReceitaInvalida("seção repetida"));
                }
                achadas[i] = Some(conteudo);
            }
        }
        let mut valores: Vec<String> = Vec::with_capacity(7);
        for (i, (_, falta)) in SECOES.iter().enumerate() {
            match achadas[i].take() {
                Some(c) if !c.is_empty() => valores.push(c),
                Some(_) => return Err(ErroIa::ReceitaInvalida("seção obrigatória vazia")),
                None => return Err(ErroIa::ReceitaInvalida(falta)),
            }
        }
        let mut v = valores.into_iter();
        let mut proxima = || v.next().unwrap_or_default();

        let id = id.ok_or(ErroIa::ReceitaInvalida("falta o id no cabeçalho"))?;
        let receita = Receita {
            titulo: titulo
                .filter(|t| !t.is_empty())
                .or(titulo_md)
                .unwrap_or_else(|| id.clone()),
            id,
            versao: versao.ok_or(ErroIa::ReceitaInvalida("falta a versão no cabeçalho"))?,
            intencao: intencao.ok_or(ErroIa::ReceitaInvalida("falta a intenção no cabeçalho"))?,
            portal: portal.ok_or(ErroIa::ReceitaInvalida("falta o portal no cabeçalho"))?,
            quando_usar: proxima(),
            entradas: proxima(),
            passos: proxima(),
            pontos_sim: proxima(),
            saida: proxima(),
            erros_licoes: proxima(),
            privacidade: proxima(),
            texto_md: texto,
        };
        receita.validar()?;
        Ok(receita)
    }

    /// Confere o cabeçalho e as seções.
    pub fn validar(&self) -> Result<()> {
        if !slug_ok(&self.id, 80) {
            return Err(ErroIa::ReceitaInvalida(
                "id precisa ter só minúsculas, dígitos e hífen (até 80)",
            ));
        }
        if self.versao == 0 {
            return Err(ErroIa::ReceitaInvalida("a versão começa em 1"));
        }
        if !intencao_ok(&self.intencao) {
            return Err(ErroIa::ReceitaInvalida(
                "intenção precisa começar com letra e ter só letras, dígitos e _",
            ));
        }
        if !slug_ok(&self.portal, 40) {
            return Err(ErroIa::ReceitaInvalida(
                "portal precisa ter só minúsculas, dígitos e hífen (até 40)",
            ));
        }
        let secoes = [
            &self.quando_usar,
            &self.entradas,
            &self.passos,
            &self.pontos_sim,
            &self.saida,
            &self.erros_licoes,
            &self.privacidade,
        ];
        if secoes.iter().any(|s| s.trim().is_empty()) || self.titulo.trim().is_empty() {
            return Err(ErroIa::ReceitaInvalida("seção obrigatória vazia"));
        }
        Ok(())
    }
}

/// Resultado da importação: ids gravados e arquivos recusados com o motivo (o motivo é a mensagem do
/// `ErroIa`, que nunca traz trecho do arquivo).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RelatorioImportacao {
    pub importadas: Vec<String>,
    pub recusadas: Vec<(String, String)>,
}

/// Importa todos os `.md` de uma pasta (sem subpastas, em ordem alfabética). Um arquivo ruim não para os
/// outros: vai para `recusadas`. Arquivo com dado sensível é recusado pela própria `MemoriaIa`.
pub fn importar_pasta_receitas(memoria: &MemoriaIa<'_>, pasta: &Path) -> Result<RelatorioImportacao> {
    let mut arquivos: Vec<std::path::PathBuf> = std::fs::read_dir(pasta)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .and_then(|x| x.to_str())
                    .is_some_and(|x| x.eq_ignore_ascii_case("md"))
        })
        .collect();
    arquivos.sort();

    let mut relatorio = RelatorioImportacao::default();
    for caminho in arquivos {
        let nome = caminho
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let resultado = std::fs::read_to_string(&caminho)
            .map_err(ErroIa::from)
            .and_then(|md| Receita::de_markdown(&md))
            .and_then(|r| memoria.gravar_receita(&r).map(|_| r.id));
        match resultado {
            Ok(id) => relatorio.importadas.push(id),
            Err(e) => relatorio.recusadas.push((nome, e.to_string())),
        }
    }
    Ok(relatorio)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Receita sintética (nada real) no formato do módulo.
    pub(crate) fn receita_md(id: &str, intencao: &str, quando: &str, passos: &str) -> String {
        format!(
            "---\nid: {id}\nversao: 1\nintenção: {intencao}\nportal: portal-exemplo\n---\n\
             # Receita {id}\n\n## Quando usar\n{quando}\n\n## Entradas\n- data da aula\n\n\
             ## Passos\n{passos}\n\n## Onde precisa do \"sim\"\nNenhum (só leitura).\n\n\
             ## Saída\nLista do que falta.\n\n## Erros e lições\nERRO: x | CERTO: y\n\n\
             ## Privacidade\nSó contagens.\n"
        )
    }

    #[test]
    fn le_cabecalho_e_secoes() {
        let md = receita_md("exemplo-um", "ExemploUm", "Quando a aula ficou branca.", "1. Abrir.\n2. Ler.");
        let r = Receita::de_markdown(&md).unwrap();
        assert_eq!(r.id, "exemplo-um");
        assert_eq!(r.versao, 1);
        assert_eq!(r.intencao, "ExemploUm");
        assert_eq!(r.portal, "portal-exemplo");
        assert_eq!(r.titulo, "Receita exemplo-um");
        assert_eq!(r.passos, "1. Abrir.\n2. Ler.");
        assert_eq!(r.pontos_sim, "Nenhum (só leitura).");
        assert_eq!(r.texto_md, md);
    }

    #[test]
    fn recusa_receita_fora_do_formato() {
        let boa = receita_md("exemplo-um", "ExemploUm", "q", "p");
        assert!(Receita::de_markdown(&boa.replace("## Privacidade\nSó contagens.\n", "")).is_err());
        assert!(Receita::de_markdown(&boa.replace("id: exemplo-um", "id: Exemplo Um")).is_err());
        assert!(Receita::de_markdown(&boa.replace("versao: 1", "versao: zero")).is_err());
        assert!(Receita::de_markdown(&boa.replace("versao: 1", "versao: 0")).is_err());
        assert!(Receita::de_markdown(&boa.replace("---\nid", "id")).is_err());
        assert!(Receita::de_markdown(&boa.replace("## Passos\np", "## Passos\n")).is_err());
        let repetida = boa.replace("## Saída", "## Entradas\nde novo\n\n## Saída");
        assert!(matches!(
            Receita::de_markdown(&repetida),
            Err(ErroIa::ReceitaInvalida("seção repetida"))
        ));
        // Fim de linha do Windows e títulos sem acento também valem.
        let crlf = boa.replace("## Saída", "## Saida").replace('\n', "\r\n");
        assert!(Receita::de_markdown(&crlf).is_ok());
    }
}
