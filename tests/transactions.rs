//! Driver-owned explicit transaction coverage.
//!
//! Toasty's shared interactive transaction suite is gated on SQL capability,
//! while this driver intentionally remains a key-value/document backend. These
//! tests exercise the same lifecycle through the public Toasty API and both
//! default embedded engines.

use std::sync::Arc;

use toasty::Db;
use toasty_core::driver::operation::{IsolationLevel, Transaction, TransactionMode};
use toasty_core::driver::{ConnectContext, Driver};
use toasty_driver_surreal::SurrealDb;

#[derive(Debug, toasty::Model)]
struct Item {
    #[key]
    id: i64,
    value: String,
}

async fn open(driver: SurrealDb) -> Db {
    let db = Db::builder()
        .models(toasty::models!(Item))
        .build(driver)
        .await
        .expect("build transaction test database");
    db.push_schema()
        .await
        .expect("push transaction test schema");
    db
}

async fn exercise_core_lifecycle(driver: SurrealDb) {
    driver
        .reset_db()
        .await
        .expect("reset transaction test database");
    let mut db = open(driver).await;

    let mut tx = db.transaction().await.expect("start commit transaction");
    toasty::create!(Item {
        id: 1,
        value: "first",
    })
    .exec(&mut tx)
    .await
    .expect("create first item in transaction");
    toasty::create!(Item {
        id: 2,
        value: "second",
    })
    .exec(&mut tx)
    .await
    .expect("create second item in transaction");
    Item::upsert_by_id(5)
        .value("upserted")
        .exec(&mut tx)
        .await
        .expect("upsert item in transaction");

    let in_transaction = Item::get_by_id(&mut tx, 1)
        .await
        .expect("transaction reads its own write");
    assert_eq!(in_transaction.value, "first");
    tx.commit().await.expect("commit transaction");

    let committed: Vec<Item> = Item::all()
        .exec(&mut db)
        .await
        .expect("scan committed items");
    assert_eq!(committed.len(), 3);

    let mut tx = db.transaction().await.expect("start rollback transaction");
    let mut first = Item::get_by_id(&mut tx, 1)
        .await
        .expect("load item for transactional update");
    first
        .update()
        .value("changed")
        .exec(&mut tx)
        .await
        .expect("update item in transaction");
    let second = Item::get_by_id(&mut tx, 2)
        .await
        .expect("load item for transactional delete");
    second
        .delete()
        .exec(&mut tx)
        .await
        .expect("delete item in transaction");
    toasty::create!(Item {
        id: 3,
        value: "rolled-back",
    })
    .exec(&mut tx)
    .await
    .expect("create item before rollback");
    tx.rollback().await.expect("roll back transaction");
    assert!(Item::get_by_id(&mut db, 3).await.is_err());
    assert_eq!(
        Item::get_by_id(&mut db, 1)
            .await
            .expect("rolled-back update preserves item")
            .value,
        "first"
    );
    assert!(Item::get_by_id(&mut db, 2).await.is_ok());

    {
        let mut tx = db.transaction().await.expect("start dropped transaction");
        toasty::create!(Item {
            id: 4,
            value: "dropped",
        })
        .exec(&mut tx)
        .await
        .expect("create item before dropping transaction");
    }
    assert!(Item::get_by_id(&mut db, 4).await.is_err());

    let tx = db
        .transaction()
        .await
        .expect("connection remains reusable after automatic rollback");
    tx.rollback()
        .await
        .expect("roll back connection reuse transaction");
}

#[tokio::test]
async fn mem_explicit_transaction_core_lifecycle() {
    exercise_core_lifecycle(SurrealDb::mem()).await;
}

#[tokio::test]
async fn surrealkv_explicit_transaction_core_lifecycle() {
    let path = format!(
        "{}/.e2e-data/transactions-surrealkv-core",
        env!("CARGO_MANIFEST_DIR")
    );
    exercise_core_lifecycle(SurrealDb::surrealkv(path)).await;
}

