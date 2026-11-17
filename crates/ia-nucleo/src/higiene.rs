//! Higienização de texto antes de qualquer motor online e antes de gravar memória (desenho, seção 7).
//!
//! O `Higienizador` acha CPF (com ou sem pontuação), RG no formato pontuado de SP, e-mail, telefone BR,
//! RA/números longos (>= 9 dígitos) e nomes de uma lista recebida em tempo de execução (sem diferenciar
//! maiúsculas nem acentos) e troca cada achado por um marcador estável: `[CPF_1]`, `[EMAIL_1]`,
//! `[ALUNO_1]`... O mesmo valor vira sempre o mesmo marcador dentro de um `MapaMarcadores`, e o mapa
//! restaura o texto original no fim, só no aparelho.
//!
//! A lista de nomes fica só em memória e é zerada no `Drop` (nunca em arquivo nem em `Debug`).

use regex::Regex;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::error::{ErroIa, Result};

/// Tipo de dado sensível achado no texto.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Categoria {
    Cpf,
    Rg,
    Email,
    Telefone,
    /// RA ou qualquer número com 9 dígitos ou mais.
    NumeroLongo,
    /// Nome da lista recebida em tempo de execução.
    Aluno,
}

const N_CATEGORIAS: usize = 6;

impl Categoria {
    /// Rótulo usado no marcador e no erro de recusa.
    pub fn rotulo(self) -> &'static str {
        match self {
            Categoria::Cpf => "CPF",
            Categoria::Rg => "RG",
            Categoria::Email => "EMAIL",
            Categoria::Telefone => "TELEFONE",
            Categoria::NumeroLongo => "RA",
            Categoria::Aluno => "ALUNO",
        }
    }

    fn indice(self) -> usize {
        match self {
            Categoria::Cpf => 0,
            Categoria::Rg => 1,
            Categoria::Email => 2,
            Categoria::Telefone => 3,
            Categoria::NumeroLongo => 4,
            Categoria::Aluno => 5,
        }
    }
}

/// Dobra um caractere: tira o acento das letras latinas e passa para minúscula. Sempre 1 char -> 1 char,
/// para as posições do texto dobrado baterem com as do original.
fn dobrar_char(c: char) -> char {
    // Qualquer letra com diacrítico vira a letra base pela decomposição (NFKD sem as marcas): á, ā, ă, ę, ő...
    // (terceira revisão de segurança de 15/11/2026: a tabela fixa deixava letras de fora).
    let base = if c.is_ascii() {
        c
    } else {
        use unicode_normalization::UnicodeNormalization;
        let mut sem_marca = std::iter::once(c)
            .nfkd()
            .filter(|x| !unicode_normalization::char::is_combining_mark(*x));
        match (sem_marca.next(), sem_marca.next()) {
            (Some(b), None) => b,
            _ => c,
        }
    };
    let mut minuscula = base.to_lowercase();
    match (minuscula.next(), minuscula.next()) {
        (Some(m), None) => m,
        _ => base,
    }
}

/// Marca combinante (acento "solto" e afins) que sobra depois da NFKC, nos blocos usados em ataques.
fn acento_solto(c: char) -> bool {
    unicode_normalization::char::is_combining_mark(c)
        || matches!(c,
        '\u{0300}'..='\u{036F}' | '\u{1AB0}'..='\u{1AFF}' | '\u{1DC0}'..='\u{1DFF}' | '\u{20D0}'..='\u{20FF}'
        | '\u{FE20}'..='\u{FE2F}')
}

/// Caracteres que não aparecem na tela mas quebram a comparação: acento solto (NFD), largura zero, marcas de
/// direção, junções invisíveis, BOM e hífen suave. Sem tratar isso, "Jose\u{301}" ou "Ma\u{200B}ria" passariam
/// pelo Higienizador sem virar marcador (revisão de segurança de 15/11/2026).
fn ignoravel(c: char) -> bool {
    acento_solto(c)
        || matches!(c,
            '\u{034F}' | '\u{00AD}' | '\u{061C}' | '\u{115F}' | '\u{1160}' | '\u{17B4}' | '\u{17B5}'
            | '\u{180B}'..='\u{180F}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{206F}' | '\u{3164}' | '\u{FE00}'..='\u{FE0F}'
            | '\u{FEFF}' | '\u{FFA0}' | '\u{FFF0}'..='\u{FFF8}' | '\u{1BCA0}'..='\u{1BCA3}'
            | '\u{1D173}'..='\u{1D17A}' | '\u{E0000}'..='\u{E0FFF}')
}

