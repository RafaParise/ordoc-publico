//! Memória da IA (desenho, seção 5.2): tabelas `ia_receitas`, `ia_textos`, `ia_licoes` (as três com FTS5),
//! `ia_normas`, `ia_execucoes` e `ia_cache`, sobre uma `rusqlite::Connection` recebida de quem chama
//! (SQLCipher no Ordoc; SQLite comum no Univo).
//!
//! Regra: TODA gravação passa pelo `Higienizador` em modo recusa. Se achar CPF, e-mail, telefone, RA ou
//! nome da lista, não grava e devolve `ErroIa::DadoSensivelBloqueado`.
//!
//! A versão do esquema fica na tabela `ia_meta` (não no `PRAGMA user_version`, que é das migrações de
//! quem chama, como o ordoc-storage).

use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::{ErroIa, Result};
use crate::higiene::{dobrar, Higienizador};
use crate::licao::{Grau, Licao, OrigemLicao};
use crate::receita::Receita;

const VERSAO_ESQUEMA: i64 = 1;
const TOKENIZADOR: &str = "tokenize = 'unicode61 remove_diacritics 2'";

fn migracao_v1() -> String {
    format!(
        "
        CREATE TABLE ia_receitas (
            id TEXT PRIMARY KEY,
            versao INTEGER NOT NULL CHECK (versao >= 1),
            intencao TEXT NOT NULL,
            portal TEXT NOT NULL,
            titulo TEXT NOT NULL,
            quando_usar TEXT NOT NULL,
            entradas TEXT NOT NULL,
            passos TEXT NOT NULL,
            pontos_sim TEXT NOT NULL,
            saida TEXT NOT NULL,
            erros_licoes TEXT NOT NULL,
            privacidade TEXT NOT NULL,
            texto_md TEXT NOT NULL,
            atualizado_em INTEGER NOT NULL
        ) STRICT;
        CREATE VIRTUAL TABLE ia_receitas_fts USING fts5(
            id UNINDEXED, nome, titulo, intencao, quando_usar, passos, erros_licoes, {TOKENIZADOR}
        );

        CREATE TABLE ia_textos (
            id TEXT PRIMARY KEY,
            titulo TEXT NOT NULL,
            canal TEXT NOT NULL,
            corpo TEXT NOT NULL,
            normas TEXT NOT NULL CHECK (json_valid(normas)),
            atualizado_em INTEGER NOT NULL
        ) STRICT;
        CREATE VIRTUAL TABLE ia_textos_fts USING fts5(id UNINDEXED, titulo, canal, corpo, {TOKENIZADOR});

        CREATE TABLE ia_licoes (
            id INTEGER PRIMARY KEY,
            origem TEXT NOT NULL CHECK (origem IN ('claude', 'professor', 'agente')),
            area TEXT NOT NULL,
            erro TEXT NOT NULL,
            certo TEXT NOT NULL,
            evidencia TEXT,
            grau TEXT NOT NULL CHECK (grau IN ('Certo', 'Provável', 'Palpite')),
            criado_em INTEGER NOT NULL
        ) STRICT;
        CREATE VIRTUAL TABLE ia_licoes_fts USING fts5(area, erro, certo, {TOKENIZADOR});

        CREATE TABLE ia_normas (
            id TEXT PRIMARY KEY,
            citacao TEXT NOT NULL,
            fonte_url TEXT,
            grau TEXT NOT NULL CHECK (grau IN ('Certo', 'Provável', 'Palpite')),
            verificada_em TEXT NOT NULL
        ) STRICT;

        CREATE TABLE ia_execucoes (
            id INTEGER PRIMARY KEY,
            receita_id TEXT NOT NULL,
            receita_versao INTEGER NOT NULL,
            hash_entradas TEXT NOT NULL CHECK (length(hash_entradas) = 64),
            passos INTEGER NOT NULL,
            sim_dados INTEGER NOT NULL,
            sim_negados INTEGER NOT NULL,
            duracao_ms INTEGER NOT NULL,
            resultado TEXT NOT NULL CHECK (resultado IN ('concluida', 'cancelada', 'falhou')),
            veredito TEXT CHECK (veredito IS NULL OR veredito IN ('ok', 'erro', 'parcial')),
            nota TEXT,
            criado_em INTEGER NOT NULL
        ) STRICT;
        CREATE INDEX ia_execucoes_receita ON ia_execucoes (receita_id, receita_versao);

        CREATE TABLE ia_cache (
            chave TEXT PRIMARY KEY CHECK (length(chave) = 64),
            resposta TEXT NOT NULL,
            criado_em INTEGER NOT NULL,
            expira_em INTEGER NOT NULL
        ) STRICT;
        "
    )
}

fn agora_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn para_u32(v: i64) -> Result<u32> {
    u32::try_from(v).map_err(|_| ErroIa::ValorInvalido("número fora do intervalo"))
}

