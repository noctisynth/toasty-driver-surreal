//! Value codec between Toasty's `stmt::Value`/`stmt::Type` and SurrealDB's
//! native `surrealdb::types::Value`.
//!
//! General encoding and decoding go directly through native `Value` so that
//! bytes, UUIDs, and other typed values keep their SurrealDB representation.
//! The one deliberate JSON-text boundary is `db::Type::Json`: Toasty defines
//! its driver wire format as serialized JSON text, which this module validates
//! and converts to/from JSON-compatible native SurrealDB values.

use serde_json::Value as JsonValue;
use surrealdb::types::{Array, Number, Object, Value as SurValue};
use toasty_core::schema::db::{self, Column};
use toasty_core::stmt::{self, Type};

/// Encodes a Toasty value as a native SurrealDB value for binding.
///
/// A `U64` above `i64::MAX` uses SurrealDB Decimal storage so its magnitude is
/// preserved.
pub(crate) fn to_surreal(value: &stmt::Value) -> toasty_core::Result<SurValue> {
    to_surreal_with_storage(value, None)
}

/// Encodes a Toasty value using the target column's database storage type.
///
/// Toasty's native JSON fields cross the driver boundary as serialized
/// strings (`stmt::Type::String`) and are distinguishable from ordinary text
/// only through `Column::storage_ty`. Every column write must therefore use
/// this entry point rather than the untyped [`to_surreal`] helper.
pub(crate) fn to_surreal_for_column(
    value: &stmt::Value,
    column: &Column,
) -> toasty_core::Result<SurValue> {
    to_surreal_with_storage(value, Some(&column.storage_ty))
}

/// Encodes a Toasty value using an explicitly inferred storage type.
///
/// Expression rendering uses this when a literal is compared with a resolved
/// column but no `Column` object is otherwise carried by the expression node.
pub(crate) fn to_surreal_for_storage(
    value: &stmt::Value,
    storage_ty: &db::Type,
) -> toasty_core::Result<SurValue> {
    to_surreal_with_storage(value, Some(storage_ty))
}

fn to_surreal_with_storage(
    value: &stmt::Value,
    storage_ty: Option<&db::Type>,
) -> toasty_core::Result<SurValue> {
    if matches!(storage_ty, Some(db::Type::Json)) {
        return json_text_to_surreal(value);
    }

    Ok(match value {
        stmt::Value::Null => SurValue::Null,
        stmt::Value::Bool(v) => SurValue::Bool(*v),
        stmt::Value::I8(v) => SurValue::Number(Number::Int(*v as i64)),
        stmt::Value::I16(v) => SurValue::Number(Number::Int(*v as i64)),
        stmt::Value::I32(v) => SurValue::Number(Number::Int(*v as i64)),
        stmt::Value::I64(v) => SurValue::Number(Number::Int(*v)),
        stmt::Value::U8(v) => SurValue::Number(Number::Int(*v as i64)),
        stmt::Value::U16(v) => SurValue::Number(Number::Int(*v as i64)),
        stmt::Value::U32(v) => SurValue::Number(Number::Int(*v as i64)),
        stmt::Value::U64(v) => match i64::try_from(*v) {
            Ok(n) => SurValue::Number(Number::Int(n)),
            // Values above i64::MAX use a Decimal so no magnitude is lost;
            // the U64 decode path recovers them.
            Err(_) => SurValue::Number(Number::Decimal(rust_decimal::Decimal::from(*v))),
        },
        stmt::Value::F32(v) => SurValue::Number(Number::Float(*v as f64)),
        stmt::Value::F64(v) => SurValue::Number(Number::Float(*v)),
        stmt::Value::String(v) => SurValue::String(v.clone()),
        stmt::Value::Bytes(v) => SurValue::Bytes(v.clone().into()),
        stmt::Value::Uuid(v) => SurValue::Uuid((*v).into()),
        stmt::Value::List(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(to_surreal(item)?);
            }
            SurValue::Array(Array::from(out))
        }
        stmt::Value::Record(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items.iter() {
                out.push(to_surreal(item)?);
            }
            SurValue::Array(Array::from(out))
        }
        stmt::Value::Object(obj) => {
            let mut out = Object::new();
            for (name, val) in obj.iter() {
                // Omit null fields so `field IS NONE` presence checks work: an
                // absent key reads back as NONE, whereas an explicit NULL would
                // not. The engine fills omitted keys with Null when raising the
                // embed. Matches the DynamoDB driver's document encoding.
                if !val.is_null() {
                    out.insert(name.clone(), to_surreal(val)?);
                }
            }
            SurValue::Object(out)
        }
        // Temporal, decimal, and network values are stored as their canonical
        // text form (SurrealDB has no matching native column type in this
        // driver's mapping); the engine casts them back on read. This mirrors
        // the DynamoDB driver's string encoding for the same value families.
        other => match other.document_storage_text() {
            Some(text) => SurValue::String(text.to_string()),
            None => {
                return Err(toasty_core::Error::unsupported_feature(format!(
                    "SurrealDB driver cannot encode value variant: {other:?}"
                )));
            }
        },
    })
}

