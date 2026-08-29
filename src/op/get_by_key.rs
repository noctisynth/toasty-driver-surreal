//! `SELECT ... FROM <record ids>` translation for
//! [`Operation::GetByKey`](toasty_core::driver::operation::Operation).

use surrealdb::types::Value as SurValue;
use toasty_core::driver::{ExecResponse, operation};
use toasty_core::schema::db;
use toasty_core::stmt;

use crate::conn::{Connection, take_rows};
use crate::op::{project_columns, row_to_record};
use crate::record_id::record_id;

impl Connection {
    pub(crate) async fn exec_get_by_key(
        &mut self,
        schema: &db::Schema,
        op: operation::GetByKey,
    ) -> toasty_core::Result<ExecResponse> {
        let table = schema.table(op.table);
        let select: Vec<&db::Column> = op.select.iter().map(|id| schema.column(*id)).collect();

        if op.keys.is_empty() {
            return Ok(ExecResponse::empty_value_stream());
        }

        // Bind each key as a record id and select from the list of them.
        let mut binds = Vec::with_capacity(op.keys.len());
        let mut targets = Vec::with_capacity(op.keys.len());
        for (i, key) in op.keys.iter().enumerate() {
            let rid = record_id(table, key)?;
            let name = format!("k{i}");
            targets.push(format!("${name}"));
            binds.push((name, SurValue::RecordId(rid)));
        }

        let projection = project_columns(table, select.iter().copied());
        let sql = format!("SELECT {projection} FROM {}", targets.join(", "));

        let mut response = self.run_query(sql, binds).await?;
        let rows = take_rows(&mut response, 0)?;

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
}