fn hex_ok(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

fn hash_partes(mut h: blake3::Hasher, partes: &[&str]) -> String {
    for p in partes {
        h.update(&(p.len() as u64).to_le_bytes());
        h.update(p.as_bytes());
    }
    h.finalize().to_hex().to_string()
}

/// Hash das entradas de uma execução, com chave (sem chave, um CPF ou RA daria para achar por força
/// bruta). No Ordoc: `master_key.derive_subkey("ordoc.ia.execucoes.v1")`.
pub fn hash_entradas(chave: &[u8; 32], partes: &[&str]) -> String {
    hash_partes(blake3::Hasher::new_keyed(chave), partes)
}

/// Chave do cache: blake3(modelo | versão da receita | texto já higienizado).
pub fn chave_cache(modelo: &str, versao_receita: &str, texto_higienizado: &str) -> String {
    hash_partes(blake3::Hasher::new(), &[modelo, versao_receita, texto_higienizado])
}

/// Consulta FTS5 segura: só palavras alfanuméricas (>= 2 letras) entre aspas, unidas por OR.
fn consulta_fts(consulta: &str) -> Option<String> {
    let mut tokens: Vec<String> = Vec::new();
    for t in dobrar(consulta).split(|c: char| !c.is_alphanumeric()) {
        if t.chars().count() >= 2 && !tokens.iter().any(|x| x == t) {
            tokens.push(t.to_owned());
        }
    }
    if tokens.is_empty() {
        None
    } else {
        Some(
            tokens
                .iter()
                .map(|t| format!("\"{t}\""))
                .collect::<Vec<_>>()
                .join(" OR "),
        )
    }
}

fn limite_sql(limite: usize) -> Result<i64> {
    if limite == 0 {
        return Err(ErroIa::ValorInvalido("o limite da busca precisa ser > 0"));
    }
    Ok(i64::try_from(limite).unwrap_or(i64::MAX))
}

/// Resultado de busca (maior `pontuacao` = mais relevante; é o BM25 com sinal trocado).
#[derive(Debug, Clone, PartialEq)]
pub struct Achado {
    pub id: String,
    pub titulo: String,
    pub pontuacao: f64,
}

/// Modelo de texto com marcadores (`[ALUNO]`, `[TURMA]`...) e as normas que ele cita.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextoModelo {
    pub id: String,
    pub titulo: String,
    pub canal: String,
    pub corpo: String,
    pub normas: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Norma {
    pub id: String,
    pub citacao: String,
    /// Sem fonte, a norma fica gravada mas não deve ser citada (desenho, etapa 5).
    pub fonte_url: Option<String>,
    pub grau: Grau,
    /// Data da última conferência (ex.: "2026-11-15").
    pub verificada_em: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LicaoGravada {
    pub id: i64,
    pub licao: Licao,
    pub criado_em: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultadoExecucao {
    Concluida,
    Cancelada,
    Falhou,
}

impl ResultadoExecucao {
    fn como_texto(self) -> &'static str {
        match self {
            ResultadoExecucao::Concluida => "concluida",
            ResultadoExecucao::Cancelada => "cancelada",
            ResultadoExecucao::Falhou => "falhou",
        }
    }

    fn de_texto(t: &str) -> Result<Self> {
        match t {
            "concluida" => Ok(ResultadoExecucao::Concluida),
            "cancelada" => Ok(ResultadoExecucao::Cancelada),
            "falhou" => Ok(ResultadoExecucao::Falhou),
            _ => Err(ErroIa::ValorInvalido("resultado de execução desconhecido")),
        }
    }
}

/// Avaliação do professor na tela Revisar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Veredito {
    Ok,
    Erro,
    Parcial,
}

impl Veredito {
    fn como_texto(self) -> &'static str {
        match self {
            Veredito::Ok => "ok",
            Veredito::Erro => "erro",
            Veredito::Parcial => "parcial",
        }
    }

    fn de_texto(t: &str) -> Result<Self> {
        match t {
            "ok" => Ok(Veredito::Ok),
            "erro" => Ok(Veredito::Erro),
            "parcial" => Ok(Veredito::Parcial),
            _ => Err(ErroIa::ValorInvalido("veredito desconhecido")),
        }
    }
}

/// Registro de uma execução de receita: só contagens e hash (nunca as entradas).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistroExecucao {
    pub receita_id: String,
    pub receita_versao: u32,
    /// `hash_entradas(...)`: 64 caracteres hexadecimais minúsculos.
    pub hash_entradas: String,
    pub passos: u32,
    pub sim_dados: u32,
    pub sim_negados: u32,
    pub duracao_ms: u64,
    pub resultado: ResultadoExecucao,
    pub veredito: Option<Veredito>,
    pub nota: Option<String>,
}

/// As tabelas `ia_*` sobre uma conexão de quem chama.
pub struct MemoriaIa<'a> {
    conn: &'a Connection,
    higiene: &'a Higienizador,
}

impl std::fmt::Debug for MemoriaIa<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoriaIa")
            .field("higiene", self.higiene)
            .finish_non_exhaustive()
    }
}

impl<'a> MemoriaIa<'a> {
    /// Cria as tabelas que faltam (migração idempotente) e devolve a memória.
    pub fn abrir(conn: &'a Connection, higiene: &'a Higienizador) -> Result<Self> {
        let m = Self { conn, higiene };
        m.migrar()?;
        Ok(m)
    }

