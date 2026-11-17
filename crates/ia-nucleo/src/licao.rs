//! Lição aprendida: `Licao` <-> linha "ERRO: ... | CERTO: ..." (o mesmo formato das lições do Hermes/Qwen).
//! Evidência opcional no fim: "ERRO: ... | CERTO: ... | EVIDÊNCIA: ...".

use crate::error::{ErroIa, Result};

/// Limite de cada campo (em caracteres).
const LIMITE_CAMPO: usize = 2000;

/// De onde a lição veio (peso: professor > Claude > agente, na rotina do professor).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrigemLicao {
    Claude,
    Professor,
    Agente,
}

impl OrigemLicao {
    pub fn como_texto(self) -> &'static str {
        match self {
            OrigemLicao::Claude => "claude",
            OrigemLicao::Professor => "professor",
            OrigemLicao::Agente => "agente",
        }
    }

    pub fn de_texto(t: &str) -> Option<Self> {
        match t.trim().to_ascii_lowercase().as_str() {
            "claude" => Some(OrigemLicao::Claude),
            "professor" => Some(OrigemLicao::Professor),
            "agente" => Some(OrigemLicao::Agente),
            _ => None,
        }
    }
}

/// Grau de certeza ([Certo]/[Provável]/[Palpite]), também usado nas normas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grau {
    Certo,
    Provavel,
    Palpite,
}

