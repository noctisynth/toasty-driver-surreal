# SurrealDB 3.2.4 远程协议 Spike

日期：2026-09-11

## 证据

`surrealdb 3.2.4` 提供：

- `surrealdb::engine::remote::http::{Client, Http, Https}`，feature `protocol-http`；
- `surrealdb::engine::remote::ws::{Client, Ws, Wss}`，feature `protocol-ws`；
- 使用 `Surreal::<Client>::init()` 后调用 `connect::<Http/Ws>(endpoint).await`；
- remote client 与 local `Surreal<local::Db>` 为不同 Rust 类型；
- remote `Surreal<Client>` 的 query 返回类型可复用，但 transaction 类型与 local 不同。

## 结论

现有 `Connection` 将 `Surreal<local::Db>` 和 `Transaction<local::Db>` 固化在公共 driver 内部，不能通过增加构造器直接支持远程。实现需要抽象 query backend，并将事务能力分派到 backend；远程事务必须单独验证，不能假设与本地一致。

本 spike 未启用 `kv-surrealkv` 或 `kv-rocksdb`。