/// Forma única antes de procurar dado sensível (revisões de segurança de 15/11/2026): NFKC (compõe o acento
/// decomposto, troca dígito e "＠" de largura dupla pelos comuns) e tira todo invisível e marca combinante que
/// sobrar. Todas as regras (nomes, CPF, e-mail, telefone, números) olham ESTE texto, e é ele que sai higienizado.
/// Limite conhecido: letras de outro alfabeto parecidas com as latinas (homóglifos, ex. "а" cirílico) não são
/// trocadas; o motor online só recebe texto que passou por aqui, e o dado de aluno nem vai a ele (tarefa sensível).
fn normalizar(texto: &str) -> std::borrow::Cow<'_, str> {
    if texto.is_ascii() {
        return std::borrow::Cow::Borrowed(texto);
    }
    use unicode_normalization::UnicodeNormalization;
    std::borrow::Cow::Owned(
        texto
            .nfkc()
            .filter(|c| !ignoravel(*c))
            .map(digito_ascii)
            .collect(),
    )
}

/// Primeiro código (o "0") de cada bloco de dígitos decimais Unicode (categoria Nd) fora do ASCII que a NFKC não
/// troca. As regras só aceitam [0-9]; isto converte "٣٤٥" e afins antes (terceira revisão de 15/11/2026).
const ZEROS_ND: &[u32] = &[
    0x0660, 0x06F0, 0x07C0, 0x0966, 0x09E6, 0x0A66, 0x0AE6, 0x0B66, 0x0BE6, 0x0C66, 0x0CE6, 0x0D66, 0x0DE6,
    0x0E50, 0x0ED0, 0x0F20, 0x1040, 0x1090, 0x17E0, 0x1810, 0x1946, 0x19D0, 0x1A80, 0x1A90, 0x1B50, 0x1BB0,
    0x1C40, 0x1C50, 0xA620, 0xA8D0, 0xA900, 0xA9D0, 0xA9F0, 0xAA50, 0xABF0, 0xFF10, 0x104A0, 0x10D30, 0x11066,
    0x110F0, 0x11136, 0x111D0, 0x112F0, 0x11450, 0x114D0, 0x11650, 0x116C0, 0x11730, 0x118E0, 0x11950, 0x11C50,
    0x11D50, 0x11DA0, 0x16A60, 0x16AC0, 0x16B50, 0x1D7CE, 0x1D7D8, 0x1D7E2, 0x1D7EC, 0x1D7F6, 0x1E140, 0x1E2F0,
    0x1E950, 0x1FBF0,
];

fn digito_ascii(c: char) -> char {
    if c.is_ascii() || !c.is_numeric() {
        return c;
    }
    let v = c as u32;
    ZEROS_ND
        .iter()
        .find(|z| (**z..**z + 10).contains(&v))
        .and_then(|z| char::from_digit(v - z, 10))
        .unwrap_or(c)
}

/// Minúsculas e sem acento (útil para comparar nomes e montar buscas). Ignora os caracteres invisíveis.
pub fn dobrar(texto: &str) -> String {
    normalizar(texto).chars().map(dobrar_char).collect()
}

fn so_digitos(texto: &str) -> String {
    texto.chars().filter(char::is_ascii_digit).collect()
}

/// Confere os dois dígitos verificadores de um CPF de 11 dígitos (sem pontuação).
fn cpf_valido(digitos: &str) -> bool {
    let d: Vec<u32> = digitos.chars().filter_map(|c| c.to_digit(10)).collect();
    if d.len() != 11 || d.iter().all(|x| *x == d[0]) {
        return false;
    }
    let dv = |n: usize| -> u32 {
        let soma: u32 = d[..n]
            .iter()
            .enumerate()
            .map(|(i, x)| x * (n as u32 + 1 - i as u32))
            .sum();
        let resto = (soma * 10) % 11;
        if resto == 10 {
            0
        } else {
            resto
        }
    };
    dv(9) == d[9] && dv(10) == d[10]
}

