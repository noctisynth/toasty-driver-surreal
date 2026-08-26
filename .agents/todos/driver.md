# SurrealDB Driver 实施清单

> 状态：Active
> 最后核对：2026-08-26
> 需求来源：[Driver Spec](../specs/driver.md)
> 决策来源：[RFC 0001](../rfcs/0001-surrealdb-driver.md)

本清单记录 Active Spec 与当前实现之间的差异。

## P0：验证门禁

- [x] 完成 SurrealDB SDK Spike，记录版本、`type::record` 事实、值往返与 CRUD 映射证据。
- [x] 确认 `surrealdb 3.2.4` 与 `toasty-core/toasty/integration-suite 0.10` 可共存解析。

## P1：crate 与依赖

- [ ] 用 Cargo CLI 创建 `toasty-driver-surreal` crate（lib）。
- [ ] `cargo add` 生产依赖：`toasty-core`、`surrealdb`（kv-mem, kv-rocksdb）、`async-trait`、
  `tracing`。
- [ ] `cargo add --dev`：`toasty`、`toasty-driver-integration-suite`、`tokio`、`uuid`。
- [ ] 检查 manifest 与 lockfile 符合 Spec 依赖边界。

## P2：Driver / Connection 骨架

- [ ] `SurrealDb`：`mem()`、`rocksdb(path)`、`namespace()`、`database()` 构造器。
- [ ] 共享 `Surreal<Db>` 句柄缓存（Arc<Mutex<Option<..>>>），`connect()` 复用。
- [ ] `Driver`：`url`、`capability`（FRU 基线 DYNAMODB）、`connect`、`max_connections`、
  `reset_db`、`generate_migration`（unimplemented）。
- [ ] `capability()` 通过 `Capability::validate()`。

## P3：值编解码

- [ ] `stmt::Value → surrealdb::types::Value`（写入/绑定）。
- [ ] `surrealdb::types::Value + stmt::Type → stmt::Value`（定向解码）。
- [ ] 记录 ID ↔ 主键映射（单列 i64/String/Uuid、复合 Array）。
- [ ] 值 round-trip 单元测试。

## P4：Operation 翻译

- [ ] `Insert`（含冲突分类、RETURN AFTER）。
- [ ] `GetByKey`（多键、未命中空）。
- [ ] `QueryPk`（过滤 + 排序 + 分页 + 索引路径）。
- [ ] `FindPkByIndex`。
- [ ] `Scan`。
- [ ] `UpdateByKey`（SET/Add/Subtract、filter、condition、returning）。
- [ ] `DeleteByKey`（filter、condition、returning）。
- [ ] `Upsert`（native UPSERT）。
- [ ] `surreal_expression()` 过滤翻译器（带 `_` 通配臂 → unsupported_feature）。
- [ ] `push_schema`（DEFINE TABLE + DEFINE INDEX）。
- [ ] `applied_migrations`/`apply_migration` 的首阶段实现选择（记录方案）。

## P5：测试

- [ ] `Setup` 实现 + `generate_driver_tests!`（kv-mem），逐位调能力至绿。
- [ ] 嵌入式 RocksDB e2e（`.e2e-data/`，gitignore）。
- [ ] 记录哪些共享用例因能力位关闭而跳过，并说明原因。

## P6：质量门禁与文档

- [ ] `cargo fmt --check`、`clippy -D warnings`、`test`、`doc`。
- [ ] README（使用方式、支持范围、未支持项）。
- [ ] 交付报告：产出、问题、技术方案变更。

## 后续范围（不在首阶段）

远程引擎、事务、图边、live query、迁移生成、URL scheme 注册、裸 SurrealQL。需各自更新 Spec
后再实施。
