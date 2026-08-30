//! The [`Connection`] implementation: dispatches Toasty [`Operation`]s to
//! SurrealQL execution and classifies SurrealDB errors.

use std::sync::Arc;

use async_trait::async_trait;
use surrealdb::Surreal;
use surrealdb::engine::local::Db;
use surrealdb::types::{RecordId, Value as SurValue};
use toasty_core::driver::ExecResponse;
use toasty_core::driver::operation::{Operation, Transaction, TransactionMode};
use toasty_core::schema::Schema;
use toasty_core::schema::db::{self, AppliedMigration};

/// An open session to an embedded SurrealDB database.
///
/// Wraps a session-scoped [`Surreal`] handle (namespace and database already
/// selected). Cloning the handle shares the underlying store, so every pool
/// slot sees the same data.
pub struct Connection {
    pub(crate) db: Surreal<Db>,
    transaction: Option<surrealdb::method::Transaction<Db>>,
    read_only: bool,
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection").finish_non_exhaustive()
    }
}

impl Connection {
    pub(crate) fn new(db: Surreal<Db>) -> Self {
        Self {
            db,
            transaction: None,
            read_only: false,
        }
    }

    fn ensure_writable(&self, operation: &str) -> toasty_core::Result<()> {
        if self.read_only {
            Err(toasty_core::Error::read_only_transaction(format!(
                "{operation} is not allowed in a read-only transaction"
            )))
        } else {
            Ok(())
        }
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
                    self.ensure_writable("Insert")?;
                    self.exec_insert(&schema.db, insert, op.ret).await
                }
                other => Err(toasty_core::Error::unsupported_feature(format!(
                    "SurrealDB driver only accepts insert statements via QuerySql, got: {}",
                    other.name()
                ))),
            },
            Operation::Insert(op) => {
                self.ensure_writable("Insert")?;
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
            Operation::UpdateByKey(op) => {
                self.ensure_writable("UpdateByKey")?;
                self.exec_update_by_key(&schema.db, op).await
            }
            Operation::DeleteByKey(op) => {
                self.ensure_writable("DeleteByKey")?;
                self.exec_delete_by_key(&schema.db, op).await
            }
            Operation::Upsert(op) => {
                self.ensure_writable("Upsert")?;
                self.exec_upsert(&schema.db, op).await
            }
            Operation::RawSql(_) => Err(toasty_core::Error::unsupported_feature(
                "SurrealDB driver does not support raw SQL",
            )),
            Operation::Transaction(op) => self.exec_transaction(op).await,
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
        self.ensure_no_active_transaction("query applied migrations")?;
        self.ensure_migrations_table().await?;

        let table = crate::expr::escape_ident(crate::migration::TRACKING_TABLE);
        let alias = crate::expr::escape_ident(crate::migration::TRACKING_ID_ALIAS);
        let mut response = self
            .db
            .query(format!("SELECT record::id(id) AS {alias} FROM {table}"))
            .await
            .map_err(classify_error)?
            .check()
            .map_err(classify_error)?;
        let rows = take_rows(&mut response, 0)?;
        let mut migrations = Vec::with_capacity(rows.len());

        for row in rows {
            let SurValue::Object(row) = row else {
                return Err(invalid_migration_tracking_row());
            };
            let Some(SurValue::String(id)) = row.get(crate::migration::TRACKING_ID_ALIAS) else {
                return Err(invalid_migration_tracking_row());
            };
            let id = id
                .parse::<u64>()
                .map_err(|_| invalid_migration_tracking_row())?;
            migrations.push(AppliedMigration::new(id));
        }

        Ok(migrations)
    }

    async fn apply_migration(
        &mut self,
        id: u64,
        name: &str,
        migration: &db::Migration,
    ) -> toasty_core::Result<()> {
        self.ensure_no_active_transaction("apply a migration")?;
        self.ensure_migrations_table().await?;

        tracing::info!(
            driver = "surrealdb",
            migration_id = id,
            "applying migration"
        );
        let transaction = self
            .db
            .clone()
            .begin()
            .await
            .map_err(classify_migration_error)?;

        let apply_result = async {
            for (statement_index, statement) in migration.statements().into_iter().enumerate() {
                if statement.trim().is_empty() {
                    continue;
                }
                tracing::trace!(
                    driver = "surrealdb",
                    migration_id = id,
                    statement_index,
                    "executing migration statement"
                );
                transaction
                    .query(statement)
                    .await
                    .map_err(classify_migration_error)?
                    .check()
                    .map_err(classify_migration_error)?;
            }

            let record_id = RecordId::new(crate::migration::TRACKING_TABLE, id.to_string());
            transaction
                .query(
                    "CREATE $migration_record SET name = $migration_name, \
                     applied_at = time::now() RETURN NONE",
                )
                .bind(("migration_record", SurValue::RecordId(record_id)))
                .bind(("migration_name", SurValue::String(name.to_string())))
                .await
                .map_err(classify_migration_error)?
                .check()
                .map_err(classify_migration_error)?;

            Ok::<(), toasty_core::Error>(())
        }
        .await;

        if let Err(apply_error) = apply_result {
            if let Err(cancel_error) = transaction.cancel().await {
                return Err(classify_migration_error(cancel_error));
            }
            return Err(apply_error);
        }

        transaction
            .commit()
            .await
            .map_err(classify_migration_error)?;
        Ok(())
    }
}

