#![warn(missing_docs)]

//! A [Toasty](https://github.com/tokio-rs/toasty) driver for
//! [SurrealDB](https://surrealdb.com), backed by the embedded `surrealdb` SDK.
//!
//! SurrealDB is integrated as a **key-value / document** backend: the driver
//! reports [`Capability::sql`](toasty_core::driver::Capability::sql) as `None`,
//! so Toasty's query engine emits key-value `Operation`s (`Insert`,
//! `GetByKey`, `QueryPk`, `Scan`, `UpdateByKey`, `DeleteByKey`, `Upsert`,
//! `FindPkByIndex`) which this driver translates into SurrealQL. It does not
//! implement a new SQL dialect and does not depend on `toasty-sql`.
//!
//! The design, capability profile, value encoding, and per-operation
//! translation are frozen in `.agents/specs/driver.md`.
//!
//! # Examples
//!
//! ```no_run
//! # async fn example() -> toasty_core::Result<()> {
//! use toasty_driver_surreal::SurrealDb;
//!
//! // In-memory embedded engine (always available).
//! let driver = SurrealDb::mem();
//! # let _ = driver;
//! # Ok(())
//! # }
//! ```
//!
//! A file-backed embedded SurrealKV engine is available via
//! [`SurrealDb::surrealkv`]. RocksDB is available behind the `rocksdb` crate
//! feature via `SurrealDb::rocksdb(path)`.
//!
//! Attach the driver with `Db::builder().build(driver)`; the driver does not
//! register a URL scheme, so `Db::builder().connect(url)` is not used.

mod capability;
mod conn;
mod expr;
mod op;
mod record_id;
mod value;

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use surrealdb::Surreal;
use surrealdb::engine::local::Db;
use toasty_core::driver::{Capability, ConnectContext, Driver};
use toasty_core::schema::db::Migration;
use toasty_core::schema::diff;
use tokio::sync::Mutex;

pub use conn::Connection;

/// The embedded engine a [`SurrealDb`] driver opens.
#[derive(Debug, Clone)]
enum Engine {
    /// In-memory (`kv-mem`). Every fresh database starts empty.
    Mem,
    /// File-backed embedded SurrealKV (`kv-surrealkv`) at the given path.
    SurrealKv(PathBuf),
    /// File-backed embedded RocksDB (`kv-rocksdb`) at the given path.
    ///
    /// Only available with the `rocksdb` crate feature.
    #[cfg(feature = "rocksdb")]
    RocksDb(PathBuf),
}

/// A SurrealDB [`Driver`] backed by the embedded `surrealdb` SDK.
///
/// Construct with [`SurrealDb::mem`] (in-memory), [`SurrealDb::surrealkv`]
/// (file-backed), or `SurrealDb::rocksdb` (file-backed, requires the `rocksdb`
/// feature), optionally overriding the namespace and database with
/// [`SurrealDb::namespace`] / [`SurrealDb::database`]. Attach it with
/// `Db::builder().build(driver)`.
#[derive(Clone)]
pub struct SurrealDb {
    engine: Engine,
    namespace: String,
    database: String,
    /// Shared handle, opened lazily on first connect and reused across pool
    /// slots so that an in-memory database is genuinely shared. Cleared by
    /// [`Driver::reset_db`].
    handle: Arc<Mutex<Option<Surreal<Db>>>>,
}

impl std::fmt::Debug for SurrealDb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SurrealDb")
            .field("engine", &self.engine)
            .field("namespace", &self.namespace)
            .field("database", &self.database)
            .finish_non_exhaustive()
    }
}

const DEFAULT_NS: &str = "toasty";
const DEFAULT_DB: &str = "toasty";

impl SurrealDb {
    /// Create an in-memory (`kv-mem`) SurrealDB driver.
    pub fn mem() -> Self {
        Self::with_engine(Engine::Mem)
    }

    /// Create a file-backed embedded SurrealKV (`kv-surrealkv`) driver rooted
    /// at `path`.
    pub fn surrealkv(path: impl Into<PathBuf>) -> Self {
        Self::with_engine(Engine::SurrealKv(path.into()))
    }

