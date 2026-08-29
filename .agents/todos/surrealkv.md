# SurrealKV 引擎实施清单

> 状态：Implemented
> 最后核对：2026-08-30
> 需求来源：[Driver Spec](../specs/driver.md)
> 决策来源：[RFC 0002](../rfcs/0002-surrealkv-engine.md)

- [x] 用 Cargo CLI 启用 SDK `kv-surrealkv` feature，并检查 manifest/lockfile；
- [x] 增加 `Engine::SurrealKv` 与 `SurrealDb::surrealkv(path)`；
- [x] 实现 session、URL、连接数与 reset 行为；
- [x] 增加 SurrealKV CRUD、过滤扫描、跨重开持久化 e2e；
- [x] 更新 README 与 CI 门禁说明；
- [x] 运行默认及与 RocksDB 组合 feature 的质量门禁；
- [x] 用 Semifold CLI 创建独立 changeset 并确认发布计划。
