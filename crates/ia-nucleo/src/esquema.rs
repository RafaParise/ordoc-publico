//! Saída presa a um esquema: o motor recebe o JSON Schema (`response_format` json_schema) e a resposta é
//! validada com serde contra o tipo esperado. Se falhar, a nova tentativa leva a descrição do erro.
//!
//! A `FalhaEsquema` guarda a mensagem do serde (que pode citar trecho da resposta do modelo) só para
//! devolver ao modelo na correção; ela não vira `ErroIa` e o `Debug` dela não mostra essa mensagem.

use serde::de::DeserializeOwned;

/// JSON Schema enviado ao motor. `nome` é o identificador do esquema na API (`json_schema.name`).
#[derive(Debug, Clone, PartialEq)]
pub struct EsquemaSaida {
    pub nome: String,
    pub esquema: serde_json::Value,
}

impl EsquemaSaida {
    pub fn novo(nome: impl Into<String>, esquema: serde_json::Value) -> Self {
        Self {
            nome: nome.into(),
            esquema,
        }
    }
}

/// Por que a saída não serviu.
pub struct FalhaEsquema {
    pub categoria: &'static str,
    pub linha: usize,
    pub coluna: usize,
    descricao: String,
}

impl std::fmt::Debug for FalhaEsquema {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FalhaEsquema")
            .field("categoria", &self.categoria)
            .field("linha", &self.linha)
            .field("coluna", &self.coluna)
            .finish_non_exhaustive()
    }
}

impl FalhaEsquema {
    /// Descrição do serde, para a mensagem de correção ao modelo (pode citar a resposta dele).
    pub fn descricao_para_modelo(&self) -> &str {
        &self.descricao
    }
}

/// Tira cercas de Markdown (```json ... ```) e texto em volta, devolvendo só o trecho JSON provável.
pub fn extrair_json(texto: &str) -> &str {
    let t = texto.trim();
    let t = match t.strip_prefix("```") {
        Some(resto) => {
            let sem_lingua = resto.split_once('\n').map_or("", |(_, corpo)| corpo);
            sem_lingua.trim_end().strip_suffix("```").unwrap_or(sem_lingua).trim()
        }
        None => t,
    };
    let inicio = t.find(['{', '[']);
    let fim = t.rfind(['}', ']']);
    match (inicio, fim) {
        (Some(i), Some(f)) if f >= i => &t[i..=f],
        _ => t,
    }
}

/// Valida a saída do modelo contra o tipo `T`.
pub fn validar_saida<T: DeserializeOwned>(texto: &str) -> std::result::Result<T, FalhaEsquema> {
    serde_json::from_str::<T>(extrair_json(texto)).map_err(|e| {
        use serde_json::error::Category;
        let categoria = match e.classify() {
            Category::Io => "io",
            Category::Syntax => "sintaxe",
            Category::Data => "dados",
            Category::Eof => "fim inesperado",
        };
        FalhaEsquema {
            categoria,
            linha: e.line(),
            coluna: e.column(),
            descricao: e.to_string(),
        }
    })
}

/// Mensagem que vai ao modelo na nova tentativa.
pub fn mensagem_correcao(falha: &FalhaEsquema) -> String {
    format!(
        "Sua resposta anterior não seguiu o esquema JSON pedido ({}: {}). \
         Responda de novo só com o JSON válido, sem nenhum texto fora dele.",
        falha.categoria, falha.descricao
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(deny_unknown_fields)]
    struct Rotulo {
        intencao: String,
        confianca: f32,
    }

    #[test]
    fn aceita_json_puro_e_com_cerca() {
        let a: Rotulo = validar_saida(r#"{"intencao":"x","confianca":0.9}"#).unwrap();
        let b: Rotulo =
            validar_saida("```json\n{\"intencao\":\"x\",\"confianca\":0.9}\n```").unwrap();
        let c: Rotulo = validar_saida("Claro! {\"intencao\":\"x\",\"confianca\":0.9} Pronto.").unwrap();
        assert_eq!(a, b);
        assert_eq!(b, c);
    }

    #[test]
    fn falha_traz_categoria_e_correcao_sem_vazar_no_debug() {
        let falha = validar_saida::<Rotulo>(r#"{"intencao":"Fulana","confianca":"alta"}"#).unwrap_err();
        assert_eq!(falha.categoria, "dados");
        assert!(!format!("{falha:?}").contains("alta"));
        let msg = mensagem_correcao(&falha);
        assert!(msg.contains("esquema JSON"));
        assert!(validar_saida::<Rotulo>("sem json nenhum").is_err());
        assert!(validar_saida::<Rotulo>(r#"{"intencao":"x","confianca":1,"extra":1}"#).is_err());
    }
}