impl Connection {
    fn ensure_no_active_transaction(&self, operation: &str) -> toasty_core::Result<()> {
        if self.transaction.is_some() {
            Err(toasty_core::Error::invalid_statement(format!(
                "cannot {operation} while a SurrealDB transaction is active"
            )))
        } else {
            Ok(())
        }
    }

    async fn ensure_migrations_table(&self) -> toasty_core::Result<()> {
        let table = crate::expr::escape_ident(crate::migration::TRACKING_TABLE);
        self.db
            .query(format!("DEFINE TABLE IF NOT EXISTS {table} SCHEMALESS"))
            .await
            .map_err(classify_error)?
            .check()
            .map_err(classify_error)?;
        Ok(())
    }

    async fn exec_transaction(&mut self, op: Transaction) -> toasty_core::Result<ExecResponse> {
        match op {
            Transaction::Start {
                isolation,
                read_only,
                mode,
            } => {
                if isolation.is_some() {
                    return Err(toasty_core::Error::unsupported_feature(
                        "SurrealDB driver does not support explicit transaction isolation levels",
                    ));
                }
                if mode != TransactionMode::Default {
                    return Err(toasty_core::Error::unsupported_feature(format!(
                        "SurrealDB driver does not support TransactionMode::{mode:?}"
                    )));
                }
                if self.transaction.is_some() {
                    return Err(toasty_core::Error::invalid_statement(
                        "SurrealDB connection already has an active transaction",
                    ));
                }

                let transaction = self.db.clone().begin().await.map_err(classify_error)?;
                self.transaction = Some(transaction);
                self.read_only = read_only;
                Ok(ExecResponse::count(0))
            }
            Transaction::Commit => {
                let Some(transaction) = self.transaction.take() else {
                    return Err(toasty_core::Error::invalid_statement(
                        "cannot commit without an active SurrealDB transaction",
                    ));
                };
                self.read_only = false;
                transaction.commit().await.map_err(classify_error)?;
                Ok(ExecResponse::count(0))
            }
            Transaction::Rollback => {
                let Some(transaction) = self.transaction.take() else {
                    return Err(toasty_core::Error::invalid_statement(
                        "cannot roll back without an active SurrealDB transaction",
                    ));
                };
                self.read_only = false;
                transaction.cancel().await.map_err(classify_error)?;
                Ok(ExecResponse::count(0))
            }
            Transaction::Savepoint(_)
            | Transaction::ReleaseSavepoint(_)
            | Transaction::RollbackToSavepoint(_) => Err(toasty_core::Error::unsupported_feature(
                "SurrealDB SDK 3.2.4 does not support transaction savepoints",
            )),
        }
    }

