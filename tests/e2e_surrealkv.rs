//! End-to-end tests against the embedded SurrealKV engine.
//!
//! Each test uses a unique directory under `.e2e-data/` (git-ignored). Run
//! serially so reopen tests do not race datastore shutdown:
//!
//! ```sh
//! cargo test --test e2e_surrealkv -- --test-threads=1
//! ```

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

async fn fresh_driver(slug: &str) -> SurrealDb {
    let path = format!("{}/.e2e-data/surrealkv-{slug}", env!("CARGO_MANIFEST_DIR"));
    let driver = SurrealDb::surrealkv(&path);
    driver
        .reset_db()
        .await
        .expect("reset SurrealKV data directory");
    driver
}

#[tokio::test]
async fn surrealkv_crud_round_trip() {
    let driver = fresh_driver("crud").await;
    let mut db = Db::builder()
        .models(toasty::models!(User))
        .build(driver)
        .await
        .unwrap();
    db.push_schema().await.unwrap();

    let created = toasty::create!(User {
        id: 1,
        name: "Alice",
        age: 30,
    })
    .exec(&mut db)
    .await
    .unwrap();
    assert_eq!(created.id, 1);

    let mut got = User::get_by_id(&mut db, 1).await.expect("row exists");
    assert_eq!(got.name, "Alice");
    got.update().name("Alicia").exec(&mut db).await.unwrap();

    let updated = User::get_by_id(&mut db, 1).await.unwrap();
    assert_eq!(updated.name, "Alicia");
    updated.delete().exec(&mut db).await.unwrap();
    assert!(User::get_by_id(&mut db, 1).await.is_err());
}

#[tokio::test]
async fn surrealkv_filter_and_scan() {
    let driver = fresh_driver("query").await;
    let mut db = Db::builder()
        .models(toasty::models!(User))
        .build(driver)
        .await
        .unwrap();
    db.push_schema().await.unwrap();

    for id in 1..=5 {
        toasty::create!(User {
            id,
            name: format!("user-{id}"),
            age: 20 + id,
        })
        .exec(&mut db)
        .await
        .unwrap();
    }

    let over_22: Vec<User> = User::filter(User::fields().age().gt(22))
        .exec(&mut db)
        .await
        .unwrap();
    assert_eq!(over_22.len(), 3);
}

#[tokio::test]
async fn surrealkv_persists_across_reopen() {
    let path = format!("{}/.e2e-data/surrealkv-persist", env!("CARGO_MANIFEST_DIR"));

    {
        let driver = SurrealDb::surrealkv(&path);
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

    let mut db = open_with_retry(&path).await;
    let got = User::get_by_id(&mut db, 7).await.expect("row persisted");
    assert_eq!(got.name, "Persisted");
}

async fn open_with_retry(path: &str) -> Db {
    let mut last_err = None;
    for _ in 0..50 {
        match Db::builder()
            .models(toasty::models!(User))
            .build(SurrealDb::surrealkv(path))
            .await
        {
            Ok(db) => return db,
            Err(error) => {
                last_err = Some(error);
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
    panic!("failed to reopen SurrealKV after retries: {last_err:?}");
}