/// O achado não pode estar "colado" em outro dígito (senão é pedaço de um número maior).
fn limites_numericos(texto: &str, ini: usize, fim: usize) -> bool {
    let antes = texto[..ini].chars().next_back();
    let depois = texto[fim..].chars().next();
    !antes.is_some_and(|c| c.is_ascii_digit()) && !depois.is_some_and(|c| c.is_ascii_digit())
}

/// Chave de comparação: o mesmo valor escrito de jeitos diferentes vira a mesma chave (e o mesmo marcador).
fn chave_de(categoria: Categoria, original: &str) -> String {
    match categoria {
        Categoria::Cpf | Categoria::NumeroLongo => so_digitos(original),
        Categoria::Rg => dobrar(original)
            .chars()
            .filter(|c| c.is_ascii_digit() || *c == 'x')
            .collect(),
        Categoria::Telefone => {
            let d = so_digitos(original);
            match d.strip_prefix("55") {
                Some(resto) if d.len() >= 12 => resto.to_owned(),
                _ => d,
            }
        }
        Categoria::Email => dobrar(original),
        Categoria::Aluno => tokens_de(&dobrar(original)).join(" "),
    }
}

/// Divide um texto já dobrado em palavras (sequências alfanuméricas).
fn tokens_de(dobrado: &str) -> Vec<String> {
    dobrado
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .collect()
}

#[derive(Debug, Clone, Copy)]
struct Achado {
    ini: usize,
    fim: usize,
    categoria: Categoria,
}

struct Padroes {
    email: Regex,
    cpf: Regex,
    rg: Regex,
    telefone: Regex,
    numero: Regex,
}

impl Padroes {
    fn compilar() -> Result<Self> {
        let r = |p: &str| Regex::new(p).map_err(|_| ErroIa::Configuracao("padrão de higiene inválido"));
        Ok(Self {
            email: r(r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9\-]+(?:\.[A-Za-z0-9\-]+)*\.[A-Za-z]{2,}")?,
            // Separador entre os grupos: ponto, hífen, barra, espaço ou tabulação (revisão de 15/11/2026).
            cpf: r(r"[0-9]{3}[ \t./\-]?[0-9]{3}[ \t./\-]?[0-9]{3}[ \t./\-]?[0-9]{2}")?,
            rg: r(r"[0-9]{1,2}\.[0-9]{3}\.[0-9]{3}-[0-9Xx]")?,
            // Com DDD: 4 ou 5 dígitos + 4. Sem DDD: só o celular 9xxxx-xxxx (evita "2025-2026").
            telefone: r(
                r"(?:\+55\s?)?(?:\([0-9]{2}\)\s?|[0-9]{2}[\s.\-])(?:9[0-9]{4}|[0-9]{4})[\s.\-]?[0-9]{4}|9[0-9]{4}[\s.\-][0-9]{4}",
            )?,
            numero: r(r"[0-9]{9,}")?,
        })
    }
}

/// Acha e troca dado sensível por marcadores. Ver o topo do módulo.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct Higienizador {
    #[zeroize(skip)]
    padroes: Padroes,
    /// Cada nome já dobrado e dividido em palavras; ordem: mais palavras primeiro.
    nomes: Vec<Vec<String>>,
    /// Valores públicos que não contam como sensíveis (ex.: e-mail institucional de ouvidoria).
    permitidos: Vec<String>,
}

impl std::fmt::Debug for Higienizador {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Higienizador")
            .field("nomes", &self.nomes.len())
            .field("permitidos", &self.permitidos.len())
            .finish()
    }
}

impl Higienizador {
    /// Higienizador sem nomes (só os padrões fixos).
    pub fn novo() -> Result<Self> {
        Ok(Self {
            padroes: Padroes::compilar()?,
            nomes: Vec::new(),
            permitidos: Vec::new(),
        })
    }