/// Parses Toasty's serialized native-JSON wire value into a SurrealDB native
/// value. A Toasty null is the database-level empty value (`NONE`); the JSON
/// text `"null"` is the JSON literal (`NULL`).
fn json_text_to_surreal(value: &stmt::Value) -> toasty_core::Result<SurValue> {
    match value {
        stmt::Value::Null => Ok(SurValue::None),
        stmt::Value::String(text) => {
            let json: JsonValue = serde_json::from_str(text).map_err(|error| {
                toasty_core::Error::serialization_failure(format!(
                    "invalid JSON for a SurrealDB native JSON column at line {}, column {}",
                    error.line(),
                    error.column()
                ))
            })?;
            json_value_to_surreal(json)
        }
        _ => Err(toasty_core::Error::serialization_failure(
            "SurrealDB native JSON columns require serialized JSON text or a database null",
        )),
    }
}

/// Converts a validated JSON tree without applying the `#[document]` codec's
/// null-field omission rule.
fn json_value_to_surreal(value: JsonValue) -> toasty_core::Result<SurValue> {
    Ok(match value {
        JsonValue::Null => SurValue::Null,
        JsonValue::Bool(value) => SurValue::Bool(value),
        JsonValue::Number(value) => {
            if let Some(value) = value.as_i64() {
                SurValue::Number(Number::Int(value))
            } else if let Some(value) = value.as_u64() {
                // SurrealDB integers are i64. Preserve larger JSON u64 values
                // as Decimal rather than rounding them through f64.
                SurValue::Number(Number::Decimal(rust_decimal::Decimal::from(value)))
            } else if let Some(value) = value.as_f64() {
                SurValue::Number(Number::Float(value))
            } else {
                return Err(toasty_core::Error::serialization_failure(
                    "JSON number cannot be represented by SurrealDB",
                ));
            }
        }
        JsonValue::String(value) => SurValue::String(value),
        JsonValue::Array(values) => {
            let mut out = Vec::with_capacity(values.len());
            for value in values {
                out.push(json_value_to_surreal(value)?);
            }
            SurValue::Array(Array::from(out))
        }
        JsonValue::Object(values) => {
            let mut out = Object::new();
            for (name, value) in values {
                out.insert(name, json_value_to_surreal(value)?);
            }
            SurValue::Object(out)
        }
    })
}

/// Decodes a native SurrealDB value into a Toasty value, directed by the
/// expected column [`Type`].
///
/// `None`/`Null` both decode to [`stmt::Value::Null`]. Numeric narrowing
/// follows the target integer/float type, mirroring the SQLite driver's
/// `from_sql`.
pub(crate) fn from_surreal(value: SurValue, ty: &Type) -> toasty_core::Result<stmt::Value> {
    from_surreal_with_storage(value, ty, None)
}

