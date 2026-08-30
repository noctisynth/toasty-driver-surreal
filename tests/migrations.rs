//! Driver-owned coverage for Toasty migration generation, tracking, atomic
//! application, and file-engine persistence.

use std::sync::Arc;

use toasty::Db;
use toasty_core::driver::operation::Transaction;
use toasty_core::driver::{ConnectContext, Driver};
use toasty_core::schema::{db, diff};
use toasty_driver_surreal::SurrealDb;

static EMBEDDED_MIGRATION_FILES: &[toasty::migration::MigrationFile] =
    &[toasty::migration::MigrationFile::new(
        50,
        "0050_embedded.sql",
        "DEFINE TABLE embedded_migration_items SCHEMALESS",
    )];
static EMBEDDED_MIGRATIONS: toasty::migration::MigrationSet =
    toasty::migration::MigrationSet::new(EMBEDDED_MIGRATION_FILES);

#[derive(Debug, toasty::Model)]
#[table = "migration_items"]
struct MigrationItem {
    #[key]
    id: i64,
    #[unique]
    value: String,
}

#[derive(Debug, toasty::Model)]
#[table = "migration_items"]
struct RenamedMigrationItem {
    #[key]
    id: i64,
    #[unique]
    renamed: String,
}

async fn build(driver: SurrealDb) -> Db {
    Db::builder()
        .models(toasty::models!(MigrationItem))
        .build(driver)
        .await
        .expect("build migration test database")
}

async fn applied_ids(driver: &SurrealDb) -> Vec<u64> {
    let mut connection = connect_with_retry(driver).await;
    let mut ids = connection
        .applied_migrations()
        .await
        .expect("read applied migrations")
        .into_iter()
        .map(|migration| migration.id())
        .collect::<Vec<_>>();
    ids.sort_unstable();
    ids
}

async fn connect_with_retry(driver: &SurrealDb) -> Box<dyn toasty_core::driver::Connection> {
    for _ in 0..50 {
        match driver.connect(&ConnectContext::default()).await {
            Ok(connection) => return connection,
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(100)).await,
        }
    }
    panic!("failed to connect to migration test database after retries");
}

#[tokio::test]
async fn generated_migration_applies_and_tracks_full_u64_ids() {
    let driver = SurrealDb::mem();
    let mut db = build(driver.clone()).await;
    let previous = db::Schema::default();
    let hints = diff::RenameHints::new();
    let schema_diff = diff::Schema::from(&previous, &db.schema().db, &hints);
    let migration = driver.generate_migration(&schema_diff);

    let mut connection = driver
        .connect(&ConnectContext::default())
        .await
        .expect("connect to apply generated migration");
    connection
        .apply_migration(7, "generated", &migration)
        .await
        .expect("apply generated migration");
    connection
        .apply_migration(
            u64::MAX,
            "full-u64",
            &db::Migration::new_sql("RETURN NONE".to_string()),
        )
        .await
        .expect("apply full-u64 tracking migration");

    let duplicate = connection
        .apply_migration(
            u64::MAX,
            "duplicate",
            &db::Migration::new_sql("RETURN NONE".to_string()),
        )
        .await
        .expect_err("duplicate migration ID must fail");
    assert!(duplicate.is_condition_failed());
    drop(connection);

    assert_eq!(applied_ids(&driver).await, vec![7, u64::MAX]);

    toasty::create!(MigrationItem {
        id: 1,
        value: "unique",
    })
    .exec(&mut db)
    .await
    .expect("generated table accepts writes");
    let item = MigrationItem::get_by_id(&mut db, 1)
        .await
        .expect("generated table supports reads");
    assert_eq!(item.value, "unique");

    let duplicate_value = toasty::create!(MigrationItem {
        id: 2,
        value: "unique",
    })
    .exec(&mut db)
    .await
    .expect_err("generated unique index must be enforced");
    assert!(
        duplicate_value.is_condition_failed() || duplicate_value.is_driver_operation_failed(),
        "generated unique index must reject duplicate values"
    );
}

