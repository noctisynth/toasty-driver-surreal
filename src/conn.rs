//! The [`Connection`] implementation: dispatches Toasty [`Operation`]s to
//! SurrealQL execution and classifies SurrealDB errors.

use std::sync::Arc;

use async_trait::async_trait;
use surrealdb::Surreal;
use surrealdb::engine::local::Db;
use toasty_core::driver::ExecResponse;
use toasty_core::driver::operation::Operation;
use toasty_core::schema::Schema;
use toasty_core::schema::db::{self, AppliedMigration};

/// An open session to an embedded SurrealDB database.
///
/// Wraps a session-scoped [`Surreal`] handle (namespace and database already
/// selected). Cloning the handle shares the underlying store, so every pool
/// slot sees the same data.
pub struct Connection {
    pub(crate) db: Surreal<Db>,
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection").finish_non_exhaustive()
    }
}

impl Connection {
    pub(crate) fn new(db: Surreal<Db>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl toasty_core::driver::Connection for Connection {
    async fn exec(
        &mut self,
        schema: &Arc<Schema>,
        op: Operation,
    ) -> toasty_core::Result<ExecResponse> {
        tracing::trace!(driver = "surrealdb", op = %op.name(), "driver exec");

        match op {
            // The published toasty 0.10 engine sends key-value inserts as a
            // `QuerySql` wrapping a `Statement::Insert` (the dedicated
            // `Operation::Insert` variant is unused by this engine version).
            // Dispatch on the inner statement; a KV driver only ever receives
            // an insert here.
            Operation::QuerySql(op) => match op.stmt {
                toasty_core::stmt::Statement::Insert(insert) => {
                    self.exec_insert(&schema.db, insert, op.ret).await
                }
                other => Err(toasty_core::Error::unsupported_feature(format!(
                    "SurrealDB driver only accepts insert statements via QuerySql, got: {}",
                    other.name()
                ))),
            },
            Operation::Insert(op) => {
                let toasty_core::stmt::Statement::Insert(insert) = op.stmt else {
                    return Err(toasty_core::Error::invalid_statement(
                        "Insert operation did not carry an insert statement",
                    ));
                };
                self.exec_insert(&schema.db, insert, op.ret).await
            }
            Operation::GetByKey(op) => self.exec_get_by_key(&schema.db, op).await,
            Operation::QueryPk(op) => self.exec_query_pk(&schema.db, op).await,
            Operation::FindPkByIndex(op) => self.exec_find_pk_by_index(&schema.db, op).await,
            Operation::Scan(op) => self.exec_scan(&schema.db, op).await,
            Operation::UpdateByKey(op) => self.exec_update_by_key(&schema.db, op).await,
            Operation::DeleteByKey(op) => self.exec_delete_by_key(&schema.db, op).await,
            Operation::Upsert(op) => self.exec_upsert(&schema.db, op).await,
            Operation::RawSql(_) => Err(toasty_core::Error::unsupported_feature(
                "SurrealDB driver does not support raw SQL",
            )),
            Operation::Transaction(_) => Err(toasty_core::Error::unsupported_feature(
                "SurrealDB driver does not support explicit transactions yet",
            )),
            // Defensive arm: `Operation` is not `#[non_exhaustive]`, so this is
            // unreachable today, but keeping it means a new variant in a future
            // toasty-core release degrades to a structured error instead of a
            // hard compile failure (spec §12). The lint is allowed only here.
            #[allow(unreachable_patterns)]
            other => Err(toasty_core::Error::unsupported_feature(format!(
                "SurrealDB driver does not support operation: {}",
                other.name()
            ))),
        }
    }

    async fn push_schema(&mut self, schema: &Schema) -> toasty_core::Result<()> {
        for table in &schema.db.tables {
            self.define_table(table).await?;
        }
        Ok(())
    }

    async fn applied_migrations(&mut self) -> toasty_core::Result<Vec<AppliedMigration>> {
        Err(toasty_core::Error::unsupported_feature(
            "SurrealDB driver does not implement migration tracking yet",
        ))
    }

    async fn apply_migration(
        &mut self,
        _id: u64,
        _name: &str,
        _migration: &db::Migration,
    ) -> toasty_core::Result<()> {
        Err(toasty_core::Error::unsupported_feature(
            "SurrealDB driver does not implement migrations yet",
        ))
    }
}

impl Connection {
    /// Defines a table and its non-primary-key indices. The primary key is the
    /// record id, so it needs no separate index.
    async fn define_table(&mut self, table: &db::Table) -> toasty_core::Result<()> {
        let table_name = crate::expr::escape_ident(&table.name);
        self.db
            .query(format!(
                "DEFINE TABLE IF NOT EXISTS {table_name} SCHEMALESS"
            ))
            .await
            .map_err(classify_error)?
            .check()
            .map_err(classify_error)?;

        for index in &table.indices {
            if index.primary_key {
                continue;
            }
            let index_name = crate::expr::escape_ident(&index.name);
            let cols = index
                .columns
                .iter()
                .map(|ic| crate::expr::escape_ident(&table.column(ic.column).name))
                .collect::<Vec<_>>()
                .join(", ");
            let unique = if index.unique { " UNIQUE" } else { "" };
            self.db
                .query(format!(
                    "DEFINE INDEX IF NOT EXISTS {index_name} ON TABLE {table_name} COLUMNS {cols}{unique}"
                ))
                .await
                .map_err(classify_error)?
                .check()
                .map_err(classify_error)?;
        }

        Ok(())
    }
}

/// Classifies a [`surrealdb::Error`] into the appropriate
/// [`toasty_core::Error`] variant.
///
/// A duplicate-record error maps to [`toasty_core::Error::condition_failed`] so
/// the engine can recognize a unique conflict; everything else is a generic
/// driver operation failure. The raw SurrealDB message is preserved for
/// diagnostics but no application data is added.
pub(crate) fn classify_error(err: surrealdb::Error) -> toasty_core::Error {
    let message = err.to_string();
    if message.contains("already exists") {
        toasty_core::Error::condition_failed(message)
    } else {
        toasty_core::Error::driver_operation_failed(err)
    }
}

/// Runs a SurrealQL query, binding each `(name, value)` pair, and returns the
/// response after checking for statement-level errors.
pub(crate) async fn run_query(
    db: &Surreal<Db>,
    sql: String,
    binds: Vec<(String, surrealdb::types::Value)>,
) -> toasty_core::Result<surrealdb::IndexedResults> {
    // The SQL text is safe to trace (values are bound separately and not
    // logged here), so surface it at trace level for debugging.
    tracing::trace!(driver = "surrealdb", %sql, "surrealql");
    let mut query = db.query(sql);
    for (name, value) in binds {
        query = query.bind((name, value));
    }
    query
        .await
        .map_err(classify_error)?
        .check()
        .map_err(classify_error)
}

/// Extracts the rows of statement `index` from a response as native
/// [`surrealdb::types::Value`]s (always an array of row objects).
pub(crate) fn take_rows(
    response: &mut surrealdb::IndexedResults,
    index: usize,
) -> toasty_core::Result<Vec<surrealdb::types::Value>> {
    let value: surrealdb::types::Value = response.take(index).map_err(classify_error)?;
    match value {
        surrealdb::types::Value::Array(arr) => Ok(arr.into_inner()),
        surrealdb::types::Value::None | surrealdb::types::Value::Null => Ok(Vec::new()),
        other => Ok(vec![other]),
    }
}
