//! Pequenos ajudantes de SQL usados pelos repositórios.

use rusqlite::{types::Type, Connection, OptionalExtension, Params, Row};
use serde_json::Value;

use crate::error::Result;

/// JSON em texto para as colunas `metadata_bruta`, `payload` e `evidencias` (todas com `json_valid`).
pub(crate) fn json_texto(valor: &Value) -> Result<String> {
    Ok(serde_json::to_string(valor)?)
}

/// Lê uma coluna de texto JSON.
pub(crate) fn json_da_coluna(row: &Row<'_>, idx: usize) -> rusqlite::Result<Value> {
    let texto: String = row.get(idx)?;
    serde_json::from_str(&texto)
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(idx, Type::Text, Box::new(e)))
}

/// Lê uma coluna de texto que guarda um enum do ordoc-core (ex.: `PENDING_SYNC`).
pub(crate) fn enum_da_coluna<T>(
    row: &Row<'_>,
    idx: usize,
    parse: fn(&str) -> ordoc_core::Result<T>,
) -> rusqlite::Result<T> {
    let texto: String = row.get(idx)?;
    parse(&texto)
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(idx, Type::Text, Box::new(e)))
}

pub(crate) fn listar<T, P: Params>(
    conn: &Connection,
    sql: &str,
    parametros: P,
    mapear: fn(&Row<'_>) -> rusqlite::Result<T>,
) -> Result<Vec<T>> {
    let mut stmt = conn.prepare_cached(sql)?;
    let linhas = stmt
        .query_map(parametros, mapear)?
        .collect::<rusqlite::Result<Vec<T>>>()?;
    Ok(linhas)
}

pub(crate) fn obter<T, P: Params>(
    conn: &Connection,
    sql: &str,
    parametros: P,
    mapear: fn(&Row<'_>) -> rusqlite::Result<T>,
) -> Result<Option<T>> {
    let linha = conn
        .prepare_cached(sql)?
        .query_row(parametros, mapear)
        .optional()?;
    Ok(linha)
}
