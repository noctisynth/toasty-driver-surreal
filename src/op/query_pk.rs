//! `SELECT ... FROM <table> WHERE ...` translation for
//! [`Operation::QueryPk`](toasty_core::driver::operation::Operation).

use surrealdb::types::Value as SurValue;
use toasty_core::driver::operation::Pagination;
use toasty_core::driver::{ExecResponse, Rows, operation};
use toasty_core::schema::db;
use toasty_core::stmt::{self, ExprContext};

use crate::conn::{Connection, take_rows};
use crate::expr::{self, Binds};
use crate::op::{project_columns, row_to_record};

/// Hidden alias used to project a record-id sort key so `ORDER BY` can
/// reference it (SurrealQL rejects `record::id(id)[k]` as an order idiom) and
/// so keyset cursors can read it back off each row.
pub(crate) const SORT_ALIAS: &str = "__toasty_sort_key";

impl Connection {
    pub(crate) async fn exec_query_pk(
        &mut self,
        schema: &db::Schema,
        op: operation::QueryPk,
    ) -> toasty_core::Result<ExecResponse> {
        let table = schema.table(op.table);
        let cx = ExprContext::new_with_target(schema, table);
        let select: Vec<&db::Column> = op.select.iter().map(|id| schema.column(*id)).collect();

        let mut binds = Binds::default();

        // The pk_filter and optional post-filter are ANDed together.
        let mut predicate = expr::render(&cx, table, &mut binds, &op.pk_filter)?;
        if let Some(filter) = &op.filter {
            let extra = expr::render(&cx, table, &mut binds, filter)?;
            predicate = format!("({predicate}) AND ({extra})");
        }

        // The sort key is the last primary-key column (the "local"/range key).
        let sort_column = table.primary_key.columns.last().map(|id| table.column(*id));
        let sort_ref = sort_column
            .map(|c| expr::column_ref(table, c))
            .unwrap_or_else(|| "id".to_string());

        run_keyset_select(
            self,
            KeysetSelect {
                table_name: &expr::escape_ident(&table.name),
                projection: &project_columns(table, select.iter().copied()),
                predicate: Some(predicate),
                sort_ref: &sort_ref,
                sort_column,
                order: op.order,
                limit: op.limit.as_ref(),
                binds,
                select: &select,
            },
        )
        .await
    }
}

/// Inputs describing a keyset/offset `SELECT` to run and decode.
pub(crate) struct KeysetSelect<'a> {
    pub table_name: &'a str,
    pub projection: &'a str,
    pub predicate: Option<String>,
    pub sort_ref: &'a str,
    pub sort_column: Option<&'a db::Column>,
    pub order: Option<stmt::Direction>,
    pub limit: Option<&'a Pagination>,
    pub binds: Binds,
    pub select: &'a [&'a db::Column],
}

/// Runs a `SELECT` with optional ordering and pagination, decoding rows and —
/// for cursor pagination — extracting a keyset `next_cursor` from the last row.
pub(crate) async fn run_keyset_select(
    connection: &Connection,
    input: KeysetSelect<'_>,
) -> toasty_core::Result<ExecResponse> {
    let KeysetSelect {
        table_name,
        projection,
        predicate,
        sort_ref,
        sort_column,
        order,
        limit,
        mut binds,
        select,
    } = input;

    // Cursor pagination needs a deterministic order; default to ascending on
    // the sort key when the caller did not request one.
    let cursor_mode = matches!(limit, Some(Pagination::Cursor { .. }));
    let effective_order = order.or(if cursor_mode {
        Some(stmt::Direction::Asc)
    } else {
        None
    });

    let mut predicates: Vec<String> = predicate.into_iter().collect();

    // Keyset resume: `sort_ref <op> $cursor` where `<op>` follows the order.
    if let Some(Pagination::Cursor {
        after: Some(cursor),
        ..
    }) = limit
    {
        let op = match effective_order {
            Some(stmt::Direction::Desc) => "<",
            _ => ">",
        };
        let placeholder = binds.push(crate::value::to_surreal(cursor)?);
        predicates.push(format!("{sort_ref} {op} {placeholder}"));
    }

    let where_clause = if predicates.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", predicates.join(" AND "))
    };

    // Project the sort key under a hidden alias so ORDER BY / keyset can use
    // it and we can read the cursor back. Skip when there is no sort key.
    let (projection, order_clause) = match (effective_order, sort_column) {
        (Some(dir), Some(_)) => {
            let dir = match dir {
                stmt::Direction::Asc => "ASC",
                stmt::Direction::Desc => "DESC",
            };
            (
                format!("{projection}, {sort_ref} AS {SORT_ALIAS}"),
                format!(" ORDER BY {SORT_ALIAS} {dir}"),
            )
        }
        _ => (projection.to_string(), String::new()),
    };

    let mut sql = format!("SELECT {projection} FROM {table_name}{where_clause}{order_clause}");
    apply_pagination(&mut sql, &limit.cloned());

    let mut response = connection.run_query(sql, binds.into_vec()).await?;
    let rows = take_rows(&mut response, 0)?;

    let next_cursor: Option<stmt::Value> = if cursor_mode {
        keyset_cursor(&rows, limit, sort_column)?
    } else {
        None
    };

    let mut records = Vec::with_capacity(rows.len());
    for row in &rows {
        records.push(stmt::Value::from(row_to_record(
            row,
            select.iter().copied(),
        )?));
    }

    Ok(ExecResponse {
        values: Rows::Stream(stmt::ValueStream::from_vec(records)),
        next_cursor,
        prev_cursor: None,
    })
}

/// Builds the next-page cursor from the last row's sort-key alias, but only
/// when the page was full (a short page means there is no next page).
fn keyset_cursor(
    rows: &[SurValue],
    limit: Option<&Pagination>,
    sort_column: Option<&db::Column>,
) -> toasty_core::Result<Option<stmt::Value>> {
    let Some(Pagination::Cursor { page_size, .. }) = limit else {
        return Ok(None);
    };
    if rows.len() < *page_size as usize {
        return Ok(None);
    }
    let (Some(last), Some(column)) = (rows.last(), sort_column) else {
        return Ok(None);
    };
    let SurValue::Object(obj) = last else {
        return Ok(None);
    };
    let Some(value) = obj.get(SORT_ALIAS).cloned() else {
        return Ok(None);
    };
    Ok(Some(crate::value::from_surreal(value, &column.ty)?))
}

/// Appends `LIMIT`/`START` for the requested pagination, if any.
pub(crate) fn apply_pagination(sql: &mut String, limit: &Option<Pagination>) {
    match limit {
        None => {}
        Some(Pagination::Offset { limit, offset }) => {
            sql.push_str(&format!(" LIMIT {limit}"));
            if let Some(offset) = offset {
                sql.push_str(&format!(" START {offset}"));
            }
        }
        Some(Pagination::Cursor { page_size, .. }) => {
            sql.push_str(&format!(" LIMIT {page_size}"));
        }
    }
}