    /// Acrescenta os nomes da lista (vinda do banco em tempo de execução).
    pub fn com_nomes<I, S>(mut self, nomes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for nome in nomes {
            self.adicionar_nome(nome.as_ref());
        }
        self
    }

    /// Acrescenta um nome. Nomes com menos de 2 letras são ignorados (marcariam letras soltas).
    pub fn adicionar_nome(&mut self, nome: &str) {
        let tokens = tokens_de(&dobrar(nome));
        let letras: usize = tokens.iter().map(|t| t.chars().count()).sum();
        if letras < 2 || self.nomes.contains(&tokens) {
            return;
        }
        self.nomes.push(tokens);
        self.nomes.sort_by_key(|t| std::cmp::Reverse(t.len()));
    }

    /// Zera e esquece todos os nomes.
    pub fn limpar_nomes(&mut self) {
        self.nomes.zeroize();
    }

    /// Quantos nomes estão na lista.
    pub fn quantidade_nomes(&self) -> usize {
        self.nomes.len()
    }

    /// Marca um valor público como permitido (não é trocado nem recusado). Ex.: e-mail de ouvidoria.
    pub fn permitir(&mut self, valor: &str) {
        let dobrado = dobrar(valor.trim());
        let digitos = so_digitos(valor);
        if !dobrado.is_empty() {
            self.permitidos.push(dobrado);
        }
        if digitos.len() >= 8 {
            self.permitidos.push(digitos);
        }
    }

    fn encontrar(&self, texto: &str) -> Vec<Achado> {
        let mut achados: Vec<Achado> = Vec::new();
        let livre = |achados: &[Achado], ini: usize, fim: usize| {
            achados.iter().all(|a| fim <= a.ini || ini >= a.fim)
        };
        let aceitar = |achados: &mut Vec<Achado>, ini: usize, fim: usize, categoria: Categoria| {
            if livre(achados, ini, fim) {
                achados.push(Achado { ini, fim, categoria });
            }
        };

        // Ordem = prioridade quando dois padrões se sobrepõem.
        for m in self.padroes.email.find_iter(texto) {
            aceitar(&mut achados, m.start(), m.end(), Categoria::Email);
        }
        for m in self.padroes.cpf.find_iter(texto) {
            if !limites_numericos(texto, m.start(), m.end()) {
                continue;
            }
            let digitos = so_digitos(m.as_str());
            // Só ponto/hífen/barra contam como "pontuação de CPF"; separado por espaço precisa do dígito certo.
            let pontuado = m.as_str().chars().any(|c| matches!(c, '.' | '-' | '/'));
            // Pontuado no formato do CPF: trata como CPF mesmo com dígito errado (melhor sobrar).
            // Sem pontuação: só se o dígito verificador bate; senão cai na regra de número longo (RA).
            if pontuado || cpf_valido(&digitos) {
                aceitar(&mut achados, m.start(), m.end(), Categoria::Cpf);
            }
        }
        for m in self.padroes.rg.find_iter(texto) {
            let depois = texto[m.end()..].chars().next();
            if limites_numericos(texto, m.start(), m.end())
                && !depois.is_some_and(char::is_alphanumeric)
            {
                aceitar(&mut achados, m.start(), m.end(), Categoria::Rg);
            }
        }
        for m in self.padroes.telefone.find_iter(texto) {
            if limites_numericos(texto, m.start(), m.end()) {
                aceitar(&mut achados, m.start(), m.end(), Categoria::Telefone);
            }
        }
        for m in self.padroes.numero.find_iter(texto) {
            aceitar(&mut achados, m.start(), m.end(), Categoria::NumeroLongo);
        }
        for (ini, fim) in self.encontrar_nomes(texto) {
            aceitar(&mut achados, ini, fim, Categoria::Aluno);
        }

        if !self.permitidos.is_empty() {
            achados.retain(|a| {
                let chave = chave_de(a.categoria, &texto[a.ini..a.fim]);
                !self.permitidos.contains(&chave)
            });
        }
        achados.sort_by_key(|a| a.ini);
        achados
    }

