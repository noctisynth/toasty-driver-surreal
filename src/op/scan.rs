//! `SELECT ... FROM <table>` (full scan) translation for
//! [`Operation::Scan`](toasty_core::driver::operation::Operation).

use toasty_core::driver::{ExecResponse, operation};
use toasty_core::schema::db::{self, ColumnId};
use toasty_core::stmt::ExprContext;

use crate::conn::Connection;
use crate::expr::{self, Binds};
use crate::op::project_columns;
use crate::op::query_pk::{KeysetSelect, run_keyset_select};

impl Connection {
    pub(crate) async fn exec_scan(
        &mut self,
        schema: &db::Schema,
        op: operation::Scan,
    ) -> toasty_core::Result<ExecResponse> {
        let table = schema.table(op.table);
        let cx = ExprContext::new_with_target(schema, table);

        // `op.columns` are indices into the table's column list.
        let select: Vec<&db::Column> = op
            .columns
            .iter()
            .map(|&idx| {
                schema.column(ColumnId {
                    table: op.table,
                    index: idx,
                })
            })
            .collect();

        let mut binds = Binds::default();
        let predicate = match &op.filter {
            Some(filter) => Some(expr::render(&cx, table, &mut binds, filter)?),
            None => None,
        };

        // Scans have no ORDER BY (capability `scan_supports_sort = false`), but
        // cursor pagination still needs a stable key to resume from; use the
        // record id via the last primary-key column.
        let sort_column = table.primary_key.columns.last().map(|id| table.column(*id));
        let sort_ref = sort_column
            .map(|c| expr::column_ref(table, c))
            .unwrap_or_else(|| "id".to_string());

        run_keyset_select(
            self,
            KeysetSelect {
                table_name: &expr::escape_ident(&table.name),
                projection: &project_columns(table, select.iter().copied()),
                predicate,
                sort_ref: &sort_ref,
                sort_column,
                // No caller-requested order; keyset resume defaults ascending.
                order: None,
                limit: op.limit.as_ref(),
                binds,
                select: &select,
            },
        )
        .await
    }
}
