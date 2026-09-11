#![cfg(any(feature = "remote-http", feature = "remote-ws"))]

use toasty_core::driver::{ConnectContext, Driver};
use toasty_driver_surreal::SurrealDb;

#[tokio::test]
#[cfg(feature = "remote-http")]
async fn http_remote_query_round_trip() {
    let db = SurrealDb::http("http://127.0.0.1:18000");
    let result = db.connect(&ConnectContext::default()).await;
    assert!(result.is_ok(), "HTTP connection failed: {result:?}");
}

#[tokio::test]
#[cfg(feature = "remote-ws")]
async fn ws_remote_connects() {
    let db = SurrealDb::ws("ws://127.0.0.1:18000/rpc");
    let result = db.connect(&ConnectContext::default()).await;
    assert!(result.is_ok(), "WS connection failed: {result:?}");
}