    /// Posições (em bytes do texto original) dos nomes da lista.
    fn encontrar_nomes(&self, texto: &str) -> Vec<(usize, usize)> {
        if self.nomes.is_empty() {
            return Vec::new();
        }
        // Os invisíveis (acento solto, largura zero) saem da comparação, mas as posições continuam sendo as do
        // texto original: o trecho trocado pelo marcador inclui esses caracteres.
        let posicoes: Vec<usize> = texto
            .char_indices()
            .filter(|(_, c)| !ignoravel(*c))
            .map(|(i, _)| i)
            .collect();
        let dobrado: Zeroizing<Vec<char>> = Zeroizing::new(
            texto
                .chars()
                .filter(|c| !ignoravel(*c))
                .map(dobrar_char)
                .collect(),
        );
        let byte_de = |i: usize| posicoes.get(i).copied().unwrap_or(texto.len());

        // Palavras do texto, em índices de caractere [ini, fim).
        let mut palavras: Vec<(usize, usize)> = Vec::new();
        let mut inicio: Option<usize> = None;
        for (i, c) in dobrado.iter().enumerate() {
            match (c.is_alphanumeric(), inicio) {
                (true, None) => inicio = Some(i),
                (false, Some(s)) => {
                    palavras.push((s, i));
                    inicio = None;
                }
                _ => {}
            }
        }
        if let Some(s) = inicio {
            palavras.push((s, dobrado.len()));
        }

        let igual = |(s, e): (usize, usize), token: &str| dobrado[s..e].iter().copied().eq(token.chars());
        // Entre as palavras de um nome: 1 a 3 caracteres não alfanuméricos ("Ana-Clara", "Maria  Souza").
        let separador_ok = |fim_anterior: usize, ini_proximo: usize| {
            let gap = ini_proximo - fim_anterior;
            (1..=3).contains(&gap)
        };

        let mut achados = Vec::new();
        let mut t = 0;
        while t < palavras.len() {
            let mut casou = None;
            for nome in &self.nomes {
                let n = nome.len();
                if n == 0 || t + n > palavras.len() {
                    continue;
                }
                let ok = (0..n).all(|k| {
                    igual(palavras[t + k], &nome[k])
                        && (k == 0 || separador_ok(palavras[t + k - 1].1, palavras[t + k].0))
                });
                if ok {
                    casou = Some(n);
                    break;
                }
            }
            match casou {
                Some(n) => {
                    achados.push((byte_de(palavras[t].0), byte_de(palavras[t + n - 1].1)));
                    t += n;
                }
                None => t += 1,
            }
        }
        achados
    }

    /// Troca o dado sensível por marcadores e devolve o texto higienizado com um mapa novo.
    pub fn higienizar(&self, texto: &str) -> (String, MapaMarcadores) {
        let mut mapa = MapaMarcadores::default();
        let limpo = self.higienizar_com(texto, &mut mapa);
        (limpo, mapa)
    }

    /// Como `higienizar`, mas reaproveita um mapa (o mesmo valor em várias mensagens = o mesmo marcador).
    pub fn higienizar_com(&self, texto: &str, mapa: &mut MapaMarcadores) -> String {
        let texto = normalizar(texto);
        let texto = texto.as_ref();
        let mut saida = String::with_capacity(texto.len());
        let mut cursor = 0;
        for a in self.encontrar(texto) {
            saida.push_str(&texto[cursor..a.ini]);
            let original = &texto[a.ini..a.fim];
            saida.push_str(&mapa.marcador_para(a.categoria, original));
            cursor = a.fim;
        }
        saida.push_str(&texto[cursor..]);
        saida
    }

    /// Modo recusa: erro com a categoria do primeiro dado sensível achado (o valor nunca vai no erro).
    pub fn recusar(&self, texto: &str) -> Result<()> {
        match self.encontrar(&normalizar(texto)).first() {
            Some(a) => Err(ErroIa::DadoSensivelBloqueado(a.categoria.rotulo())),
            None => Ok(()),
        }
    }

    /// `true` se o texto tem algum dado sensível.
    pub fn contem_dado_sensivel(&self, texto: &str) -> bool {
        !self.encontrar(&normalizar(texto)).is_empty()
    }
}

#[derive(Zeroize)]
struct Entrada {
    marcador: String,
    chave: String,
    original: String,
    #[zeroize(skip)]
    categoria: Categoria,
}