/// Decodes a value using the concrete column metadata.
pub(crate) fn from_surreal_for_column(
    value: SurValue,
    column: &Column,
) -> toasty_core::Result<stmt::Value> {
    from_surreal_with_storage(value, &column.ty, Some(&column.storage_ty))
}

fn from_surreal_with_storage(
    value: SurValue,
    ty: &Type,
    storage_ty: Option<&db::Type>,
) -> toasty_core::Result<stmt::Value> {
    if matches!(storage_ty, Some(db::Type::Json)) {
        return surreal_json_to_text(value);
    }

    match value {
        SurValue::None | SurValue::Null => Ok(stmt::Value::Null),
        SurValue::Bool(b) => Ok(stmt::Value::Bool(b)),
        SurValue::Number(n) => number_to_stmt(n, ty),
        SurValue::String(s) => match ty {
            // Plain strings, and document leaves (decoded as `Unknown`), stay
            // strings; the engine casts document leaves when it raises the embed.
            Type::String | Type::Unknown => Ok(stmt::Value::String(s)),
            Type::Uuid => s.parse().map(stmt::Value::Uuid).map_err(|_| {
                toasty_core::Error::unsupported_feature("expected UUID text from SurrealDB")
            }),
            // Temporal / decimal / network columns are stored as text; recover
            // the typed value through the same `Type::cast` the engine uses.
            _ => ty.cast(&(), stmt::Value::String(s)).map_err(|_| {
                toasty_core::Error::unsupported_feature(format!(
                    "SurrealDB text value does not convert to column type {ty:?}"
                ))
            }),
        },
        SurValue::Uuid(u) => Ok(stmt::Value::Uuid(u.into_inner())),
        SurValue::Bytes(b) => Ok(stmt::Value::Bytes(b.into_inner().to_vec())),
        SurValue::RecordId(rid) => record_id_key_to_stmt(rid.key, ty),
        SurValue::Array(arr) => {
            let elem_ty = match ty {
                Type::List(inner) => inner.as_ref(),
                _ => &Type::Unknown,
            };
            let mut out = Vec::with_capacity(arr.len());
            for item in arr.into_inner() {
                out.push(from_surreal(item, elem_ty)?);
            }
            Ok(stmt::Value::List(out))
        }
        SurValue::Object(obj) => {
            let mut entries = Vec::new();
            for (name, val) in obj.into_iter() {
                entries.push((name, from_surreal(val, &Type::Unknown)?));
            }
            Ok(stmt::Value::Object(stmt::ValueObject::from_vec(entries)))
        }
        other => Err(toasty_core::Error::unsupported_feature(format!(
            "SurrealDB driver cannot decode value: {other:?}"
        ))),
    }
}

/// Converts a SurrealDB native JSON value back to Toasty's serialized JSON
/// wire representation. Only `NONE` maps to Toasty null; `NULL` remains the
/// non-null JSON text `"null"`.
fn surreal_json_to_text(value: SurValue) -> toasty_core::Result<stmt::Value> {
    if matches!(value, SurValue::None) {
        return Ok(stmt::Value::Null);
    }

    let json = surreal_to_json_value(value)?;
    let text = serde_json::to_string(&json).map_err(|error| {
        toasty_core::Error::serialization_failure(format!(
            "failed to serialize a SurrealDB native JSON value: {error}"
        ))
    })?;
    Ok(stmt::Value::String(text))
}