impl Grau {
    pub fn como_texto(self) -> &'static str {
        match self {
            Grau::Certo => "Certo",
            Grau::Provavel => "Provável",
            Grau::Palpite => "Palpite",
        }
    }

    pub fn de_texto(t: &str) -> Option<Self> {
        let t = crate::higiene::dobrar(t.trim().trim_matches(['[', ']']));
        match t.as_str() {
            "certo" => Some(Grau::Certo),
            "provavel" => Some(Grau::Provavel),
            "palpite" => Some(Grau::Palpite),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Licao {
    pub origem: OrigemLicao,
    pub area: String,
    pub erro: String,
    pub certo: String,
    pub evidencia: Option<String>,
    pub grau: Grau,
}

/// Acha "| ROTULO" (com espaços opcionais depois da barra) num texto já em maiúsculas ASCII.
/// Devolve (início da barra, fim do rótulo).
fn achar_marca(maiusculo: &str, rotulo: &str, a_partir: usize) -> Option<(usize, usize)> {
    let mut busca = a_partir;
    while let Some(rel) = maiusculo.get(busca..)?.find('|') {
        let barra = busca + rel;
        let depois = &maiusculo[barra + 1..];
        let espacos = depois.len() - depois.trim_start().len();
        if depois.trim_start().starts_with(rotulo) {
            return Some((barra, barra + 1 + espacos + rotulo.len()));
        }
        busca = barra + 1;
    }
    None
}

impl Licao {
    /// Cria e valida.
    pub fn nova(
        origem: OrigemLicao,
        area: impl Into<String>,
        erro: impl Into<String>,
        certo: impl Into<String>,
        grau: Grau,
    ) -> Result<Self> {
        let l = Self {
            origem,
            area: area.into().trim().to_owned(),
            erro: erro.into().trim().to_owned(),
            certo: certo.into().trim().to_owned(),
            evidencia: None,
            grau,
        };
        l.validar()?;
        Ok(l)
    }

    pub fn com_evidencia(mut self, evidencia: impl Into<String>) -> Self {
        let e = evidencia.into().trim().to_owned();
        self.evidencia = (!e.is_empty()).then_some(e);
        self
    }

    /// Campos preenchidos, sem quebra de linha e sem as marcas do formato (para ida e volta exata).
    pub fn validar(&self) -> Result<()> {
        if self.area.is_empty() {
            return Err(ErroIa::LicaoInvalida("falta a área"));
        }
        if self.erro.is_empty() {
            return Err(ErroIa::LicaoInvalida("o ERRO está vazio"));
        }
        if self.certo.is_empty() {
            return Err(ErroIa::LicaoInvalida("o CERTO está vazio"));
        }
        let campos = [
            Some(self.area.as_str()),
            Some(self.erro.as_str()),
            Some(self.certo.as_str()),
            self.evidencia.as_deref(),
        ];
        for c in campos.into_iter().flatten() {
            if c.chars().count() > LIMITE_CAMPO {
                return Err(ErroIa::LicaoInvalida("campo longo demais"));
            }
            if c.contains(['\n', '\r']) {
                return Err(ErroIa::LicaoInvalida("campo com quebra de linha"));
            }
            let m = c.to_ascii_uppercase();
            if achar_marca(&m, "CERTO:", 0).is_some() || achar_marca(&m, "EVID", 0).is_some() {
                return Err(ErroIa::LicaoInvalida("campo contém a marca \"| CERTO:\" ou \"| EVIDÊNCIA:\""));
            }
        }
        Ok(())
    }

    /// "ERRO: ... | CERTO: ..." (+ " | EVIDÊNCIA: ..." se houver).
    pub fn para_linha(&self) -> String {
        let mut linha = format!("ERRO: {} | CERTO: {}", self.erro, self.certo);
        if let Some(e) = &self.evidencia {
            linha.push_str(" | EVIDÊNCIA: ");
            linha.push_str(e);
        }
        linha
    }

    /// Lê a linha "ERRO: ... | CERTO: ...". Origem, área e grau vêm de quem chama.
    pub fn de_linha(linha: &str, origem: OrigemLicao, area: &str, grau: Grau) -> Result<Self> {
        let t = linha.trim();
        let m = t.to_ascii_uppercase(); // mesmo tamanho em bytes que `t`
        if !m.starts_with("ERRO:") {
            return Err(ErroIa::LicaoInvalida("a linha precisa começar com \"ERRO:\""));
        }
        let (barra_certo, fim_certo) =
            achar_marca(&m, "CERTO:", 0).ok_or(ErroIa::LicaoInvalida("falta \"| CERTO:\""))?;
        let erro = &t["ERRO:".len()..barra_certo];
        let (certo, evidencia) = match achar_marca(&m, "EVID", fim_certo) {
            Some((barra_evid, fim_evid)) => {
                let resto = &t[fim_evid..];
                let ev = resto.split_once(':').map_or("", |(_, depois)| depois);
                (&t[fim_certo..barra_evid], Some(ev))
            }
            None => (&t[fim_certo..], None),
        };
        let mut l = Licao::nova(origem, area, erro, certo, grau)?;
        if let Some(e) = evidencia {
            l = l.com_evidencia(e);
        }
        l.validar()?;
        Ok(l)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ida_e_volta_da_linha() {
        let l = Licao::nova(
            OrigemLicao::Claude,
            "portal-exemplo",
            "reenviar o formulário sem olhar a área do usuário",
            "conferir a lista de pedidos antes de reenviar",
            Grau::Certo,
        )
        .unwrap()
        .com_evidencia("teste sintético");
        let linha = l.para_linha();
        assert_eq!(
            linha,
            "ERRO: reenviar o formulário sem olhar a área do usuário | CERTO: conferir a lista \
             de pedidos antes de reenviar | EVIDÊNCIA: teste sintético"
        );
        let volta = Licao::de_linha(&linha, OrigemLicao::Claude, "portal-exemplo", Grau::Certo).unwrap();
        assert_eq!(volta, l);
    }

    #[test]
    fn aceita_variacoes_e_recusa_formato_errado() {
        let l = Licao::de_linha("erro: a|certo: b", OrigemLicao::Agente, "x", Grau::Palpite).unwrap();
        assert_eq!((l.erro.as_str(), l.certo.as_str(), l.evidencia), ("a", "b", None));
        for ruim in ["CERTO: b", "ERRO: a", "ERRO:  | CERTO: b", "ERRO: a | CERTO: "] {
            assert!(Licao::de_linha(ruim, OrigemLicao::Agente, "x", Grau::Palpite).is_err());
        }
        assert!(Licao::nova(OrigemLicao::Agente, "x", "a\nb", "c", Grau::Certo).is_err());
    }

    #[test]
    fn grau_e_origem_em_texto() {
        assert_eq!(Grau::de_texto("[Provável]"), Some(Grau::Provavel));
        assert_eq!(Grau::de_texto("palpite"), Some(Grau::Palpite));
        assert_eq!(OrigemLicao::de_texto("Professor"), Some(OrigemLicao::Professor));
        assert_eq!(Grau::de_texto("talvez"), None);
    }
}
