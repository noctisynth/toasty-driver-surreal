//! `UPDATE <record id> ...` translation for
//! [`Operation::UpdateByKey`](toasty_core::driver::operation::Operation).

use surrealdb::types::Value as SurValue;
use toasty_core::driver::{ExecResponse, operation};
use toasty_core::schema::db;
use toasty_core::stmt::{self, ExprContext};

use crate::conn::{Connection, run_query, take_rows};
use crate::expr::{self, Binds};
use crate::op::row_to_record;
use crate::record_id::record_id;
use crate::value::to_surreal;

impl Connection {
    pub(crate) async fn exec_update_by_key(
        &mut self,
        schema: &db::Schema,
        op: operation::UpdateByKey,
    ) -> toasty_core::Result<ExecResponse> {
        let table = schema.table(op.table);

        // The engine shreds multi-key updates into one op per key.
        let [key] = &op.keys[..] else {
            return Err(toasty_core::Error::invalid_statement(format!(
                "SurrealDB update expects exactly one key, got {}",
                op.keys.len()
            )));
        };

        let cx = ExprContext::new_with_target(schema, table);
        let mut binds = Binds::default();

        // Render SET assignments.
        let mut sets: Vec<String> = Vec::new();
        for (projection, assignment) in op.assignments.iter() {
            let column = table.resolve(projection);
            let name = expr::escape_ident(&column.name);
            match assignment {
                stmt::Assignment::Set(stmt::Expr::Value(value)) if value.is_null() => {
                    sets.push(format!("{name} = NONE"));
                }
                stmt::Assignment::Set(stmt::Expr::Value(value)) => {
                    let placeholder = binds.push(to_surreal(value)?);
                    sets.push(format!("{name} = {placeholder}"));
                }
                stmt::Assignment::Add(stmt::Expr::Value(value)) => {
                    let placeholder = binds.push(to_surreal(value)?);
                    sets.push(format!("{name} = {name} + {placeholder}"));
                }
                stmt::Assignment::Subtract(stmt::Expr::Value(value)) => {
                    let placeholder = binds.push(to_surreal(value)?);
                    sets.push(format!("{name} = {name} - {placeholder}"));
                }
                // `push` / `extend` on a `Vec<scalar>` field append a list.
                // SurrealQL's `+=` concatenates an array onto the field.
                stmt::Assignment::Append(stmt::Expr::Value(value)) => {
                    let list = match value {
                        stmt::Value::List(_) => to_surreal(value)?,
                        // A single pushed element still arrives as a scalar;
                        // wrap it so `+=` appends one item rather than erroring.
                        other => to_surreal(&stmt::Value::List(vec![other.clone()]))?,
                    };
                    let placeholder = binds.push(list);
                    sets.push(format!("{name} += {placeholder}"));
                }
                other => {
                    return Err(toasty_core::Error::unsupported_feature(format!(
                        "SurrealDB driver does not support update assignment: {other:?}"
                    )));
                }
            }
        }

        // A filter and/or condition restrict which record is updated.
        let mut predicates: Vec<String> = Vec::new();
        if let Some(filter) = &op.filter {
            predicates.push(expr::render(&cx, table, &mut binds, filter)?);
        }
        if let Some(condition) = &op.condition {
            predicates.push(expr::render(&cx, table, &mut binds, condition)?);
        }

        let rid = record_id(table, key)?;
        binds.push_named("rid", SurValue::RecordId(rid));

        let set_clause = if sets.is_empty() {
            String::new()
        } else {
            format!(" SET {}", sets.join(", "))
        };
        let where_clause = if predicates.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", predicates.join(" AND "))
        };
        let returning = op.returning.is_some();
        let return_clause = if returning { " RETURN AFTER" } else { "" };

        let sql = format!("UPDATE $rid{set_clause}{where_clause}{return_clause}");
        let mut response = run_query(&self.db, sql, binds.into_vec()).await?;
        let rows = take_rows(&mut response, 0)?;

        // A condition that fails matches no row; treat as a condition failure
        // so the engine can surface it, matching the DynamoDB contract.
        if rows.is_empty() && op.condition.is_some() {
            return Err(toasty_core::Error::condition_failed(
                "SurrealDB update condition did not match",
            ));
        }

        match &op.returning {
            Some(columns) => {
                let select: Vec<&db::Column> =
                    columns.iter().map(|id| schema.column(*id)).collect();
                let mut records = Vec::with_capacity(rows.len());
                for row in rows {
                    records.push(stmt::Value::from(row_to_record(
                        &row,
                        select.iter().copied(),
                    )?));
                }
                Ok(ExecResponse::value_stream(stmt::ValueStream::from_vec(
                    records,
                )))
            }
            None => Ok(ExecResponse::count(rows.len() as u64)),
        }
    }
}
