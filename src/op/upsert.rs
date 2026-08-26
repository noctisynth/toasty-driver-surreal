//! `UPSERT`/`CREATE` translation for
//! [`Operation::Upsert`](toasty_core::driver::operation::Operation).
//!
//! SurrealDB's native `UPSERT` handles the create-or-update case; the
//! insert-or-ignore case is a `CREATE` whose duplicate error is swallowed. Only
//! primary-key targets are supported, which matches the capability profile
//! (`upsert_primary_key = true`, `upsert_unique = false`).

use surrealdb::types::{Object, Value as SurValue};
use toasty_core::driver::{ExecResponse, operation};
use toasty_core::schema::db;
use toasty_core::stmt;

use crate::conn::{Connection, run_query, take_rows};
use crate::op::row_to_record;
use crate::record_id::record_id;
use crate::value::to_surreal;

impl Connection {
    pub(crate) async fn exec_upsert(
        &mut self,
        schema: &db::Schema,
        op: operation::Upsert,
    ) -> toasty_core::Result<ExecResponse> {
        let insert = op.stmt;
        let target = insert.target.as_table_unwrap();
        let table = schema.table(target.table);
        let upsert = insert.upsert.as_ref().ok_or_else(|| {
            toasty_core::Error::invalid_statement("upsert operation without clause")
        })?;

        let stmt::UpsertTarget::Columns(conflict_columns) = &upsert.target else {
            return Err(toasty_core::Error::invalid_statement(
                "SurrealDB upsert target was not lowered to columns",
            ));
        };
        let pk_columns: Vec<_> = table.primary_key_columns().map(|c| c.id).collect();
        if conflict_columns.as_slice() != pk_columns.as_slice() {
            return Err(toasty_core::Error::unsupported_feature(
                "SurrealDB upsert only supports targeting the primary key",
            ));
        }

        let stmt::ExprSet::Values(values) = &insert.source.body else {
            return Err(toasty_core::Error::invalid_statement(
                "SurrealDB upsert requires a VALUES source",
            ));
        };
        let [row] = values.rows.as_slice() else {
            return Err(toasty_core::Error::invalid_statement(
                "SurrealDB upsert requires exactly one row",
            ));
        };

        // Split the source row into the record id (primary key) and the
        // content object (remaining non-null columns).
        let mut content = Object::new();
        let mut pk_values: Vec<(usize, stmt::Value)> = Vec::new();
        for (position, column_id) in target.columns.iter().enumerate() {
            let column = schema.column(*column_id);
            let value = row_value(row, position)?;
            if crate::expr::is_primary_key(table, column) {
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
        pk_values.sort_by_key(|(pos, _)| *pos);
        let key_value = if pk_values.len() == 1 {
            pk_values.remove(0).1
        } else {
            stmt::Value::record_from_vec(pk_values.into_iter().map(|(_, v)| v).collect())
        };
        let rid = record_id(table, &key_value)?;

        let returning = returning_columns(schema, table, insert.returning.as_ref())?;

        match upsert.action {
            stmt::UpsertAction::Update => {
                // Build a SET clause that behaves correctly on both create and
                // update. SurrealDB's UPSERT runs SET in either case, so:
                //   * create-only source values use `col = col ?? value`
                //     (like DynamoDB's `if_not_exists`);
                //   * shared operator mutations (Add/Subtract/Append) fold in
                //     the declared `#[default]` via `col ?? default`.
                let mut binds = crate::expr::Binds::default();
                let mut sets: Vec<String> = Vec::new();

                for (position, column_id) in target.columns.iter().enumerate() {
                    let column = schema.column(*column_id);
                    if crate::expr::is_primary_key(table, column)
                        || upsert.shared.contains(&[column.id.index])
                    {
                        continue;
                    }
                    let value = row_value(row, position)?;
                    if value.is_null() {
                        continue;
                    }
                    let name = crate::expr::escape_ident(&column.name);
                    let ph = binds.push(to_surreal(value)?);
                    // Create-only: keep an existing value, set it when absent.
                    sets.push(format!("{name} = {name} ?? {ph}"));
                }

                for (projection, assignment) in upsert.shared.iter() {
                    let column = table.resolve(projection);
                    let name = crate::expr::escape_ident(&column.name);
                    render_shared_mutation(
                        &name,
                        assignment,
                        upsert.defaults.get(projection),
                        &mut binds,
                        &mut sets,
                    )?;
                }

                let set_clause = if sets.is_empty() {
                    // Nothing to assign beyond the record id; MERGE the (empty)
                    // content so the row is still created/touched.
                    let ph = binds.push(SurValue::Object(content));
                    format!("MERGE {ph}")
                } else {
                    format!("SET {}", sets.join(", "))
                };

                let return_clause = if returning.is_some() {
                    " RETURN AFTER"
                } else {
                    ""
                };
                let sql = format!("UPSERT $rid {set_clause}{return_clause}");

                let mut all_binds = vec![("rid".to_string(), SurValue::RecordId(rid))];
                all_binds.extend(binds.into_vec());

                let mut response = run_query(&self.db, sql, all_binds).await?;
                let rows = take_rows(&mut response, 0)?;
                finish(returning.as_deref(), rows)
            }
            stmt::UpsertAction::Ignore => {
                // Insert-or-ignore: attempt a CREATE and swallow a duplicate.
                let sql = if returning.is_some() {
                    "CREATE $rid CONTENT $data RETURN AFTER".to_string()
                } else {
                    "CREATE $rid CONTENT $data".to_string()
                };
                let binds = vec![
                    ("rid".to_string(), SurValue::RecordId(rid)),
                    ("data".to_string(), SurValue::Object(content)),
                ];
                match run_query(&self.db, sql, binds).await {
                    Ok(mut response) => {
                        let rows = take_rows(&mut response, 0)?;
                        finish(returning.as_deref(), rows)
                    }
                    Err(e) if is_conflict(&e) => {
                        // Selected target already exists → ignore.
                        if returning.is_some() {
                            Ok(ExecResponse::empty_value_stream())
                        } else {
                            Ok(ExecResponse::count(0))
                        }
                    }
                    Err(e) => Err(e),
                }
            }
        }
    }
}

/// Renders one shared upsert mutation into the `SET` clause, folding the
/// declared create-branch default into operator assignments via `col ?? d`.
fn render_shared_mutation(
    name: &str,
    assignment: &stmt::Assignment,
    default: Option<&stmt::Assignment>,
    binds: &mut crate::expr::Binds,
    sets: &mut Vec<String>,
) -> toasty_core::Result<()> {
    let default_placeholder = |binds: &mut crate::expr::Binds| -> toasty_core::Result<String> {
        match default {
            Some(stmt::Assignment::Set(stmt::Expr::Value(value))) => {
                Ok(binds.push(to_surreal(value)?))
            }
            _ => Err(toasty_core::Error::invalid_statement(
                "SurrealDB shared upsert mutation requires a literal #[default] value",
            )),
        }
    };

    match assignment {
        stmt::Assignment::Set(stmt::Expr::Value(value)) if value.is_null() => {
            sets.push(format!("{name} = NONE"));
        }
        stmt::Assignment::Set(stmt::Expr::Value(value)) => {
            let ph = binds.push(to_surreal(value)?);
            sets.push(format!("{name} = {ph}"));
        }
        stmt::Assignment::Add(stmt::Expr::Value(value)) => {
            let d = default_placeholder(binds)?;
            let ph = binds.push(to_surreal(value)?);
            sets.push(format!("{name} = ({name} ?? {d}) + {ph}"));
        }
        stmt::Assignment::Subtract(stmt::Expr::Value(value)) => {
            let d = default_placeholder(binds)?;
            let ph = binds.push(to_surreal(value)?);
            sets.push(format!("{name} = ({name} ?? {d}) - {ph}"));
        }
        stmt::Assignment::Append(stmt::Expr::Value(value)) => {
            let d = default_placeholder(binds)?;
            let list = match value {
                stmt::Value::List(_) => to_surreal(value)?,
                other => to_surreal(&stmt::Value::List(vec![other.clone()]))?,
            };
            let ph = binds.push(list);
            sets.push(format!("{name} = array::concat({name} ?? {d}, {ph})"));
        }
        other => {
            return Err(toasty_core::Error::unsupported_feature(format!(
                "SurrealDB driver does not support shared upsert mutation: {other:?}"
            )));
        }
    }
    Ok(())
}

/// Builds the final response from the returned rows.
fn finish(
    returning: Option<&[&db::Column]>,
    rows: Vec<SurValue>,
) -> toasty_core::Result<ExecResponse> {
    match returning {
        Some(columns) => {
            let mut records = Vec::with_capacity(rows.len());
            for row in rows {
                records.push(stmt::Value::from(row_to_record(
                    &row,
                    columns.iter().copied(),
                )?));
            }
            Ok(ExecResponse::value_stream(stmt::ValueStream::from_vec(
                records,
            )))
        }
        None => Ok(ExecResponse::count(rows.len().max(1) as u64)),
    }
}

/// Returns `true` when the error is a unique/record conflict (already exists),
/// which `classify_error` maps to [`toasty_core::Error::condition_failed`].
fn is_conflict(err: &toasty_core::Error) -> bool {
    err.to_string().contains("already exists")
}

/// Resolves the `RETURNING` projection into the concrete columns to decode.
fn returning_columns<'a>(
    schema: &'a db::Schema,
    table: &'a db::Table,
    returning: Option<&stmt::Returning>,
) -> toasty_core::Result<Option<Vec<&'a db::Column>>> {
    let Some(returning) = returning else {
        return Ok(None);
    };
    let expr = returning.as_project().ok_or_else(|| {
        toasty_core::Error::invalid_statement("upsert returning is not a projection")
    })?;
    let record = expr.as_record().ok_or_else(|| {
        toasty_core::Error::invalid_statement("upsert returning must be a record")
    })?;
    let mut columns = Vec::with_capacity(record.fields.len());
    for field in &record.fields {
        let stmt::Expr::Reference(reference) = field else {
            return Err(toasty_core::Error::invalid_statement(
                "upsert returning item is not a column reference",
            ));
        };
        let column = reference.as_expr_column_unwrap();
        columns.push(schema.column(table.columns[column.column].id));
    }
    Ok(Some(columns))
}

/// Extracts the literal value at `position` in a values row.
fn row_value(row: &stmt::Expr, position: usize) -> toasty_core::Result<&stmt::Value> {
    match row.entry(position) {
        Some(stmt::Entry::Value(value)) | Some(stmt::Entry::Expr(stmt::Expr::Value(value))) => {
            Ok(value)
        }
        _ => Err(toasty_core::Error::invalid_statement(format!(
            "SurrealDB upsert row entry did not lower to a literal at position {position}"
        ))),
    }
}
