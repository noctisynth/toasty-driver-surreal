//! `SELECT <pk> FROM <table> WHERE <index filter>` translation for
//! [`Operation::FindPkByIndex`](toasty_core::driver::operation::Operation).

use toasty_core::driver::{ExecResponse, operation};
use toasty_core::schema::db;
use toasty_core::stmt::{self, ExprContext};

use crate::conn::{Connection, run_query, take_rows};
use crate::expr::{self, Binds};
use crate::op::{project_columns, row_to_record};

impl Connection {
    pub(crate) async fn exec_find_pk_by_index(
        &mut self,
        schema: &db::Schema,
        op: operation::FindPkByIndex,
    ) -> toasty_core::Result<ExecResponse> {
        let table = schema.table(op.table);
        let cx = ExprContext::new_with_target(schema, table);

        let mut binds = Binds::default();
        let predicate = expr::render(&cx, table, &mut binds, &op.filter)?;

        // Return the primary-key columns so the engine can follow up with a
        // GetByKey.
        let pk_columns: Vec<&db::Column> = table.primary_key_columns().collect();
        let projection = project_columns(table, pk_columns.iter().copied());
        let table_name = expr::escape_ident(&table.name);
        let sql = format!("SELECT {projection} FROM {table_name} WHERE {predicate}");

        let mut response = run_query(&self.db, sql, binds.into_vec()).await?;
        let rows = take_rows(&mut response, 0)?;

        let mut records = Vec::with_capacity(rows.len());
        for row in rows {
            records.push(stmt::Value::from(row_to_record(
                &row,
                pk_columns.iter().copied(),
            )?));
        }

        Ok(ExecResponse::value_stream(stmt::ValueStream::from_vec(
            records,
        )))
    }
}
