//! Focused smoke test used to debug the create → get round-trip against the
//! in-memory engine. Kept as a fast, driver-owned regression check separate
//! from the shared suite.

use toasty::Db;
use toasty_driver_surreal::SurrealDb;

#[derive(Debug, toasty::Model)]
struct User {
    #[key]
    id: i64,
    name: String,
}

#[tokio::test]
async fn create_then_get_by_id() {
    let mut db = Db::builder()
        .models(toasty::models!(User))
        .build(SurrealDb::mem())
        .await
        .unwrap();
    db.push_schema().await.unwrap();

    let created = toasty::create!(User {
        id: 1,
        name: "Alice"
    })
    .exec(&mut db)
    .await
    .unwrap();
    assert_eq!(created.id, 1);

    let got = User::get_by_id(&mut db, 1).await.expect("row should exist");
    assert_eq!(got.name, "Alice");
}
