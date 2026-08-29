//! `CREATE` translation for [`Operation::Insert`](toasty_core::driver::operation::Operation).

use surrealdb::types::{Object, Value as SurValue};
use toasty_core::driver::ExecResponse;
use toasty_core::schema::db;
use toasty_core::stmt;

use crate::conn::{Connection, take_rows};
use crate::record_id::record_id;
use crate::value::to_surreal;

impl Connection {
    pub(crate) async fn exec_insert(
        &mut self,
        schema: &db::Schema,
        insert: stmt::Insert,
        ret: Option<Vec<stmt::Type>>,
    ) -> toasty_core::Result<ExecResponse> {
        let target = insert.target.as_table_unwrap();
        let table = schema.table(target.table);

        let stmt::ExprSet::Values(values) = &insert.source.body else {
            return Err(toasty_core::Error::invalid_statement(
                "SurrealDB insert requires a VALUES source",
            ));
        };

        // The engine expresses the returned columns as a `RETURNING` record of
        // column references (populated from the requested select items).
        let returning = returning_columns(schema, table, insert.returning.as_ref())?;
        let want_return = returning.is_some() || ret.is_some();

        let mut count: u64 = 0;
        let mut records: Vec<stmt::Value> = Vec::new();

        // The engine may batch multiple rows; issue one CREATE per row so each
        // record id is set explicitly. SurrealDB has no multi-id CREATE.
        for row in &values.rows {
            let (rid_value, content) = build_row(schema, table, target, row)?;

            let sql = if want_return {
                "CREATE $rid CONTENT $data RETURN AFTER".to_string()
            } else {
                "CREATE $rid CONTENT $data".to_string()
            };

            let binds = vec![
                ("rid".to_string(), rid_value),
                ("data".to_string(), SurValue::Object(content)),
            ];

            let mut response = self.run_query(sql, binds).await?;
            let rows = take_rows(&mut response, 0)?;

            if let Some(columns) = &returning {
                for row in rows {
                    records.push(stmt::Value::from(crate::op::row_to_record(
                        &row,
                        columns.iter().copied(),
                    )?));
                }
            }
            count += 1;
        }

        if returning.is_some() {
            Ok(ExecResponse::value_stream(stmt::ValueStream::from_vec(
                records,
            )))
        } else {
            Ok(ExecResponse::count(count))
        }
    }
}

/// Resolves the insert's `RETURNING` projection into the columns to decode.
fn returning_columns<'a>(
    schema: &'a db::Schema,
    table: &'a db::Table,
    returning: Option<&stmt::Returning>,
) -> toasty_core::Result<Option<Vec<&'a db::Column>>> {
    let Some(returning) = returning else {
        return Ok(None);
    };
    let expr = returning.as_project().ok_or_else(|| {
        toasty_core::Error::invalid_statement("insert returning is not a projection")
    })?;
    let record = expr.as_record().ok_or_else(|| {
        toasty_core::Error::invalid_statement("insert returning must be a record")
    })?;
    let mut columns = Vec::with_capacity(record.fields.len());
    for field in &record.fields {
        let stmt::Expr::Reference(reference) = field else {
            return Err(toasty_core::Error::invalid_statement(
                "insert returning item is not a column reference",
            ));
        };
        let column = reference.as_expr_column_unwrap();
        columns.push(schema.column(table.columns[column.column].id));
    }
    Ok(Some(columns))
}

/// Builds the record id and CONTENT object for one insert row.
///
/// Primary-key columns feed the record id; every other non-null column becomes
/// a CONTENT field.
fn build_row(
    schema: &db::Schema,
    table: &db::Table,
    target: &stmt::InsertTable,
    row: &stmt::Expr,
) -> toasty_core::Result<(SurValue, Object)> {
    let mut content = Object::new();
    let mut pk_values: Vec<(usize, stmt::Value)> = Vec::new();

    for (position, column_id) in target.columns.iter().enumerate() {
        let column = schema.column(*column_id);
        let value = row_value(row, position)?;

        if crate::expr::is_primary_key(table, column) {
            // Record its position in the primary key so composite keys keep
            // column order.
            let pk_pos = table
                .primary_key
                .columns
                .iter()
                .position(|id| *id == column.id)
                .unwrap_or(0);
            pk_values.push((pk_pos, value.clone()));
        } else if !value.is_null() {
            content.insert(column.name.clone(), to_surreal(value)?);
        }
    }

    let key_value = pk_to_value(&mut pk_values);
    let rid = record_id(table, &key_value)?;
    Ok((SurValue::RecordId(rid), content))
}

/// Assembles the primary-key value from collected `(position, value)` pairs:
/// a single scalar for one column, or an ordered record for a composite key.
fn pk_to_value(pk_values: &mut Vec<(usize, stmt::Value)>) -> stmt::Value {
    pk_values.sort_by_key(|(pos, _)| *pos);
    if pk_values.len() == 1 {
        pk_values.remove(0).1
    } else {
        stmt::Value::record_from_vec(pk_values.iter().map(|(_, v)| v.clone()).collect())
    }
}

/// Extracts the literal value at `position` in a values row.
fn row_value(row: &stmt::Expr, position: usize) -> toasty_core::Result<&stmt::Value> {
    match row.entry(position) {
        Some(stmt::Entry::Value(value)) | Some(stmt::Entry::Expr(stmt::Expr::Value(value))) => {
            Ok(value)
        }
        _ => Err(toasty_core::Error::invalid_statement(format!(
            "SurrealDB insert row entry did not lower to a literal at position {position}"
        ))),
    }
}