/// Strictly converts only JSON-compatible SurrealDB variants. This avoids the
/// SDK's best-effort JSON conversion, which would stringify UUIDs, record ids,
/// and other values that are invalid for this column contract.
fn surreal_to_json_value(value: SurValue) -> toasty_core::Result<JsonValue> {
    Ok(match value {
        SurValue::Null => JsonValue::Null,
        SurValue::Bool(value) => JsonValue::Bool(value),
        SurValue::Number(Number::Int(value)) => JsonValue::Number(value.into()),
        SurValue::Number(Number::Float(value)) => {
            JsonValue::Number(serde_json::Number::from_f64(value).ok_or_else(|| {
                toasty_core::Error::serialization_failure(
                    "SurrealDB native JSON contained a non-finite number",
                )
            })?)
        }
        SurValue::Number(Number::Decimal(value)) => {
            let json: JsonValue = serde_json::from_str(&value.to_string()).map_err(|error| {
                toasty_core::Error::serialization_failure(format!(
                    "SurrealDB native JSON decimal could not be serialized: {error}"
                ))
            })?;
            if !json.is_number() {
                return Err(toasty_core::Error::serialization_failure(
                    "SurrealDB native JSON decimal did not produce a JSON number",
                ));
            }
            json
        }
        SurValue::String(value) => JsonValue::String(value),
        SurValue::Array(values) => {
            let values = values.into_inner();
            let mut out = Vec::with_capacity(values.len());
            for value in values {
                out.push(surreal_to_json_value(value)?);
            }
            JsonValue::Array(out)
        }
        SurValue::Object(values) => {
            let mut out = serde_json::Map::new();
            for (name, value) in values.into_iter() {
                out.insert(name, surreal_to_json_value(value)?);
            }
            JsonValue::Object(out)
        }
        SurValue::None => {
            return Err(toasty_core::Error::serialization_failure(
                "SurrealDB native JSON contained an embedded NONE value",
            ));
        }
        _ => {
            return Err(toasty_core::Error::serialization_failure(
                "SurrealDB native JSON column contained a non-JSON value",
            ));
        }
    })
}

/// Narrows a SurrealDB [`Number`] to the target integer/float Toasty value.
fn number_to_stmt(n: Number, ty: &Type) -> toasty_core::Result<stmt::Value> {
    let unexpected = || {
        toasty_core::Error::unsupported_feature(format!(
            "SurrealDB returned a number for a non-numeric column type: {ty:?}"
        ))
    };

    match ty {
        Type::Bool => Ok(stmt::Value::Bool(n.to_int().unwrap_or(0) != 0)),
        Type::I8 => Ok(stmt::Value::I8(n.to_int().unwrap_or(0) as i8)),
        Type::I16 => Ok(stmt::Value::I16(n.to_int().unwrap_or(0) as i16)),
        Type::I32 => Ok(stmt::Value::I32(n.to_int().unwrap_or(0) as i32)),
        Type::I64 => Ok(stmt::Value::I64(n.to_int().unwrap_or(0))),
        Type::U8 => Ok(stmt::Value::U8(n.to_int().unwrap_or(0) as u8)),
        Type::U16 => Ok(stmt::Value::U16(n.to_int().unwrap_or(0) as u16)),
        Type::U32 => Ok(stmt::Value::U32(n.to_int().unwrap_or(0) as u32)),
        // A u64 above i64::MAX is stored as a Decimal, so `to_int` (i64) would
        // lose it; recover it from the decimal form first.
        Type::U64 => Ok(stmt::Value::U64(number_to_u64(&n))),
        Type::F32 => Ok(stmt::Value::F32(n.to_f64().unwrap_or(0.0) as f32)),
        Type::F64 => Ok(stmt::Value::F64(n.to_f64().unwrap_or(0.0))),
        // A number may arrive for an untyped projection (e.g. count()); fall
        // back to i64 / f64 rather than failing.
        Type::Unknown => match n.to_int() {
            Some(i) => Ok(stmt::Value::I64(i)),
            None => Ok(stmt::Value::F64(n.to_f64().unwrap_or(0.0))),
        },
        _ => Err(unexpected()),
    }
}

/// Recovers a `u64` from a SurrealDB [`Number`], covering the Decimal form used
/// for values above `i64::MAX`.
fn number_to_u64(n: &Number) -> u64 {
    use rust_decimal::prelude::ToPrimitive;
    match n {
        Number::Int(v) => *v as u64,
        Number::Float(v) => *v as u64,
        Number::Decimal(v) => v.to_u64().unwrap_or(0),
    }
}