/// Marcador -> valor original. Fica só em memória, é zerado no `Drop` e o `Debug` não mostra valores.
#[derive(Default, Zeroize, ZeroizeOnDrop)]
pub struct MapaMarcadores {
    entradas: Vec<Entrada>,
    contadores: [u32; N_CATEGORIAS],
}

impl std::fmt::Debug for MapaMarcadores {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MapaMarcadores")
            .field("marcadores", &self.entradas.len())
            .finish_non_exhaustive()
    }
}

impl MapaMarcadores {
    fn marcador_para(&mut self, categoria: Categoria, original: &str) -> String {
        let chave = chave_de(categoria, original);
        if let Some(e) = self
            .entradas
            .iter()
            .find(|e| e.categoria == categoria && e.chave == chave)
        {
            return e.marcador.clone();
        }
        let contador = &mut self.contadores[categoria.indice()];
        *contador += 1;
        let marcador = format!("[{}_{}]", categoria.rotulo(), contador);
        self.entradas.push(Entrada {
            marcador: marcador.clone(),
            chave,
            original: original.to_owned(),
            categoria,
        });
        marcador
    }

    /// Quantos marcadores diferentes o mapa tem.
    pub fn len(&self) -> usize {
        self.entradas.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entradas.is_empty()
    }

    /// Os marcadores em uso (sem os valores), na ordem em que apareceram.
    pub fn marcadores(&self) -> Vec<String> {
        self.entradas.iter().map(|e| e.marcador.clone()).collect()
    }

