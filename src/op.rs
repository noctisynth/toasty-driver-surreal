//! Per-[`Operation`] translation into SurrealQL.
//!
//! Each submodule handles one key-value operation. Shared helpers for building
//! column projections, decoding result rows into Toasty records, and rendering
//! record ids live here.

mod delete_by_key;
mod find_pk_by_index;
mod get_by_key;
mod insert;
pub(crate) mod query_pk;
mod scan;
mod update_by_key;
mod upsert;

use surrealdb::types::{Object, Value as SurValue};
use toasty_core::schema::db::{Column, Table};
use toasty_core::stmt;

use crate::expr;
use crate::value::from_surreal_for_column;

/// Renders a projection over the given columns, aliasing primary-key columns
/// out of the record id (`record::id(id) AS <name>`), so the decoded row
/// exposes the key as a normal field.
fn project_columns<'a>(table: &Table, columns: impl Iterator<Item = &'a Column>) -> String {
    let parts: Vec<String> = columns
        .map(|column| {
            let alias = expr::escape_ident(&column.name);
            if expr::is_primary_key(table, column) {
                let reference = expr::column_ref(table, column);
                format!("{reference} AS {alias}")
            } else {
                alias
            }
        })
        .collect();

    if parts.is_empty() {
        "*".to_string()
    } else {
        parts.join(", ")
    }
}

/// Decodes one SurrealDB row object into a Toasty record with the columns in
/// `select` order. Missing fields decode to `Null`.
fn row_to_record<'a>(
    row: &SurValue,
    select: impl Iterator<Item = &'a Column>,
) -> toasty_core::Result<stmt::ValueRecord> {
    let object = match row {
        SurValue::Object(obj) => Some(obj),
        _ => None,
    };

    let mut fields = Vec::new();
    for column in select {
        let value = object
            .and_then(|obj: &Object| obj.get(column.name.as_str()).cloned())
            .unwrap_or(SurValue::None);
        fields.push(from_surreal_for_column(value, column)?);
    }

    Ok(stmt::ValueRecord::from_vec(fields))
}