    /// Create a file-backed embedded RocksDB (`kv-rocksdb`) SurrealDB driver
    /// rooted at `path`.
    ///
    /// Requires the `rocksdb` crate feature.
    #[cfg(feature = "rocksdb")]
    pub fn rocksdb(path: impl Into<PathBuf>) -> Self {
        Self::with_engine(Engine::RocksDb(path.into()))
    }

    fn with_engine(engine: Engine) -> Self {
        Self {
            engine,
            namespace: DEFAULT_NS.to_string(),
            database: DEFAULT_DB.to_string(),
            handle: Arc::new(Mutex::new(None)),
        }
    }

    /// Override the SurrealDB namespace (default `"toasty"`).
    pub fn namespace(mut self, ns: impl Into<String>) -> Self {
        self.namespace = ns.into();
        self
    }

    /// Override the SurrealDB database (default `"toasty"`).
    pub fn database(mut self, db: impl Into<String>) -> Self {
        self.database = db.into();
        self
    }

    /// Opens the shared handle on first use and returns a session-scoped clone
    /// with the namespace and database selected.
    async fn session(&self) -> toasty_core::Result<Surreal<Db>> {
        let mut slot = self.handle.lock().await;
        if slot.is_none() {
            let db = match &self.engine {
                Engine::Mem => Surreal::new::<surrealdb::engine::local::Mem>(())
                    .await
                    .map_err(conn::classify_error)?,
                Engine::SurrealKv(path) => {
                    Surreal::new::<surrealdb::engine::local::SurrealKv>(path.as_path())
                        .await
                        .map_err(conn::classify_error)?
                }
                #[cfg(feature = "rocksdb")]
                Engine::RocksDb(path) => {
                    Surreal::new::<surrealdb::engine::local::RocksDb>(path.as_path())
                        .await
                        .map_err(conn::classify_error)?
                }
            };
            *slot = Some(db);
        }

        // A cloned handle shares the underlying store but gets a fresh session,
        // so namespace/database must be selected on the clone.
        let session = slot.as_ref().expect("handle opened above").clone();
        session
            .use_ns(self.namespace.clone())
            .use_db(self.database.clone())
            .await
            .map_err(conn::classify_error)?;
        Ok(session)
    }
}

#[async_trait]
impl Driver for SurrealDb {
    fn url(&self) -> Cow<'_, str> {
        match &self.engine {
            Engine::Mem => Cow::Borrowed("surrealdb:mem"),
            Engine::SurrealKv(path) => {
                Cow::Owned(format!("surrealdb:surrealkv:{}", path.display()))
            }
            #[cfg(feature = "rocksdb")]
            Engine::RocksDb(path) => Cow::Owned(format!("surrealdb:rocksdb:{}", path.display())),
        }
    }

    fn capability(&self) -> &'static Capability {
        capability::surrealdb_capability()
    }

    async fn connect(
        &self,
        _cx: &ConnectContext,
    ) -> toasty_core::Result<Box<dyn toasty_core::driver::Connection>> {
        let session = self.session().await?;
        Ok(Box::new(Connection::new(session)))
    }

    fn max_connections(&self) -> Option<usize> {
        // An in-memory database only exists for as long as a handle is held;
        // cap at one connection so all work shares the same store, matching
        // the in-memory SQLite driver.
        match self.engine {
            Engine::Mem => Some(1),
            Engine::SurrealKv(_) => None,
            #[cfg(feature = "rocksdb")]
            Engine::RocksDb(_) => None,
        }
    }

    fn generate_migration(&self, _schema_diff: &diff::Schema<'_>) -> Migration {
        unimplemented!(
            "SurrealDB migration generation is not supported yet; schema is applied via push_schema"
        )
    }

    async fn reset_db(&self) -> toasty_core::Result<()> {
        // Drop the cached handle so the next connect starts fresh.
        self.handle.lock().await.take();

        let file_path = match &self.engine {
            Engine::Mem => None,
            Engine::SurrealKv(path) => Some(path),
            #[cfg(feature = "rocksdb")]
            Engine::RocksDb(path) => Some(path),
        };

        if let Some(path) = file_path
            && path.exists()
        {
            std::fs::remove_dir_all(path).map_err(toasty_core::Error::driver_operation_failed)?;
        }

        Ok(())
    }
}