/// Decodes a record id key back into a Toasty scalar (or record for composite
/// keys), directed by the primary-key column type.
fn record_id_key_to_stmt(
    key: surrealdb::types::RecordIdKey,
    ty: &Type,
) -> toasty_core::Result<stmt::Value> {
    use surrealdb::types::RecordIdKey;

    match key {
        RecordIdKey::Number(i) => number_to_stmt(Number::Int(i), ty),
        RecordIdKey::String(s) => match ty {
            Type::Uuid => s.parse().map(stmt::Value::Uuid).map_err(|_| {
                toasty_core::Error::unsupported_feature("expected UUID record id key")
            }),
            _ => Ok(stmt::Value::String(s)),
        },
        RecordIdKey::Uuid(u) => Ok(stmt::Value::Uuid(u.into_inner())),
        RecordIdKey::Array(arr) => {
            // Composite key: decode each element as Unknown; the engine
            // re-types record fields from the key column list.
            let mut out = Vec::with_capacity(arr.len());
            for item in arr.into_inner() {
                out.push(from_surreal(item, &Type::Unknown)?);
            }
            Ok(stmt::Value::record_from_vec(out))
        }
        other => Err(toasty_core::Error::unsupported_feature(format!(
            "SurrealDB driver cannot decode record id key: {other:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_json_distinguishes_database_null_from_json_null() {
        let storage_ty = db::Type::Json;

        assert_eq!(
            to_surreal_with_storage(&stmt::Value::Null, Some(&storage_ty)).unwrap(),
            SurValue::None
        );
        assert_eq!(
            to_surreal_with_storage(&stmt::Value::String("null".to_string()), Some(&storage_ty))
                .unwrap(),
            SurValue::Null
        );
        assert_eq!(
            from_surreal_with_storage(SurValue::None, &Type::String, Some(&storage_ty)).unwrap(),
            stmt::Value::Null
        );
        assert_eq!(
            from_surreal_with_storage(SurValue::Null, &Type::String, Some(&storage_ty)).unwrap(),
            stmt::Value::String("null".to_string())
        );
    }

    #[test]
    fn native_json_preserves_null_object_members() {
        let storage_ty = db::Type::Json;
        let input = stmt::Value::String(r#"{"present":null,"items":[1,true]}"#.to_string());
        let encoded = to_surreal_with_storage(&input, Some(&storage_ty)).unwrap();

        let SurValue::Object(object) = &encoded else {
            panic!("native JSON object must encode as a SurrealDB object");
        };
        assert_eq!(object.get("present"), Some(&SurValue::Null));

        let stmt::Value::String(decoded) =
            from_surreal_with_storage(encoded, &Type::String, Some(&storage_ty)).unwrap()
        else {
            panic!("native JSON object must decode as serialized JSON text");
        };
        let stmt::Value::String(input) = input else {
            unreachable!("test input is JSON text");
        };
        assert_eq!(
            serde_json::from_str::<JsonValue>(&decoded).unwrap(),
            serde_json::from_str::<JsonValue>(&input).unwrap()
        );
    }

    #[test]
    fn native_json_rejects_invalid_text_without_echoing_it() {
        let storage_ty = db::Type::Json;
        let invalid = "{secret-payload";
        let error =
            to_surreal_with_storage(&stmt::Value::String(invalid.to_string()), Some(&storage_ty))
                .unwrap_err();

        assert!(error.is_serialization_failure());
        assert!(!error.to_string().contains(invalid));
    }

    #[test]
    fn native_json_round_trips_top_level_scalars_and_large_unsigned_numbers() {
        let storage_ty = db::Type::Json;

        for input in ["true", "42", r#""text""#, "18446744073709551615"] {
            let encoded =
                to_surreal_with_storage(&stmt::Value::String(input.to_string()), Some(&storage_ty))
                    .unwrap();
            let stmt::Value::String(decoded) =
                from_surreal_with_storage(encoded, &Type::String, Some(&storage_ty)).unwrap()
            else {
                panic!("native JSON scalar must decode as serialized JSON text");
            };

            assert_eq!(
                serde_json::from_str::<JsonValue>(&decoded).unwrap(),
                serde_json::from_str::<JsonValue>(input).unwrap()
            );
        }
    }
}
