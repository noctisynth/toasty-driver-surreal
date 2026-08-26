# SurrealDB Driver 实施清单

> 状态：Implemented（首阶段）
> 最后核对：2026-08-26
> 需求来源：[Driver Spec](../specs/driver.md)
> 决策来源：[RFC 0001](../rfcs/0001-surrealdb-driver.md)

本清单记录 Active Spec 与当前实现之间的差异。首阶段实现已完成，质量门禁通过。

## P0：验证门禁

- [x] 完成 SurrealDB SDK Spike，记录版本、`type::record` 事实、值往返与 CRUD 映射证据。
- [x] 确认 `surrealdb 3.2.4` 与 `toasty-core/toasty/integration-suite 0.10` 可共存解析。

## P1：crate 与依赖

- [x] 用 Cargo CLI 创建 `toasty-driver-surreal` crate（lib）。
- [x] `cargo add` 生产依赖：`toasty-core`（`jiff`、`rust_decimal`）、`surrealdb`（默认 kv-mem，
  可选 `rocksdb` feature 引入 kv-rocksdb）、`async-trait`、`tracing`、`tokio`（sync）、
  `rust_decimal`。
- [x] `cargo add --dev`：`toasty`、`toasty-driver-integration-suite`、`tokio`、`uuid`、
  `hashbrown`。
- [x] 检查 manifest 与 lockfile 符合 Spec 依赖边界。

## P2：Driver / Connection 骨架

- [x] `SurrealDb`：`mem()`、`rocksdb(path)`（feature-gated）、`namespace()`、`database()`。
- [x] 共享 `Surreal<Db>` 句柄缓存（`Arc<Mutex<Option<..>>>`），`connect()` 复用并选 ns/db。
- [x] `Driver`：`url`、`capability`（FRU 基线 DYNAMODB）、`connect`、`max_connections`、
  `reset_db`、`generate_migration`（unimplemented）。
- [x] `capability()` 通过 `Capability::validate()`（单元测试覆盖）。

## P3：值编解码

- [x] `stmt::Value → surrealdb::types::Value`（写入/绑定）。
- [x] `surrealdb::types::Value + stmt::Type → stmt::Value`（定向解码）。
- [x] 记录 ID ↔ 主键映射（单列 i64/String/Uuid、复合 Array），按主键 arity 归一。
- [x] 温度/decimal/网络值以 canonical text 编码，读取时 `Type::cast` 还原。
- [x] `u64` 超过 `i64::MAX` 以 Decimal 编码，避免丢失。

## P4：Operation 翻译

- [x] `Insert`（经 `QuerySql(Statement::Insert)` 路由，含 RETURNING 解码、冲突分类）。
- [x] `GetByKey`（多记录 id 一次查询、未命中空）。
- [x] `QueryPk`（过滤 + 排序[别名] + keyset/offset 分页 + 索引字段路径）。
- [x] `FindPkByIndex`。
- [x] `Scan`（keyset 游标可续）。
- [x] `UpdateByKey`（Set/Add/Subtract/Append、filter、condition、returning）。
- [x] `DeleteByKey`（filter、condition、RETURN BEFORE 计数）。
- [x] `Upsert`（native UPSERT SET，create-only 用 `?? `，shared 操作折叠 `#[default]`）。
- [x] `expr::render()` 过滤翻译器：BinaryOp/And/Or/Not/IsNull/Between/InList/AnyOp/
  IsSuperset/Intersects/Length/StartsWith/JsonExtract/Reference/Value/List，带 `_` 通配臂。
- [x] `push_schema`（DEFINE TABLE + DEFINE INDEX，含 UNIQUE）。
- [x] `applied_migrations`/`apply_migration` 首阶段返回 `unsupported_feature`（不 panic）。

## P5：测试

- [x] `Setup` 实现 + `generate_driver_tests!`（kv-mem）。**609/612 共享用例通过。**
- [x] 嵌入式 RocksDB e2e（`.e2e-data/`，gitignore）：CRUD、过滤扫描、重开持久化，3/3 通过。
- [x] 记录 3 个"因 SurrealDB 能力强于 DynamoDB 而无法满足 DynamoDB 负向断言"的用例（见下）。

### 已知分歧（3 个共享用例，非缺陷）

以下用例仅以 `requires(not(sql))` 门控，断言的是 DynamoDB 实现的*限制*；SurrealDB 没有这些
限制，故正确地成功执行，从而与断言相悖。按 Toasty「不隐藏后端差异」的设计哲学，不通过人为
拒绝来迎合，属有意分歧（记录于 `tests/mem.rs` 顶部）：

- `index_composite::composite_index_too_many_range_columns`
- `index_composite::composite_unique_index_unsupported_on_dynamodb`
- `starts_with::starts_with_empty_prefix`

## P6：质量门禁与文档

- [x] `cargo fmt --check`、`cargo clippy --all-targets -D warnings`（默认与 `rocksdb`）、
  `cargo test`、`cargo doc`（`RUSTDOCFLAGS=-D warnings`）全部通过。
- [x] README（使用方式、支持范围、未支持项、已知分歧）。
- [x] 交付报告：产出、问题、技术方案变更（见最终回复）。

## P7：发布与 CI/CD

- [x] Cargo.toml 发布元数据（license `AGPL-3.0-only`、repository、description、keywords、
  categories、readme）+ AGPL-3.0 `LICENSE` 文件；`version` 不手改（由 Semifold CI 管理）。
- [x] `semifold init`（rust resolver，base=main、release=release，stable 通道），
  `semifold config sync --check` + `semifold status` 通过。
- [x] 生成 changeset `initial-surrealdb-driver`（minor/feat）；`semifold status` 规划 0.1.0→0.2.0。
- [x] GitHub Actions：`quality.yaml`（fmt/check/clippy/test/doc + 独立 rocksdb e2e job，
  stable toolchain）、`security.yaml`（cargo-deny + gitleaks）、`semifold-ci.yaml`、
  `semifold-status.yaml`。
- [x] `deny.toml`：AGPL 自例外 + SurrealDB BUSL-1.1 / ext-sort Unlicense 例外（逐条注明）、
  bincode 未维护 advisory ignore；`cargo deny check advisories licenses bans sources` 通过。

## 后续范围（不在首阶段）

远程引擎、事务、图边、live query、迁移生成、URL scheme 注册、裸 SurrealQL。需各自更新 Spec
后再实施。