    /// Troca cada marcador conhecido pelo valor original (numa passada só). Marcador desconhecido fica.
    pub fn restaurar(&self, texto: &str) -> Zeroizing<String> {
        let mut saida = Zeroizing::new(String::with_capacity(texto.len()));
        let mut resto = texto;
        while let Some(abre) = resto.find('[') {
            saida.push_str(&resto[..abre]);
            let depois = &resto[abre..];
            let trocado = depois.find(']').and_then(|fecha| {
                let candidato = &depois[..=fecha];
                self.entradas
                    .iter()
                    .find(|e| e.marcador == candidato)
                    .map(|e| (e.original.as_str(), fecha + 1))
            });
            match trocado {
                Some((original, tamanho)) => {
                    saida.push_str(original);
                    resto = &depois[tamanho..];
                }
                None => {
                    saida.push('[');
                    resto = &depois[1..];
                }
            }
        }
        saida.push_str(resto);
        saida
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Gera um CPF de teste com dígito verificador válido a partir de 9 dígitos inventados.
    pub(crate) fn cpf_de_teste(base: [u32; 9]) -> String {
        let mut d: Vec<u32> = base.to_vec();
        for n in [9usize, 10] {
            let soma: u32 = d.iter().enumerate().map(|(i, x)| x * (n as u32 + 1 - i as u32)).sum();
            let resto = (soma * 10) % 11;
            d.push(if resto == 10 { 0 } else { resto });
        }
        d.iter().map(|x| char::from_digit(*x, 10).unwrap()).collect()
    }

    fn pontuar(cpf: &str) -> String {
        format!("{}.{}.{}-{}", &cpf[0..3], &cpf[3..6], &cpf[6..9], &cpf[9..11])
    }

    fn h() -> Higienizador {
        Higienizador::novo()
            .unwrap()
            .com_nomes(["Joana Teste Silva", "Ícaro Exemplo", "Beto"])
    }

    #[test]
    fn cpf_gerado_no_teste_e_valido() {
        let cpf = cpf_de_teste([1, 2, 3, 4, 5, 6, 7, 8, 0]);
        assert!(cpf_valido(&cpf));
        assert!(!cpf_valido("11111111111"));
    }

    #[test]
    fn cpf_com_e_sem_pontuacao_vira_o_mesmo_marcador() {
        let cpf = cpf_de_teste([5, 2, 9, 9, 8, 2, 2, 4, 7]);
        let texto = format!("CPF {} ou {} no cadastro.", pontuar(&cpf), cpf);
        let (limpo, mapa) = h().higienizar(&texto);
        assert_eq!(limpo, "CPF [CPF_1] ou [CPF_1] no cadastro.");
        assert_eq!(mapa.len(), 1);
        assert!(!limpo.contains(&cpf));
    }

    #[test]
    fn email_telefone_rg_e_ra_viram_marcadores() {
        let texto = "Escreva para fulano.inventado@exemplo.test, ligue (11) 98765-4321 ou \
                     11 3456-7890; RG 12.345.678-X; RA 000123456789-SP.";
        let (limpo, _) = h().higienizar(texto);
        assert_eq!(
            limpo,
            "Escreva para [EMAIL_1], ligue [TELEFONE_1] ou [TELEFONE_2]; RG [RG_1]; RA [RA_1]-SP."
        );
    }

    #[test]
    fn nomes_sem_diferenciar_maiusculas_nem_acentos() {
        let texto = "JOANA teste SÍLVA faltou; joana teste silva voltou. Icaro exemplo e Beto vieram.";
        let (limpo, mapa) = h().higienizar(texto);
        assert_eq!(
            limpo,
            "[ALUNO_1] faltou; [ALUNO_1] voltou. [ALUNO_2] e [ALUNO_3] vieram."
        );
        assert_eq!(mapa.len(), 3);
        // Palavra que só começa com o nome não é trocada.
        let (limpo, _) = h().higienizar("Betoneira e Robeto ficam.");
        assert_eq!(limpo, "Betoneira e Robeto ficam.");
    }

    #[test]
    fn nome_em_forma_decomposta_ou_com_invisivel_tambem_vira_marcador() {
        // Revisão de segurança de 15/11/2026: acento solto (NFD) e largura zero não podem driblar a lista.
        let nfd = "JOANA teste SI\u{301}LVA faltou";
        let (limpo, _) = h().higienizar(nfd);
        assert_eq!(limpo, "[ALUNO_1] faltou");
        let zw = "Jo\u{200B}ana te\u{200D}ste Silva e Be\u{FEFF}to";
        let (limpo, _) = h().higienizar(zw);
        assert_eq!(limpo, "[ALUNO_1] e [ALUNO_2]");
        assert!(h().recusar("aviso para Be\u{00AD}to").is_err());
        let cpf = cpf_de_teste([3, 1, 4, 1, 5, 9, 2, 6, 5]);
        let quebrado: String = cpf.chars().flat_map(|c| [c, '\u{200B}']).collect();
        assert!(h().contem_dado_sensivel(&format!("cpf {quebrado} ok")));
        // Segunda revisão: CGJ (U+034F), seletor de variação (U+FE0F) e acento solto depois do dígito.
        for sep in ['\u{034F}', '\u{FE0F}', '\u{0301}', '\u{E0041}'] {
            let q: String = cpf.chars().flat_map(|c| [c, sep]).collect();
            assert!(h().contem_dado_sensivel(&format!("cpf {q} ok")), "separador U+{:04X}", sep as u32);
            let nome = format!("Jo{sep}ana Teste Si{sep}lva");
            assert_eq!(h().higienizar(&nome).0, "[ALUNO_1]", "nome com U+{:04X}", sep as u32);
        }
        // Dígitos e arroba de largura dupla viram os comuns (NFKC).
        let largo: String = cpf
            .chars()
            .map(|c| char::from_u32(c as u32 - '0' as u32 + 0xFF10).unwrap_or(c))
            .collect();
        assert!(h().contem_dado_sensivel(&format!("cpf {largo}")));
        assert!(h().contem_dado_sensivel("fulano\u{FF20}exemplo.test"));
        // Terceira revisão: letras latinas com outros diacríticos e dígitos de outros sistemas de escrita.
        assert_eq!(h().higienizar("Jo\u{0101}na Te\u{0306}ste S\u{012B}lv\u{0105}").0, "[ALUNO_1]");
        let arabe: String = cpf
            .chars()
            .map(|c| char::from_u32(c as u32 - '0' as u32 + 0x0660).unwrap_or(c))
            .collect();
        assert!(h().contem_dado_sensivel(&format!("cpf {arabe}")));
        let devanagari: String = cpf
            .chars()
            .map(|c| char::from_u32(c as u32 - '0' as u32 + 0x0966).unwrap_or(c))
            .collect();
        assert!(h().contem_dado_sensivel(&format!("doc {devanagari}")));
        // CPF separado por espaço só conta com o dígito verificador certo.
        let espacado = format!("{} {} {} {}", &cpf[0..3], &cpf[3..6], &cpf[6..9], &cpf[9..11]);
        assert!(h().contem_dado_sensivel(&format!("doc {espacado} fim")));
    }

    #[test]
    fn restaurar_devolve_o_texto_original() {
        let cpf = cpf_de_teste([3, 1, 4, 1, 5, 9, 2, 6, 5]);
        let texto = format!(
            "Joana Teste Silva (CPF {}, e-mail joana.inventada@exemplo.test, tel 98888-7777) e Beto [nota].",
            pontuar(&cpf)
        );
        let (limpo, mapa) = h().higienizar(&texto);
        assert!(!limpo.contains("Joana"));
        assert_eq!(mapa.restaurar(&limpo).as_str(), texto);
        // O modelo pode repetir um marcador: restaura todos.
        assert_eq!(
            mapa.restaurar("[ALUNO_1] e [ALUNO_1]; [X] fica").as_str(),
            "Joana Teste Silva e Joana Teste Silva; [X] fica"
        );
    }

    #[test]
    fn mesmo_valor_mesmo_marcador_entre_mensagens() {
        let hig = h();
        let mut mapa = MapaMarcadores::default();
        let a = hig.higienizar_com("Beto e Ícaro Exemplo", &mut mapa);
        let b = hig.higienizar_com("ícaro exemplo, depois beto", &mut mapa);
        assert_eq!(a, "[ALUNO_1] e [ALUNO_2]");
        assert_eq!(b, "[ALUNO_2], depois [ALUNO_1]");
    }

    #[test]
    fn texto_sem_dado_sensivel_fica_igual() {
        let texto = "Ano letivo 2025-2026, aula 3 das 7h30, prazo de 16 dias, turma 2B.";
        let (limpo, mapa) = h().higienizar(texto);
        assert_eq!(limpo, texto);
        assert!(mapa.is_empty());
        assert!(h().recusar(texto).is_ok());
    }

    #[test]
    fn modo_recusa_falha_com_dado_sensivel_sem_mostrar_o_valor() {
        let hig = h();
        let cpf = cpf_de_teste([9, 8, 7, 6, 5, 4, 3, 2, 1]);
        for (texto, cat) in [
            (format!("cpf {cpf}"), "CPF"),
            ("fale com x.y@exemplo.test".to_owned(), "EMAIL"),
            ("ligue 97777-6666".to_owned(), "TELEFONE"),
            ("ra 123456789".to_owned(), "RA"),
            ("a aluna joana teste silva".to_owned(), "ALUNO"),
        ] {
            let erro = hig.recusar(&texto).unwrap_err();
            assert!(matches!(erro, ErroIa::DadoSensivelBloqueado(c) if c == cat));
            assert!(!erro.to_string().contains(&texto));
        }
    }

    #[test]
    fn valor_permitido_nao_e_recusado() {
        let mut hig = h();
        assert!(hig.recusar("ouvidoria@exemplo.invalid").is_err());
        hig.permitir("Ouvidoria@exemplo.invalid");
        assert!(hig.recusar("Escreva para ouvidoria@exemplo.invalid").is_ok());
    }

    #[test]
    fn debug_nao_mostra_valores() {
        let (_, mapa) = h().higienizar("Joana Teste Silva, ra 987654321");
        let dbg = format!("{mapa:?} {:?}", h());
        assert!(!dbg.to_lowercase().contains("joana"));
        assert!(!dbg.contains("987654321"));
        assert!(dbg.contains("marcadores: 2"));
    }

    #[test]
    fn limpar_nomes_esquece_a_lista() {
        let mut hig = h();
        assert_eq!(hig.quantidade_nomes(), 3);
        hig.limpar_nomes();
        assert_eq!(hig.quantidade_nomes(), 0);
        assert!(hig.recusar("Beto").is_ok());
    }
}
