//! Mapping between Toasty primary keys and SurrealDB record ids.
//!
//! Toasty identifies rows by primary-key columns; SurrealDB identifies rows by
//! a `table:key` record id. This module converts between the two. Single-column
//! keys map to a scalar [`RecordIdKey`]; composite keys map to an array key,
//! with elements ordered to match `Table::primary_key_columns()`.

use surrealdb::types::{Array, RecordId, RecordIdKey, Value as SurValue};
use toasty_core::schema::db::Table;
use toasty_core::stmt;

use crate::value::to_surreal;

/// Builds a SurrealDB [`RecordId`] from a table and a Toasty key value.
///
/// The engine represents keys inconsistently across operations: an insert row
/// yields a bare scalar for a single-column key, while `GetByKey` wraps even a
/// single-column key in a [`stmt::Value::Record`]. The record id shape must
/// therefore be driven by the table's primary-key arity, not by whether the
/// value happens to be a record: a single-column key becomes a scalar
/// [`RecordIdKey`], and a multi-column key becomes an array key ordered to
/// match `Table::primary_key.columns`.
pub(crate) fn record_id(table: &Table, key: &stmt::Value) -> toasty_core::Result<RecordId> {
    let arity = table.primary_key.columns.len();
    let key = record_id_key(key, arity)?;
    Ok(RecordId::new(table.name.as_str(), key))
}

/// Converts a Toasty key value into a SurrealDB [`RecordIdKey`], using the
/// primary-key `arity` to decide between a scalar and an array key.
fn record_id_key(key: &stmt::Value, arity: usize) -> toasty_core::Result<RecordIdKey> {
    match key {
        // A record holds the key columns. With a single-column primary key the
        // engine still wraps the scalar in a one-element record on some paths,
        // so unwrap it; with a composite key, build an ordered array.
        stmt::Value::Record(fields) => {
            if arity <= 1 {
                let field = fields.iter().next().ok_or_else(|| {
                    toasty_core::Error::invalid_statement(
                        "empty record key for single-column table",
                    )
                })?;
                scalar_to_key(field)
            } else {
                let mut elems = Vec::with_capacity(fields.len());
                for field in fields.iter() {
                    elems.push(scalar_key_value(field)?);
                }
                Ok(RecordIdKey::Array(Array::from(elems)))
            }
        }
        other => scalar_to_key(other),
    }
}

/// Converts a single scalar Toasty value into a scalar [`RecordIdKey`].
fn scalar_to_key(value: &stmt::Value) -> toasty_core::Result<RecordIdKey> {
    Ok(match value {
        stmt::Value::I8(v) => RecordIdKey::Number(*v as i64),
        stmt::Value::I16(v) => RecordIdKey::Number(*v as i64),
        stmt::Value::I32(v) => RecordIdKey::Number(*v as i64),
        stmt::Value::I64(v) => RecordIdKey::Number(*v),
        stmt::Value::U8(v) => RecordIdKey::Number(*v as i64),
        stmt::Value::U16(v) => RecordIdKey::Number(*v as i64),
        stmt::Value::U32(v) => RecordIdKey::Number(*v as i64),
        stmt::Value::U64(v) => RecordIdKey::Number(i64::try_from(*v).map_err(|_| {
            toasty_core::Error::unsupported_feature(
                "SurrealDB record id integers are i64; u64 keys above i64::MAX are not supported",
            )
        })?),
        stmt::Value::String(v) => RecordIdKey::String(v.clone()),
        stmt::Value::Uuid(v) => RecordIdKey::Uuid((*v).into()),
        other => {
            return Err(toasty_core::Error::unsupported_feature(format!(
                "SurrealDB record id key cannot be built from value: {other:?}"
            )));
        }
    })
}

/// Converts a composite-key element into a SurrealDB [`Value`] for an array key.
fn scalar_key_value(value: &stmt::Value) -> toasty_core::Result<SurValue> {
    // Array record-id elements are ordinary values, so reuse the value codec.
    to_surreal(value)
}