#[tokio::test]
async fn toasty_migration_set_applies_once_and_skips_recorded_ids() {
    let driver = SurrealDb::mem();
    let db = build(driver.clone()).await;

    let first = EMBEDDED_MIGRATIONS
        .apply(&db)
        .await
        .expect("apply Toasty migration set");
    assert_eq!(first.applied(), 1);
    assert_eq!(first.skipped(), 0);

    let repeated = EMBEDDED_MIGRATIONS
        .apply(&db)
        .await
        .expect("repeat Toasty migration set");
    assert_eq!(repeated.applied(), 0);
    assert_eq!(repeated.skipped(), 1);
    assert_eq!(applied_ids(&driver).await, vec![50]);
}

#[tokio::test]
async fn generated_column_rename_moves_data_and_rebuilds_index() {
    let driver = SurrealDb::mem();
    let mut previous_db = build(driver.clone()).await;
    let previous_schema = previous_db.schema().db.clone();
    let empty = db::Schema::default();
    let initial_hints = diff::RenameHints::new();
    let initial_diff = diff::Schema::from(&empty, &previous_schema, &initial_hints);

    let mut connection = driver
        .connect(&ConnectContext::default())
        .await
        .expect("connect for rename migration test");
    connection
        .apply_migration(40, "initial", &driver.generate_migration(&initial_diff))
        .await
        .expect("apply initial schema migration");
    toasty::create!(MigrationItem {
        id: 1,
        value: "before-rename",
    })
    .exec(&mut previous_db)
    .await
    .expect("insert data before column rename");

    let mut next_db = Db::builder()
        .models(toasty::models!(RenamedMigrationItem))
        .build(driver.clone())
        .await
        .expect("build renamed model database");
    let next_schema = next_db.schema().db.clone();
    let mut hints = diff::RenameHints::new();
    hints.add_column_hint(
        previous_schema.tables[0].columns[1].id,
        next_schema.tables[0].columns[1].id,
    );
    hints.add_index_hint(
        previous_schema.tables[0].indices[1].id,
        next_schema.tables[0].indices[1].id,
    );
    let rename_diff = diff::Schema::from(&previous_schema, &next_schema, &hints);
    let rename_migration = driver.generate_migration(&rename_diff);
    assert!(
        rename_migration
            .statements()
            .iter()
            .any(|statement| statement.starts_with("DEFINE INDEX")
                && statement.contains("`renamed`")
                && statement.ends_with(" UNIQUE")),
        "column rename must rebuild the unique index on its new field name"
    );
    connection
        .apply_migration(41, "rename-column", &rename_migration)
        .await
        .expect("apply generated column rename");
    drop(connection);

    let renamed = RenamedMigrationItem::get_by_id(&mut next_db, 1)
        .await
        .expect("renamed model reads migrated data");
    assert_eq!(renamed.renamed, "before-rename");

    let duplicate = toasty::create!(RenamedMigrationItem {
        id: 2,
        renamed: "before-rename",
    })
    .exec(&mut next_db)
    .await;
    let Err(duplicate) = duplicate else {
        panic!("rebuilt unique index must reject duplicate renamed values");
    };
    assert!(duplicate.is_condition_failed() || duplicate.is_driver_operation_failed());

    assert_eq!(applied_ids(&driver).await, vec![40, 41]);
}

