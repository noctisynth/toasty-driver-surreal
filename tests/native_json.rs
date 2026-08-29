//! Driver-owned native JSON tests.
//!
//! Toasty 0.10's shared native-JSON tests hard-code SQL operation log shapes,
//! so a KV driver needs equivalent behavior tests that exercise its actual
//! Insert/GetByKey/UpdateByKey/Upsert paths.

use serde_json::{Value as JsonValue, json};
use toasty::{Db, Json};
use toasty_core::driver::Driver;
use toasty_driver_surreal::SurrealDb;

#[derive(Debug, toasty::Model)]
struct NativeJsonItem {
    #[key]
    id: i64,

    #[column(type = json)]
    payload: JsonValue,

    #[column(type = json)]
    nullable: Option<JsonValue>,

    #[column(type = json)]
    wrapped_null: Json<Option<String>>,

    #[column(type = json)]
    tags: Json<Vec<String>>,
}

async fn database() -> Db {
    let db = Db::builder()
        .models(toasty::models!(NativeJsonItem))
        .build(SurrealDb::mem())
        .await
        .expect("build native JSON test database");
    db.push_schema()
        .await
        .expect("push native JSON test schema");
    db
}

#[test]
fn capability_reports_json_but_not_jsonb() {
    let driver = SurrealDb::mem();
    assert!(driver.capability().native_json);
    assert!(!driver.capability().native_jsonb);
}

#[tokio::test]
async fn native_json_round_trips_and_preserves_null_semantics() {
    let mut db = database().await;
    let payload = json!({
        "array": [1, true, null, "quoted \"text\""],
        "nested": {"language": "日本語", "present": null},
    });

    let created = toasty::create!(NativeJsonItem {
        id: 1,
        payload: payload.clone(),
        nullable: None,
        wrapped_null: Json(None),
        tags: Json(vec!["rust".to_string(), "surrealdb".to_string()]),
    })
    .exec(&mut db)
    .await
    .expect("insert native JSON item");

    assert_eq!(created.payload, payload);
    assert_eq!(created.nullable, None);
    assert_eq!(created.wrapped_null, Json(None));
    assert_eq!(
        created.tags,
        Json(vec!["rust".to_string(), "surrealdb".to_string()])
    );

    let loaded = NativeJsonItem::get_by_id(&mut db, 1)
        .await
        .expect("read native JSON item");
    assert_eq!(loaded.payload, payload);
    assert_eq!(loaded.nullable, None);
    assert_eq!(loaded.wrapped_null, Json(None));
}

#[tokio::test]
async fn native_json_update_upsert_and_whole_value_filter() {
    let mut db = database().await;
    let initial = json!({"version": 1});
    let mut item = toasty::create!(NativeJsonItem {
        id: 1,
        payload: initial,
        nullable: None,
        wrapped_null: Json(None),
        tags: Json(vec!["initial".to_string()]),
    })
    .exec(&mut db)
    .await
    .expect("insert native JSON item for update");

    let updated = json!([{"version": 2}, "updated", false, null]);
    let optional = json!({"enabled": true});
    item.update()
        .payload(updated.clone())
        .nullable(Some(optional.clone()))
        .tags(vec!["updated".to_string()])
        .exec(&mut db)
        .await
        .expect("update native JSON columns");

    let matches: Vec<NativeJsonItem> =
        NativeJsonItem::filter(NativeJsonItem::fields().payload().eq(updated.clone()))
            .exec(&mut db)
            .await
            .expect("filter by a whole native JSON value");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].nullable, Some(optional));
    assert_eq!(matches[0].tags, Json(vec!["updated".to_string()]));

    item.update()
        .nullable(None)
        .exec(&mut db)
        .await
        .expect("set native JSON column to database null");
    assert_eq!(
        NativeJsonItem::get_by_id(&mut db, 1)
            .await
            .expect("reload database-null native JSON")
            .nullable,
        None
    );

    let upsert_payload = json!({"source": "upsert", "null": null});
    let upserted = NativeJsonItem::upsert_by_id(2)
        .payload(upsert_payload.clone())
        .nullable(None)
        .wrapped_null(Json(Some("value".to_string())))
        .tags(vec!["upsert".to_string()])
        .exec(&mut db)
        .await
        .expect("create through native JSON upsert");
    assert_eq!(upserted.payload, upsert_payload);
    assert_eq!(upserted.nullable, None);
    assert_eq!(upserted.wrapped_null, Json(Some("value".to_string())));

    let replaced_payload = json!(["upsert-update", {"version": 2}]);
    let upserted = NativeJsonItem::upsert_by_id(2)
        .payload(replaced_payload.clone())
        .nullable(Some(JsonValue::Null))
        .wrapped_null(Json(None))
        .tags(vec!["replaced".to_string()])
        .exec(&mut db)
        .await
        .expect("update through native JSON upsert");
    assert_eq!(upserted.payload, replaced_payload);
    assert_eq!(upserted.nullable, Some(JsonValue::Null));
    assert_eq!(upserted.wrapped_null, Json(None));
}
