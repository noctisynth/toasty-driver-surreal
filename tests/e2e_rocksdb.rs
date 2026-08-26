//! End-to-end tests against the embedded RocksDB engine.
//!
//! Gated behind the `rocksdb` crate feature because it compiles `librocksdb`
//! from source. Run with:
//!
//! ```sh
//! cargo test --test e2e_rocksdb --features rocksdb
//! ```
//!
//! Each test uses a unique directory under `.e2e-data/` (git-ignored) and
//! removes it via `reset_db()` on entry so reruns start clean.
#![cfg(feature = "rocksdb")]

use toasty::Db;
use toasty_core::driver::Driver;
use toasty_driver_surreal::SurrealDb;

#[derive(Debug, toasty::Model)]
struct User {
    #[key]
    id: i64,
    name: String,
    age: i64,
}

/// Returns a RocksDB-backed driver rooted at a unique, cleaned directory.
async fn fresh_driver(slug: &str) -> SurrealDb {
    let path = format!("{}/.e2e-data/{slug}", env!("CARGO_MANIFEST_DIR"));
    let driver = SurrealDb::rocksdb(&path);
    // Wipe any leftover data from a previous run.
    driver.reset_db().await.expect("reset rocksdb data dir");
    driver
}

#[tokio::test]
async fn rocksdb_crud_round_trip() {
    let driver = fresh_driver("crud").await;
    let mut db = Db::builder()
        .models(toasty::models!(User))
        .build(driver)
        .await
        .unwrap();
    db.push_schema().await.unwrap();

    // Create.
    let created = toasty::create!(User {
        id: 1,
        name: "Alice",
        age: 30,
    })
    .exec(&mut db)
    .await
    .unwrap();
    assert_eq!(created.id, 1);

    // Get by id.
    let mut got = User::get_by_id(&mut db, 1).await.expect("row exists");
    assert_eq!(got.name, "Alice");
    assert_eq!(got.age, 30);

    // Update.
    got.update().name("Alicia").exec(&mut db).await.unwrap();
    let updated = User::get_by_id(&mut db, 1).await.unwrap();
    assert_eq!(updated.name, "Alicia");

    // Delete.
    updated.delete().exec(&mut db).await.unwrap();
    let missing = User::get_by_id(&mut db, 1).await;
    assert!(missing.is_err(), "row should be gone after delete");
}

#[tokio::test]
async fn rocksdb_filter_and_scan() {
    let driver = fresh_driver("query").await;
    let mut db = Db::builder()
        .models(toasty::models!(User))
        .build(driver)
        .await
        .unwrap();
    db.push_schema().await.unwrap();

    for i in 1..=5 {
        toasty::create!(User {
            id: i,
            name: format!("user-{i}"),
            age: 20 + i,
        })
        .exec(&mut db)
        .await
        .unwrap();
    }

    // Full-table scan with a filter (no index on age).
    let over_22: Vec<User> = User::filter(User::fields().age().gt(22))
        .exec(&mut db)
        .await
        .unwrap();
    assert_eq!(over_22.len(), 3);

    // Get by primary key.
    let three = User::get_by_id(&mut db, 3).await.unwrap();
    assert_eq!(three.name, "user-3");
}

/// Data written through one `Db` is visible after reopening the same on-disk
/// database, proving durability of the embedded RocksDB engine.
#[tokio::test]
async fn rocksdb_persists_across_reopen() {
    let path = format!("{}/.e2e-data/persist", env!("CARGO_MANIFEST_DIR"));

    // First open: reset, write a row.
    {
        let driver = SurrealDb::rocksdb(&path);
        driver.reset_db().await.unwrap();
        let mut db = Db::builder()
            .models(toasty::models!(User))
            .build(driver)
            .await
            .unwrap();
        db.push_schema().await.unwrap();
        toasty::create!(User {
            id: 7,
            name: "Persisted",
            age: 99,
        })
        .exec(&mut db)
        .await
        .unwrap();
    }

    // Second open: same path, no reset — the row must still be there.
    //
    // The embedded RocksDB datastore releases its file lock when the previous
    // handle's background shutdown completes, which is not synchronous with
    // `Db` drop. Retry the open briefly so the test reflects a real reopen
    // rather than racing that shutdown.
    let mut db = open_with_retry(&path).await;
    let got = User::get_by_id(&mut db, 7).await.expect("row persisted");
    assert_eq!(got.name, "Persisted");
    assert_eq!(got.age, 99);
}

/// Opens the RocksDB database at `path`, retrying while the previous handle's
/// file lock is still being released.
async fn open_with_retry(path: &str) -> Db {
    let mut last_err = None;
    for _ in 0..50 {
        match Db::builder()
            .models(toasty::models!(User))
            .build(SurrealDb::rocksdb(path))
            .await
        {
            Ok(db) => return db,
            Err(e) => {
                last_err = Some(e);
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
    panic!("failed to reopen rocksdb after retries: {last_err:?}");
}
