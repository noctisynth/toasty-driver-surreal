//! Value codec between Toasty's `stmt::Value`/`stmt::Type` and SurrealDB's
//! native `surrealdb::types::Value`.
//!
//! Encoding and decoding go through the native `Value` (never JSON) so that
//! bytes, UUIDs, and other typed values keep their SurrealDB representation.
//! See `.agents/spikes/surrealdb-sdk-3.2.4.md` for why the JSON path is lossy.

use surrealdb::types::{Array, Number, Object, Value as SurValue};
use toasty_core::stmt::{self, Type};

/// Encodes a Toasty value as a native SurrealDB value for binding.
///
/// The `U64` case fails with [`toasty_core::Error::unsupported_feature`] when
/// the value exceeds `i64::MAX`, because SurrealDB integers are `i64`.
pub(crate) fn to_surreal(value: &stmt::Value) -> toasty_core::Result<SurValue> {
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

/// Decodes a native SurrealDB value into a Toasty value, directed by the
/// expected column [`Type`].
///
/// `None`/`Null` both decode to [`stmt::Value::Null`]. Numeric narrowing
/// follows the target integer/float type, mirroring the SQLite driver's
/// `from_sql`.
pub(crate) fn from_surreal(value: SurValue, ty: &Type) -> toasty_core::Result<stmt::Value> {
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