    /// Defines a table and its non-primary-key indices. The primary key is the
    /// record id, so it needs no separate index.
    async fn define_table(&mut self, table: &db::Table) -> toasty_core::Result<()> {
        for statement in crate::migration::define_table_statements(table, true) {
            self.db
                .query(statement)
                .await
                .map_err(classify_error)?
                .check()
                .map_err(classify_error)?;
        }

        Ok(())
    }
}

fn invalid_migration_tracking_row() -> toasty_core::Error {
    toasty_core::Error::serialization_failure(
        "SurrealDB migration tracking table contains an invalid migration ID",
    )
}

fn classify_migration_error(err: surrealdb::Error) -> toasty_core::Error {
    let message = err.to_string();
    if is_transaction_conflict(&err) || message.contains("Transaction conflict:") {
        toasty_core::Error::serialization_failure("SurrealDB migration transaction conflicted")
    } else if message.contains("manual migration required:") {
        toasty_core::Error::unsupported_feature(
            "SurrealDB migration requires manual editing before it can be applied",
        )
    } else if message.contains("already exists") {
        toasty_core::Error::condition_failed("SurrealDB migration object already exists")
    } else {
        toasty_core::Error::driver_operation_failed(std::io::Error::other(
            "SurrealDB migration statement failed",
        ))
    }
}

/// Classifies a [`surrealdb::Error`] into the appropriate
/// [`toasty_core::Error`] variant.
///
/// A transaction conflict maps to [`toasty_core::Error::serialization_failure`]
/// and a duplicate-record error maps to
/// [`toasty_core::Error::condition_failed`]; everything else is a generic
/// driver operation failure. The raw SurrealDB message is preserved for
/// diagnostics but no application data is added.
pub(crate) fn classify_error(err: surrealdb::Error) -> toasty_core::Error {
    let message = err.to_string();
    if is_transaction_conflict(&err) || message.contains("Transaction conflict:") {
        toasty_core::Error::serialization_failure(message)
    } else if message.contains("already exists") {
        toasty_core::Error::condition_failed(message)
    } else {
        toasty_core::Error::driver_operation_failed(err)
    }
}

fn is_transaction_conflict(err: &surrealdb::Error) -> bool {
    let mut current = Some(err);
    while let Some(error) = current {
        if matches!(
            error.query_details(),
            Some(surrealdb::types::QueryError::TransactionConflict)
        ) {
            return true;
        }
        current = error.cause();
    }
    false
}

impl Connection {
    /// Runs a SurrealQL query through the active transaction when present,
    /// binding each `(name, value)` pair, and checks statement-level errors.
    pub(crate) async fn run_query(
        &self,
        sql: String,
        binds: Vec<(String, surrealdb::types::Value)>,
    ) -> toasty_core::Result<surrealdb::IndexedResults> {
        // The SQL text is safe to trace (values are bound separately and not
        // logged here), so surface it at trace level for debugging.
        tracing::trace!(driver = "surrealdb", %sql, "surrealql");

        let response = if let Some(transaction) = &self.transaction {
            let mut query = transaction.query(sql);
            for (name, value) in binds {
                query = query.bind((name, value));
            }
            query.await
        } else {
            let mut query = self.db.query(sql);
            for (name, value) in binds {
                query = query.bind((name, value));
            }
            query.await
        };

        response
            .map_err(classify_error)?
            .check()
            .map_err(classify_error)
    }
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

#[cfg(test)]
mod tests {
    use super::classify_error;

    #[test]
    fn classifies_typed_transaction_conflict() {
        let error = surrealdb::Error::query(
            "transaction conflict".to_string(),
            Some(surrealdb::types::QueryError::TransactionConflict),
        );

        assert!(classify_error(error).is_serialization_failure());
    }

    #[test]
    fn classifies_embedded_commit_conflict_compatibility_message() {
        let error = surrealdb::Error::internal(
            "Transaction conflict: Transaction write conflict. This transaction can be retried"
                .to_string(),
        );

        assert!(classify_error(error).is_serialization_failure());
    }
}