#[tokio::test]
async fn failed_migration_rolls_back_schema_data_and_tracking() {
    let driver = SurrealDb::mem();
    let mut connection = driver
        .connect(&ConnectContext::default())
        .await
        .expect("connect for rollback test");
    let migration = db::Migration::new_sql_with_breakpoints(&[
        "DEFINE TABLE rollback_items SCHEMALESS",
        "CREATE rollback_items:1 SET payload = 'secret-migration-payload' RETURN NONE",
        "THROW 'expected failure'",
    ]);

    let error = connection
        .apply_migration(10, "must-rollback", &migration)
        .await
        .expect_err("migration must fail");
    assert!(error.is_driver_operation_failed());
    assert!(!error.to_string().contains("secret-migration-payload"));
    assert!(connection.applied_migrations().await.unwrap().is_empty());

    // DEFINE without IF NOT EXISTS proves the failed transaction did not leave
    // either the table definition or its data behind.
    connection
        .apply_migration(
            11,
            "after-rollback",
            &db::Migration::new_sql("DEFINE TABLE rollback_items SCHEMALESS".to_string()),
        )
        .await
        .expect("connection remains reusable after migration rollback");
    assert_eq!(applied_ids(&driver).await, vec![11]);

    let manual = connection
        .apply_migration(
            12,
            "manual",
            &db::Migration::new_sql(
                "THROW 'manual migration required: type conversion'".to_string(),
            ),
        )
        .await
        .expect_err("unedited manual guard must fail");
    assert!(manual.is_unsupported_feature());
    assert_eq!(applied_ids(&driver).await, vec![11]);
}

#[tokio::test]
async fn migration_methods_reject_an_active_user_transaction() {
    let driver = SurrealDb::mem();
    let db = build(driver.clone()).await;
    let schema = Arc::clone(db.schema());
    let mut connection = driver
        .connect(&ConnectContext::default())
        .await
        .expect("connect for active transaction test");

    connection
        .exec(&schema, Transaction::start().into())
        .await
        .expect("start user transaction");
    let query_error = match connection.applied_migrations().await {
        Ok(_) => panic!("tracking query must reject active user transaction"),
        Err(error) => error,
    };
    assert!(query_error.is_invalid_statement());
    let apply_error = connection
        .apply_migration(
            20,
            "nested",
            &db::Migration::new_sql("RETURN NONE".to_string()),
        )
        .await
        .expect_err("migration apply must reject active user transaction");
    assert!(apply_error.is_invalid_statement());
    connection
        .exec(&schema, Transaction::Rollback.into())
        .await
        .expect("roll back user transaction");
}

#[tokio::test]
async fn malformed_tracking_ids_return_sanitized_serialization_errors() {
    let driver = SurrealDb::mem();
    let mut connection = driver
        .connect(&ConnectContext::default())
        .await
        .expect("connect for malformed tracking test");
    connection
        .apply_migration(
            60,
            "inject-malformed-tracking-row",
            &db::Migration::new_sql(
                "CREATE type::record('__toasty_migrations', 'not-a-u64') \
                 SET name = 'malformed', applied_at = time::now() RETURN NONE"
                    .to_string(),
            ),
        )
        .await
        .expect("create malformed tracking fixture");

    let error = match connection.applied_migrations().await {
        Ok(_) => panic!("malformed tracking ID must not be accepted"),
        Err(error) => error,
    };
    assert!(error.is_serialization_failure());
    assert!(!error.to_string().contains("not-a-u64"));
}

#[tokio::test]
async fn surrealkv_tracking_persists_across_reopen() {
    let path = format!(
        "{}/.e2e-data/migrations-surrealkv-persistence",
        env!("CARGO_MANIFEST_DIR")
    );
    let driver = SurrealDb::surrealkv(&path);
    driver
        .reset_db()
        .await
        .expect("reset migration persistence database");
    let mut connection = driver
        .connect(&ConnectContext::default())
        .await
        .expect("open first SurrealKV migration connection");
    connection
        .apply_migration(
            30,
            "persistent",
            &db::Migration::new_sql(
                "DEFINE TABLE persistent_migration_items SCHEMALESS".to_string(),
            ),
        )
        .await
        .expect("apply persistent migration");
    drop(connection);
    drop(driver);

    let reopened = SurrealDb::surrealkv(&path);
    assert_eq!(applied_ids(&reopened).await, vec![30]);

    let mut connection = reopened
        .connect(&ConnectContext::default())
        .await
        .expect("open reopened SurrealKV connection");
    connection
        .apply_migration(
            31,
            "schema-persisted",
            &db::Migration::new_sql("REMOVE TABLE persistent_migration_items".to_string()),
        )
        .await
        .expect("persisted table definition remains removable after reopen");
    assert_eq!(applied_ids(&reopened).await, vec![30, 31]);
}
