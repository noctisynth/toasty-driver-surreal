//! `DELETE <record id> ...` translation for
//! [`Operation::DeleteByKey`](toasty_core::driver::operation::Operation).

use surrealdb::types::Value as SurValue;
use toasty_core::driver::{ExecResponse, operation};
use toasty_core::schema::db;
use toasty_core::stmt::ExprContext;

use crate::conn::{Connection, take_rows};
use crate::expr::{self, Binds};
use crate::record_id::record_id;

impl Connection {
    pub(crate) async fn exec_delete_by_key(
        &mut self,
        schema: &db::Schema,
        op: operation::DeleteByKey,
    ) -> toasty_core::Result<ExecResponse> {
        let table = schema.table(op.table);

        // The engine shreds multi-key deletes into one op per key.
        let [key] = &op.keys[..] else {
            return Err(toasty_core::Error::invalid_statement(format!(
                "SurrealDB delete expects exactly one key, got {}",
                op.keys.len()
            )));
        };

        let cx = ExprContext::new_with_target(schema, table);
        let mut binds = Binds::default();

        let mut predicates: Vec<String> = Vec::new();
        if let Some(filter) = &op.filter {
            predicates.push(expr::render(&cx, table, &mut binds, filter)?);
        }
        if let Some(condition) = &op.condition {
            predicates.push(expr::render(&cx, table, &mut binds, condition)?);
        }

        let rid = record_id(table, key)?;
        binds.push_named("rid", SurValue::RecordId(rid));

        let where_clause = if predicates.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", predicates.join(" AND "))
        };

        // RETURN BEFORE lets us count what was actually deleted.
        let sql = format!("DELETE $rid{where_clause} RETURN BEFORE");
        let mut response = self.run_query(sql, binds.into_vec()).await?;
        let rows = take_rows(&mut response, 0)?;

        if rows.is_empty() && op.condition.is_some() {
            return Err(toasty_core::Error::condition_failed(
                "SurrealDB delete condition did not match",
            ));
        }

        Ok(ExecResponse::count(rows.len() as u64))
    }
}