#[tokio::test]
async fn read_only_transaction_rejects_writes_and_can_rollback() {
    let mut db = open(SurrealDb::mem()).await;
    let mut tx = db
        .transaction_builder()
        .read_only(true)
        .begin()
        .await
        .expect("start read-only transaction");

    let error = toasty::create!(Item {
        id: 10,
        value: "forbidden",
    })
    .exec(&mut tx)
    .await
    .expect_err("read-only transaction must reject insert");
    assert!(error.is_read_only_transaction());

    let items: Vec<Item> = Item::all()
        .exec(&mut tx)
        .await
        .expect("reads remain available in read-only transaction");
    assert!(items.is_empty());
    tx.rollback()
        .await
        .expect("read-only transaction remains finalizable");

    toasty::create!(Item {
        id: 11,
        value: "allowed",
    })
    .exec(&mut db)
    .await
    .expect("connection clears read-only state after rollback");
}

#[tokio::test]
async fn unsupported_options_and_savepoints_are_structured() {
    let driver = SurrealDb::mem();
    let db = open(driver.clone()).await;
    let schema = Arc::clone(db.schema());
    let mut connection = driver
        .connect(&ConnectContext::default())
        .await
        .expect("open direct driver connection");

    let error = connection
        .exec(
            &schema,
            Transaction::Start {
                isolation: Some(IsolationLevel::Serializable),
                read_only: false,
                mode: TransactionMode::Default,
            }
            .into(),
        )
        .await
        .expect_err("explicit isolation must be rejected");
    assert!(error.is_unsupported_feature());

    for mode in [
        TransactionMode::Deferred,
        TransactionMode::Immediate,
        TransactionMode::Exclusive,
    ] {
        let error = connection
            .exec(
                &schema,
                Transaction::Start {
                    isolation: None,
                    read_only: false,
                    mode,
                }
                .into(),
            )
            .await
            .expect_err("non-default transaction mode must be rejected");
        assert!(error.is_unsupported_feature());
    }

    let error = connection
        .exec(&schema, Transaction::Commit.into())
        .await
        .expect_err("commit without an active transaction must fail");
    assert!(error.is_invalid_statement());
    let error = connection
        .exec(&schema, Transaction::Rollback.into())
        .await
        .expect_err("rollback without an active transaction must fail");
    assert!(error.is_invalid_statement());

    connection
        .exec(&schema, Transaction::start().into())
        .await
        .expect("start default transaction");
    let error = connection
        .exec(&schema, Transaction::start().into())
        .await
        .expect_err("duplicate transaction start must fail");
    assert!(error.is_invalid_statement());

    for operation in [
        Transaction::Savepoint("sp".to_string()),
        Transaction::ReleaseSavepoint("sp".to_string()),
        Transaction::RollbackToSavepoint("sp".to_string()),
    ] {
        let error = connection
            .exec(&schema, operation.into())
            .await
            .expect_err("savepoint operation must be rejected");
        assert!(error.is_unsupported_feature());
    }

    connection
        .exec(&schema, Transaction::Rollback.into())
        .await
        .expect("savepoint errors preserve the active top-level transaction");
}

#[tokio::test]
async fn surrealkv_write_conflict_is_serialization_failure() {
    let path = format!(
        "{}/.e2e-data/transactions-surrealkv-conflict",
        env!("CARGO_MANIFEST_DIR")
    );
    let driver = SurrealDb::surrealkv(path);
    driver.reset_db().await.expect("reset conflict database");

    let mut first_db = open(driver.clone()).await;
    let mut second_db = open(driver).await;
    toasty::create!(Item {
        id: 20,
        value: "initial",
    })
    .exec(&mut first_db)
    .await
    .expect("create conflict seed row");

    let mut first_tx = first_db
        .transaction()
        .await
        .expect("start first transaction");
    let mut second_tx = second_db
        .transaction()
        .await
        .expect("start second transaction");

    let mut first_item = Item::get_by_id(&mut first_tx, 20)
        .await
        .expect("first transaction reads seed row");
    let mut second_item = Item::get_by_id(&mut second_tx, 20)
        .await
        .expect("second transaction reads seed row");
    first_item
        .update()
        .value("first")
        .exec(&mut first_tx)
        .await
        .expect("first transaction updates row");
    second_item
        .update()
        .value("second")
        .exec(&mut second_tx)
        .await
        .expect("second transaction stages conflicting update");

    first_tx.commit().await.expect("commit first transaction");
    let error = second_tx
        .commit()
        .await
        .expect_err("second transaction must observe a write conflict");
    assert!(
        error.is_serialization_failure(),
        "unexpected error: {error}"
    );
}