    fn migrar(&self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS ia_meta (chave TEXT PRIMARY KEY, valor INTEGER NOT NULL) STRICT;",
        )?;
        let versao: i64 = self
            .conn
            .query_row("SELECT valor FROM ia_meta WHERE chave = 'versao_esquema'", [], |r| r.get(0))
            .optional()?
            .unwrap_or(0);
        if versao > VERSAO_ESQUEMA {
            return Err(ErroIa::ValorInvalido(
                "o banco tem um esquema de IA mais novo que este programa",
            ));
        }
        if versao < 1 {
            let tx = self.conn.unchecked_transaction()?;
            tx.execute_batch(&migracao_v1())?;
            tx.execute(
                "INSERT INTO ia_meta (chave, valor) VALUES ('versao_esquema', ?1)
                 ON CONFLICT (chave) DO UPDATE SET valor = excluded.valor",
                params![VERSAO_ESQUEMA],
            )?;
            tx.commit()?;
        }
        Ok(())
    }

    /// Versão do esquema das tabelas `ia_*`.
    pub fn versao_esquema(&self) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT valor FROM ia_meta WHERE chave = 'versao_esquema'",
            [],
            |r| r.get(0),
        )?)
    }

    fn recusar_todos(&self, textos: &[&str]) -> Result<()> {
        for t in textos {
            self.higiene.recusar(t)?;
        }
        Ok(())
    }

    // --- Receitas --------------------------------------------------------------------------------

    /// Grava (ou atualiza) uma receita. Recusa dado sensível e versão menor que a já gravada.
    pub fn gravar_receita(&self, r: &Receita) -> Result<()> {
        r.validar()?;
        // Todos os campos que vão para o banco passam pela recusa (revisão de segurança de 15/11/2026).
        self.recusar_todos(&[
            &r.id,
            &r.intencao,
            &r.portal,
            &r.titulo,
            &r.quando_usar,
            &r.entradas,
            &r.passos,
            &r.pontos_sim,
            &r.saida,
            &r.erros_licoes,
            &r.privacidade,
            &r.texto_md,
        ])?;
        let tx = self.conn.unchecked_transaction()?;
        let atual: Option<i64> = tx
            .query_row("SELECT versao FROM ia_receitas WHERE id = ?1", [&r.id], |x| x.get(0))
            .optional()?;
        if atual.is_some_and(|v| v > i64::from(r.versao)) {
            return Err(ErroIa::ValorInvalido("já existe uma versão mais nova desta receita"));
        }
        tx.execute(
            "INSERT INTO ia_receitas (id, versao, intencao, portal, titulo, quando_usar, entradas, passos,
                 pontos_sim, saida, erros_licoes, privacidade, texto_md, atualizado_em)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
             ON CONFLICT (id) DO UPDATE SET versao = excluded.versao, intencao = excluded.intencao,
                 portal = excluded.portal, titulo = excluded.titulo, quando_usar = excluded.quando_usar,
                 entradas = excluded.entradas, passos = excluded.passos, pontos_sim = excluded.pontos_sim,
                 saida = excluded.saida, erros_licoes = excluded.erros_licoes,
                 privacidade = excluded.privacidade, texto_md = excluded.texto_md,
                 atualizado_em = excluded.atualizado_em",
            params![
                r.id,
                i64::from(r.versao),
                r.intencao,
                r.portal,
                r.titulo,
                r.quando_usar,
                r.entradas,
                r.passos,
                r.pontos_sim,
                r.saida,
                r.erros_licoes,
                r.privacidade,
                r.texto_md,
                agora_ms()
            ],
        )?;
        tx.execute("DELETE FROM ia_receitas_fts WHERE id = ?1", [&r.id])?;
        tx.execute(
            "INSERT INTO ia_receitas_fts (id, nome, titulo, intencao, quando_usar, passos, erros_licoes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                r.id,
                r.id.replace('-', " "),
                r.titulo,
                r.intencao,
                r.quando_usar,
                r.passos,
                r.erros_licoes
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Lê uma receita (reconstruída do Markdown integral gravado).
    pub fn receita(&self, id: &str) -> Result<Option<Receita>> {
        let md: Option<String> = self
            .conn
            .query_row("SELECT texto_md FROM ia_receitas WHERE id = ?1", [id], |r| r.get(0))
            .optional()?;
        md.map(|m| Receita::de_markdown(&m)).transpose()
    }

    pub fn quantidade_receitas(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT count(*) FROM ia_receitas", [], |r| r.get(0))?)
    }

    /// Busca BM25 nas receitas: nome (id), título, intenção, "quando usar", passos e lições.
    pub fn buscar_receitas(&self, consulta: &str, limite: usize) -> Result<Vec<Achado>> {
        let Some(q) = consulta_fts(consulta) else {
            return Ok(Vec::new());
        };
        let mut st = self.conn.prepare(
            "SELECT id, titulo, bm25(ia_receitas_fts, 0.0, 8.0, 6.0, 3.0, 4.0, 1.0, 1.0) AS r
             FROM ia_receitas_fts WHERE ia_receitas_fts MATCH ?1 ORDER BY r LIMIT ?2",
        )?;
        let linhas = st.query_map(params![q, limite_sql(limite)?], |x| {
            Ok(Achado {
                id: x.get(0)?,
                titulo: x.get(1)?,
                pontuacao: -x.get::<_, f64>(2)?,
            })
        })?;
        Ok(linhas.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    // --- Textos ----------------------------------------------------------------------------------

    pub fn gravar_texto(&self, t: &TextoModelo) -> Result<()> {
        if t.id.trim().is_empty() || t.corpo.trim().is_empty() {
            return Err(ErroIa::ValorInvalido("texto sem id ou sem corpo"));
        }
        let normas: Vec<&str> = t.normas.iter().map(String::as_str).collect();
        self.recusar_todos(&[&t.id, &t.titulo, &t.canal, &t.corpo])?;
        self.recusar_todos(&normas)?;
        let normas_json = serde_json::to_string(&normas)?;
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO ia_textos (id, titulo, canal, corpo, normas, atualizado_em)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (id) DO UPDATE SET titulo = excluded.titulo, canal = excluded.canal,
                 corpo = excluded.corpo, normas = excluded.normas, atualizado_em = excluded.atualizado_em",
            params![t.id, t.titulo, t.canal, t.corpo, normas_json, agora_ms()],
        )?;
        tx.execute("DELETE FROM ia_textos_fts WHERE id = ?1", [&t.id])?;
        tx.execute(
            "INSERT INTO ia_textos_fts (id, titulo, canal, corpo) VALUES (?1, ?2, ?3, ?4)",
            params![t.id, t.titulo, t.canal, t.corpo],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn texto(&self, id: &str) -> Result<Option<TextoModelo>> {
        let linha: Option<(String, String, String, String, String)> = self
            .conn
            .query_row(
                "SELECT id, titulo, canal, corpo, normas FROM ia_textos WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?;
        linha
            .map(|(id, titulo, canal, corpo, normas)| {
                Ok(TextoModelo {
                    id,
                    titulo,
                    canal,
                    corpo,
                    normas: serde_json::from_str(&normas)?,
                })
            })
            .transpose()
    }

    pub fn buscar_textos(&self, consulta: &str, limite: usize) -> Result<Vec<Achado>> {
        let Some(q) = consulta_fts(consulta) else {
            return Ok(Vec::new());
        };
        let mut st = self.conn.prepare(
            "SELECT id, titulo, bm25(ia_textos_fts, 0.0, 6.0, 2.0, 1.0) AS r
             FROM ia_textos_fts WHERE ia_textos_fts MATCH ?1 ORDER BY r LIMIT ?2",
        )?;
        let linhas = st.query_map(params![q, limite_sql(limite)?], |x| {
            Ok(Achado {
                id: x.get(0)?,
                titulo: x.get(1)?,
                pontuacao: -x.get::<_, f64>(2)?,
            })
        })?;
        Ok(linhas.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    // --- Lições ----------------------------------------------------------------------------------

    /// Grava uma lição e devolve o id.
    pub fn gravar_licao(&self, l: &Licao) -> Result<i64> {
        l.validar()?;
        self.recusar_todos(&[&l.area, &l.erro, &l.certo, l.evidencia.as_deref().unwrap_or("")])?;
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO ia_licoes (origem, area, erro, certo, evidencia, grau, criado_em)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                l.origem.como_texto(),
                l.area,
                l.erro,
                l.certo,
                l.evidencia,
                l.grau.como_texto(),
                agora_ms()
            ],
        )?;
        let id = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO ia_licoes_fts (rowid, area, erro, certo) VALUES (?1, ?2, ?3, ?4)",
            params![id, l.area, l.erro, l.certo],
        )?;
        tx.commit()?;
        Ok(id)
    }

    /// Busca BM25 nas lições (área, ERRO, CERTO).
    pub fn buscar_licoes(&self, consulta: &str, limite: usize) -> Result<Vec<LicaoGravada>> {
        let Some(q) = consulta_fts(consulta) else {
            return Ok(Vec::new());
        };
        let mut st = self.conn.prepare(
            "SELECT l.id, l.origem, l.area, l.erro, l.certo, l.evidencia, l.grau, l.criado_em,
                    bm25(ia_licoes_fts, 2.0, 3.0, 3.0) AS r
             FROM ia_licoes_fts JOIN ia_licoes l ON l.id = ia_licoes_fts.rowid
             WHERE ia_licoes_fts MATCH ?1 ORDER BY r LIMIT ?2",
        )?;
        let brutas = st
            .query_map(params![q, limite_sql(limite)?], |x| {
                Ok((
                    x.get::<_, i64>(0)?,
                    x.get::<_, String>(1)?,
                    x.get::<_, String>(2)?,
                    x.get::<_, String>(3)?,
                    x.get::<_, String>(4)?,
                    x.get::<_, Option<String>>(5)?,
                    x.get::<_, String>(6)?,
                    x.get::<_, i64>(7)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        brutas
            .into_iter()
            .map(|(id, origem, area, erro, certo, evidencia, grau, criado_em)| {
                Ok(LicaoGravada {
                    id,
                    licao: Licao {
                        origem: OrigemLicao::de_texto(&origem)
                            .ok_or(ErroIa::ValorInvalido("origem de lição desconhecida"))?,
                        area,
                        erro,
                        certo,
                        evidencia,
                        grau: Grau::de_texto(&grau)
                            .ok_or(ErroIa::ValorInvalido("grau desconhecido"))?,
                    },
                    criado_em,
                })
            })
            .collect()
    }

    // --- Normas ----------------------------------------------------------------------------------

    pub fn gravar_norma(&self, n: &Norma) -> Result<()> {
        if n.id.trim().is_empty() || n.citacao.trim().is_empty() {
            return Err(ErroIa::ValorInvalido("norma sem id ou sem citação"));
        }
        self.recusar_todos(&[
            &n.id,
            &n.citacao,
            n.fonte_url.as_deref().unwrap_or(""),
            &n.verificada_em,
        ])?;
        self.conn.execute(
            "INSERT INTO ia_normas (id, citacao, fonte_url, grau, verificada_em) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (id) DO UPDATE SET citacao = excluded.citacao, fonte_url = excluded.fonte_url,
                 grau = excluded.grau, verificada_em = excluded.verificada_em",
            params![n.id, n.citacao, n.fonte_url, n.grau.como_texto(), n.verificada_em],
        )?;
        Ok(())
    }

    pub fn norma(&self, id: &str) -> Result<Option<Norma>> {
        let linha: Option<(String, String, Option<String>, String, String)> = self
            .conn
            .query_row(
                "SELECT id, citacao, fonte_url, grau, verificada_em FROM ia_normas WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?;
        linha
            .map(|(id, citacao, fonte_url, grau, verificada_em)| {
                Ok(Norma {
                    id,
                    citacao,
                    fonte_url,
                    grau: Grau::de_texto(&grau).ok_or(ErroIa::ValorInvalido("grau desconhecido"))?,
                    verificada_em,
                })
            })
            .transpose()
    }

    // --- Execuções -------------------------------------------------------------------------------

    /// Registra uma execução e devolve o id. O hash precisa estar no formato de `hash_entradas`.
    pub fn registrar_execucao(&self, e: &RegistroExecucao) -> Result<i64> {
        if !hex_ok(&e.hash_entradas) {
            return Err(ErroIa::ValorInvalido(
                "hash_entradas precisa ter 64 caracteres hexadecimais minúsculos",
            ));
        }
        self.recusar_todos(&[&e.receita_id, e.nota.as_deref().unwrap_or("")])?;
        self.conn.execute(
            "INSERT INTO ia_execucoes (receita_id, receita_versao, hash_entradas, passos, sim_dados,
                 sim_negados, duracao_ms, resultado, veredito, nota, criado_em)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                e.receita_id,
                i64::from(e.receita_versao),
                e.hash_entradas,
                i64::from(e.passos),
                i64::from(e.sim_dados),
                i64::from(e.sim_negados),
                i64::try_from(e.duracao_ms).unwrap_or(i64::MAX),
                e.resultado.como_texto(),
                e.veredito.map(Veredito::como_texto),
                e.nota,
                agora_ms()
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// O professor avalia a execução (ok / erro / parcial + nota opcional).
    pub fn avaliar_execucao(&self, id: i64, veredito: Veredito, nota: Option<&str>) -> Result<()> {
        self.higiene.recusar(nota.unwrap_or(""))?;
        let n = self.conn.execute(
            "UPDATE ia_execucoes SET veredito = ?2, nota = ?3 WHERE id = ?1",
            params![id, veredito.como_texto(), nota],
        )?;
        if n == 0 {
            return Err(ErroIa::ValorInvalido("execução não encontrada"));
        }
        Ok(())
    }

    /// As últimas execuções (mais nova primeiro), para a tela Revisar.
    pub fn ultimas_execucoes(&self, limite: usize) -> Result<Vec<(i64, RegistroExecucao)>> {
        let mut st = self.conn.prepare(
            "SELECT id, receita_id, receita_versao, hash_entradas, passos, sim_dados, sim_negados,
                    duracao_ms, resultado, veredito, nota
             FROM ia_execucoes ORDER BY id DESC LIMIT ?1",
        )?;
        type Linha = (
            i64,
            String,
            i64,
            String,
            i64,
            i64,
            i64,
            i64,
            String,
            Option<String>,
            Option<String>,
        );
        let brutas: Vec<Linha> = st
            .query_map([limite_sql(limite)?], |x| {
                Ok((
                    x.get(0)?,
                    x.get(1)?,
                    x.get(2)?,
                    x.get(3)?,
                    x.get(4)?,
                    x.get(5)?,
                    x.get(6)?,
                    x.get(7)?,
                    x.get(8)?,
                    x.get(9)?,
                    x.get(10)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        brutas
            .into_iter()
            .map(|(id, receita_id, versao, hash, passos, sim, nao, dur, res, ver, nota)| {
                Ok((
                    id,
                    RegistroExecucao {
                        receita_id,
                        receita_versao: para_u32(versao)?,
                        hash_entradas: hash,
                        passos: para_u32(passos)?,
                        sim_dados: para_u32(sim)?,
                        sim_negados: para_u32(nao)?,
                        duracao_ms: u64::try_from(dur).unwrap_or(0),
                        resultado: ResultadoExecucao::de_texto(&res)?,
                        veredito: ver.as_deref().map(Veredito::de_texto).transpose()?,
                        nota,
                    },
                ))
            })
            .collect()
    }

    // --- Cache -----------------------------------------------------------------------------------

    /// Grava uma resposta (já higienizada) no cache. `chave` = `chave_cache(...)`.
    pub fn gravar_cache(&self, chave: &str, resposta: &str, agora_ms: i64, validade_ms: i64) -> Result<()> {
        if !hex_ok(chave) {
            return Err(ErroIa::ValorInvalido("chave de cache fora do formato"));
        }
        self.higiene.recusar(resposta)?;
        self.conn.execute(
            "INSERT INTO ia_cache (chave, resposta, criado_em, expira_em) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (chave) DO UPDATE SET resposta = excluded.resposta,
                 criado_em = excluded.criado_em, expira_em = excluded.expira_em",
            params![chave, resposta, agora_ms, agora_ms.saturating_add(validade_ms)],
        )?;
        Ok(())
    }

    /// Resposta do cache, se ainda válida em `agora_ms`.
    pub fn ler_cache(&self, chave: &str, agora_ms: i64) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT resposta FROM ia_cache WHERE chave = ?1 AND expira_em > ?2",
                params![chave, agora_ms],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Apaga o que venceu e devolve quantas linhas saíram.
    pub fn limpar_cache_vencido(&self, agora_ms: i64) -> Result<usize> {
        Ok(self
            .conn
            .execute("DELETE FROM ia_cache WHERE expira_em <= ?1", [agora_ms])?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::higiene::tests::cpf_de_teste;
    use crate::receita::tests::receita_md;
    use crate::receita::importar_pasta_receitas;

    fn hig() -> Higienizador {
        Higienizador::novo()
            .unwrap()
            .com_nomes(["Joana Teste Silva", "Beto Inventado"])
    }

    fn sensiveis() -> Vec<String> {
        vec![
            format!("cpf {}", cpf_de_teste([2, 7, 1, 8, 2, 8, 1, 8, 2])),
            "escreva para joana.inventada@exemplo.test".to_owned(),
            "ligue (11) 98765-4321".to_owned(),
            "RA 000123456789".to_owned(),
            "falar com joana teste silva".to_owned(),
        ]
    }

    fn assert_recusado(r: Result<impl std::fmt::Debug>) {
        assert!(
            matches!(r, Err(ErroIa::DadoSensivelBloqueado(_))),
            "deveria recusar: {r:?}"
        );
    }

    fn conta(conn: &Connection, tabela: &str) -> i64 {
        conn.query_row(&format!("SELECT count(*) FROM {tabela}"), [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn migracao_e_idempotente_e_cria_as_tabelas() {
        let conn = Connection::open_in_memory().unwrap();
        let h = hig();
        MemoriaIa::abrir(&conn, &h).unwrap();
        let m = MemoriaIa::abrir(&conn, &h).unwrap();
        assert_eq!(m.versao_esquema().unwrap(), 1);
        for t in ["ia_receitas", "ia_textos", "ia_licoes", "ia_normas", "ia_execucoes", "ia_cache"] {
            assert_eq!(conta(&conn, t), 0);
        }
    }

    #[test]
    fn ia_receitas_recusa_dado_sensivel() {
        let conn = Connection::open_in_memory().unwrap();
        let h = hig();
        let m = MemoriaIa::abrir(&conn, &h).unwrap();
        for s in sensiveis() {
            let md = receita_md("exemplo", "Exemplo", &s, "1. Ler.");
            let r = Receita::de_markdown(&md).unwrap();
            assert_recusado(m.gravar_receita(&r));
        }
        assert_eq!(conta(&conn, "ia_receitas"), 0);
        assert_eq!(conta(&conn, "ia_receitas_fts"), 0);
    }

    #[test]
    fn ia_receitas_confere_cada_campo_mesmo_se_texto_md_estiver_limpo() {
        // Revisão de segurança de 15/11/2026: um campo derivado com dado sensível não pode passar só porque o
        // `texto_md` (o único conferido antes) está limpo.
        let conn = Connection::open_in_memory().unwrap();
        let h = hig();
        let m = MemoriaIa::abrir(&conn, &h).unwrap();
        for s in sensiveis() {
            let mut r = Receita::de_markdown(&receita_md("exemplo", "Exemplo", "1. Ler.", "1. Ler.")).unwrap();
            r.privacidade = s.clone();
            assert_recusado(m.gravar_receita(&r));
            let mut r = Receita::de_markdown(&receita_md("exemplo", "Exemplo", "1. Ler.", "1. Ler.")).unwrap();
            r.saida = s.clone();
            assert_recusado(m.gravar_receita(&r));
        }
        assert_eq!(conta(&conn, "ia_receitas"), 0);
    }

    #[test]
    fn ia_textos_recusa_dado_sensivel() {
        let conn = Connection::open_in_memory().unwrap();
        let h = hig();
        let m = MemoriaIa::abrir(&conn, &h).unwrap();
        for s in sensiveis() {
            let t = TextoModelo {
                id: "texto-exemplo".into(),
                titulo: "Aviso".into(),
                canal: "email".into(),
                corpo: format!("Prezada direção, {s}."),
                normas: vec![],
            };
            assert_recusado(m.gravar_texto(&t));
        }
        let bom = TextoModelo {
            id: "texto-exemplo".into(),
            titulo: "Aviso de pendência".into(),
            canal: "email".into(),
            corpo: "Prezada direção, segue a tabela de [TURMA]: [TABELA POR TURMA].".into(),
            normas: vec!["norma-exemplo".into()],
        };
        m.gravar_texto(&bom).unwrap();
        assert_eq!(m.texto("texto-exemplo").unwrap(), Some(bom));
        assert_eq!(m.buscar_textos("tabela pendencia", 5).unwrap()[0].id, "texto-exemplo");
    }

    #[test]
    fn ia_licoes_recusa_dado_sensivel_e_busca() {
        let conn = Connection::open_in_memory().unwrap();
        let h = hig();
        let m = MemoriaIa::abrir(&conn, &h).unwrap();
        for s in sensiveis() {
            let l = Licao::nova(OrigemLicao::Agente, "portal", s.as_str(), "conferir", Grau::Palpite).unwrap();
            assert_recusado(m.gravar_licao(&l));
        }
        assert_eq!(conta(&conn, "ia_licoes"), 0);
        let l = Licao::nova(
            OrigemLicao::Professor,
            "presenca",
            "registro sumiu por instabilidade",
            "salvar o comprovante antes de sair da página",
            Grau::Certo,
        )
        .unwrap();
        let id = m.gravar_licao(&l).unwrap();
        m.gravar_licao(
            &Licao::nova(OrigemLicao::Claude, "outro", "a", "b", Grau::Provavel).unwrap(),
        )
        .unwrap();
        let achadas = m.buscar_licoes("instabilidade no registro", 5).unwrap();
        assert_eq!(achadas.len(), 1);
        assert_eq!(achadas[0].id, id);
        assert_eq!(achadas[0].licao, l);
    }

    #[test]
    fn ia_normas_recusa_dado_sensivel() {
        let conn = Connection::open_in_memory().unwrap();
        let h = hig();
        let m = MemoriaIa::abrir(&conn, &h).unwrap();
        for s in sensiveis() {
            let n = Norma {
                id: "norma-exemplo".into(),
                citacao: s.clone(),
                fonte_url: None,
                grau: Grau::Certo,
                verificada_em: "2026-11-15".into(),
            };
            assert_recusado(m.gravar_norma(&n));
        }
        let n = Norma {
            id: "norma-exemplo".into(),
            citacao: "Artigo inventado para teste".into(),
            fonte_url: Some("https://exemplo.invalid/norma".into()),
            grau: Grau::Provavel,
            verificada_em: "2026-11-15".into(),
        };
        m.gravar_norma(&n).unwrap();
        assert_eq!(m.norma("norma-exemplo").unwrap(), Some(n));
    }

    #[test]
    fn ia_execucoes_recusa_dado_sensivel_e_so_guarda_hash() {
        let conn = Connection::open_in_memory().unwrap();
        let h = hig();
        let m = MemoriaIa::abrir(&conn, &h).unwrap();
        let chave = [7u8; 32];
        let mut e = RegistroExecucao {
            receita_id: "exemplo".into(),
            receita_versao: 1,
            hash_entradas: hash_entradas(&chave, &["Joana Teste Silva", "2026-11-15"]),
            passos: 4,
            sim_dados: 1,
            sim_negados: 0,
            duracao_ms: 1234,
            resultado: ResultadoExecucao::Concluida,
            veredito: None,
            nota: None,
        };
        for s in sensiveis() {
            let mut ruim = e.clone();
            ruim.nota = Some(s.clone());
            assert_recusado(m.registrar_execucao(&ruim));
        }
        let mut sem_hash = e.clone();
        sem_hash.hash_entradas = "Joana Teste Silva".into();
        assert!(matches!(m.registrar_execucao(&sem_hash), Err(ErroIa::ValorInvalido(_))));
        assert_eq!(conta(&conn, "ia_execucoes"), 0);

        let id = m.registrar_execucao(&e).unwrap();
        assert_recusado(m.avaliar_execucao(id, Veredito::Erro, Some("faltou o Beto Inventado")));
        m.avaliar_execucao(id, Veredito::Parcial, Some("faltou um passo")).unwrap();
        e.veredito = Some(Veredito::Parcial);
        e.nota = Some("faltou um passo".into());
        assert_eq!(m.ultimas_execucoes(10).unwrap(), vec![(id, e)]);
        // O hash com chave muda com a chave (sem a chave não dá para conferir um palpite).
        assert_ne!(
            hash_entradas(&chave, &["Joana Teste Silva"]),
            hash_entradas(&[8u8; 32], &["Joana Teste Silva"])
        );
    }

    #[test]
    fn ia_cache_recusa_dado_sensivel_e_vence() {
        let conn = Connection::open_in_memory().unwrap();
        let h = hig();
        let m = MemoriaIa::abrir(&conn, &h).unwrap();
        let chave = chave_cache("modelo-x", "receita@1", "texto com [ALUNO_1]");
        for s in sensiveis() {
            assert_recusado(m.gravar_cache(&chave, &s, 1_000, 10_000));
        }
        assert_eq!(conta(&conn, "ia_cache"), 0);
        m.gravar_cache(&chave, "resposta com [ALUNO_1]", 1_000, 10_000).unwrap();
        assert_eq!(m.ler_cache(&chave, 5_000).unwrap().as_deref(), Some("resposta com [ALUNO_1]"));
        assert_eq!(m.ler_cache(&chave, 11_000).unwrap(), None);
        assert_eq!(m.limpar_cache_vencido(11_000).unwrap(), 1);
        assert_ne!(chave, chave_cache("modelo-y", "receita@1", "texto com [ALUNO_1]"));
    }

    #[test]
    fn busca_ignora_sintaxe_do_fts_e_nao_quebra() {
        let conn = Connection::open_in_memory().unwrap();
        let h = hig();
        let m = MemoriaIa::abrir(&conn, &h).unwrap();
        assert!(m.buscar_receitas("\" OR NEAR( * ", 5).unwrap().is_empty());
        assert!(m.buscar_receitas("", 5).unwrap().is_empty());
        assert!(m.buscar_receitas("aula", 0).is_err());
    }

    fn pasta_temporaria(nome: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("ia_nucleo_{nome}_{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn importa_pasta_de_receitas_sinteticas_e_busca_por_bm25() {
        let dir = pasta_temporaria("receitas");
        let arquivos = [
            (
                "professor-presente-diagnostico.md",
                receita_md(
                    "professor-presente-diagnostico",
                    "PresencaDiagnosticar",
                    "Quando a aula ficou branca ou amarela no painel de presença.",
                    "1. Abrir o painel.\n2. Ver se a frequência e o registro da aula foram lançados.",
                ),
            ),
            (
                "frequencia-lancar.md",
                receita_md(
                    "frequencia-lancar",
                    "FrequenciaLancar",
                    "Quando falta lançar a frequência de uma turma.",
                    "1. Abrir a turma.\n2. Marcar a chamada.",
                ),
            ),
            (
                "contracheque-conferir.md",
                receita_md(
                    "contracheque-conferir",
                    "ContrachequeConferir",
                    "Quando um mês do contracheque não aparece.",
                    "1. Listar as competências.",
                ),
            ),
            (
                "com-dado-sensivel.md",
                receita_md(
                    "com-dado-sensivel",
                    "Exemplo",
                    "Quando a aluna Joana Teste Silva pedir.",
                    "1. Ler.",
                ),
            ),
            ("fora-do-formato.md", "# Só um título\n\nsem cabeçalho".to_owned()),
            ("ignorado.txt", "não é .md".to_owned()),
        ];
        for (nome, conteudo) in &arquivos {
            std::fs::write(dir.join(nome), conteudo).unwrap();
        }

        let conn = Connection::open_in_memory().unwrap();
        let h = hig();
        let m = MemoriaIa::abrir(&conn, &h).unwrap();
        let rel = importar_pasta_receitas(&m, &dir).unwrap();
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(
            rel.importadas,
            vec!["contracheque-conferir", "frequencia-lancar", "professor-presente-diagnostico"]
        );
        let recusadas: Vec<&str> = rel.recusadas.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(recusadas, vec!["com-dado-sensivel.md", "fora-do-formato.md"]);
        assert!(rel.recusadas[0].1.contains("ALUNO"));
        assert!(!rel.recusadas[0].1.contains("Joana"));
        assert_eq!(m.quantidade_receitas().unwrap(), 3);

        let achados = m.buscar_receitas("minha aula ficou branca", 3).unwrap();
        assert_eq!(achados[0].id, "professor-presente-diagnostico");
        assert!(achados[0].pontuacao > 0.0);
        let achados = m.buscar_receitas("lançar frequencia", 3).unwrap();
        assert_eq!(achados[0].id, "frequencia-lancar");

        // Ida e volta pelo banco e reimportação (upsert sem duplicar no FTS).
        let r = m.receita("frequencia-lancar").unwrap().unwrap();
        assert_eq!(r.intencao, "FrequenciaLancar");
        m.gravar_receita(&r).unwrap();
        assert_eq!(conta(&conn, "ia_receitas_fts"), 3);

        // Versão menor que a gravada é recusada.
        let mut r2 = Receita::de_markdown(&receita_md("frequencia-lancar", "FrequenciaLancar", "q", "p")
            .replace("versao: 1", "versao: 3"))
        .unwrap();
        m.gravar_receita(&r2).unwrap();
        r2.versao = 2;
        assert!(matches!(m.gravar_receita(&r2), Err(ErroIa::ValorInvalido(_))));
    }
}
