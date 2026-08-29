# RFC 0003：显式顶层事务

> 状态：Accepted
> 接受日期：2026-08-30
> 设计入口：[设计索引](../DESIGN.md)
> 落地规范：[Driver Active Spec](../specs/driver.md)
> 验证证据：[事务 Spike](../spikes/transactions-3.2.4.md)

## 1. 问题

首阶段把 `Operation::Transaction` 整体列为非目标，依据是 Toasty 的 KV 批处理路径受
`Capability::sql()` 门控。审计后确认该判断只适用于普通 `toasty::batch` 的自动事务包装；用户显式
调用 `Db::transaction()` / `transaction_builder()` 时，Toasty 0.10 会直接向同一连接下发
`Transaction::Start/Commit/Rollback`，不受 `sql()` 门控。

`surrealdb 3.2.4` 已提供客户端事务句柄：`Surreal::begin()` 返回带事务 ID 的
`method::Transaction<Db>`，句柄上的 `query()` 参与同一事务，`commit()` / `cancel()` 完成顶层事务。
因此 driver 可以保持 KV/文档定位与 `sql = None`，同时补齐用户显式事务。

## 2. 目标与非目标

目标：

- 支持顶层 `Transaction::Start`、`Commit`、`Rollback`；
- 让八个既有 KV Operation 在活动事务中复用同一个 SDK 事务句柄；
- 接受默认事务参数与 `read_only = true`，由 driver 在下发前拒绝写 Operation；
- 把 SurrealDB 的结构化事务冲突映射为 `serialization_failure`；
- 覆盖内存与 SurrealKV 引擎的 commit、rollback、read-your-writes 和连接复用。

非目标：

- 不支持 savepoint、嵌套事务或部分回滚；
- 不支持显式 isolation level，也不把 SurrealDB 当前隔离语义冒充为 Toasty 的某个 SQL 隔离级别；
- 不支持 `TransactionMode::Deferred/Immediate/Exclusive`；
- 不改变 `Capability.sql = None`，不宣称普通 `toasty::batch` 自动具备原子性；
- 不暴露 SurrealDB 事务句柄或事务 ID 到公共 API；
- 不实现自动冲突重试。

## 3. 候选方案

| 方案 | 结论 | 原因 |
|---|---|---|
| 使用 SDK 客户端事务句柄 | 接受 | SDK 原生维持事务 ID；query/commit/cancel 已在 kv-mem 与 kv-surrealkv 实测，且不改变公共 API |
| 在普通连接上拼装 `BEGIN TRANSACTION` SurrealQL | 拒绝 | SDK 的客户端事务协议才保证后续独立 query 绑定同一事务；裸语句无法安全跨 Operation 持有上下文 |
| 把 driver 改为 SQL 能力以开启 Toasty 自动事务 | 拒绝 | 会让查询规划器下发 SQL Operation，破坏既有 KV Operation 架构与 `toasty-sql` 非依赖边界 |
| driver 全局互斥并缓存写操作模拟事务 | 拒绝 | 无法提供可靠回滚、隔离和冲突语义，也会错误串行化不同数据库会话 |

## 4. 已接受设计

`Connection` 保留稳定的 `Surreal<Db>` 句柄，并增加私有的活动事务状态和只读标记：

```rust
transaction: Option<surrealdb::method::Transaction<Db>>,
read_only: bool,
```

`Start` 必须满足 `isolation.is_none()` 与 `mode == TransactionMode::Default`；否则在开始 SDK 事务前
返回 `unsupported_feature`。`read_only` 可以为 `true`，但 SDK 3.2.4 没有对应 begin 选项，因此由
driver 在每个写 Operation（Insert、UpdateByKey、DeleteByKey、Upsert）下发前返回
`read_only_transaction`。重复开始顶层事务同样返回结构化错误，不替换现有句柄。

所有数据 query 通过 `Connection` 的统一执行入口：存在活动事务时调用 `tx.query(...)`，否则调用
`db.query(...)`。绑定参数仍不写入日志。`Commit` / `Rollback` 用 `Option::take()` 取得并消费句柄，
分别调用 `commit()` / `cancel()`；完成或失败后都清除 driver 的活动状态，使同一 Toasty 连接可以
再次使用稳定的 `db` 句柄。重复 Start 或没有活动事务的 finalize 返回 `invalid_statement`。

`Savepoint`、`ReleaseSavepoint`、`RollbackToSavepoint` 一律返回 `unsupported_feature`，且不改变外层
事务状态。Toasty 的嵌套 `Transaction::transaction()` 因而会在创建 savepoint 时立即失败。

## 5. 错误与安全

- 优先遍历公开的 `surrealdb::Error::cause()` 链；任一层的 `query_details()` 为
  `Some(QueryError::TransactionConflict)` 时映射为 `toasty_core::Error::serialization_failure`；嵌入式
  commit 在 SDK 3.2.4 中经 `std_error_to_types_error` 丢失详情并变成无 cause 的 `Internal`，因此兼容
  该版本 core 自身测试采用的 `message.contains("Transaction conflict:")` 回退；
- 只读事务的写入在值编码和 SDK query 前拒绝，不把记录内容或绑定值写入错误；
- 未支持的 isolation、mode 与 savepoint 返回 `unsupported_feature`；
- 其它错误沿用 Driver Spec 的分类；commit/cancel 错误不会遗留一个无法 finalize 的句柄；
- driver 不自动重试事务，调用方根据 `is_serialization_failure()` 决定完整重试边界。

## 6. Toasty 语义边界

用户显式 `db.transaction()` 和 `db.transaction_builder()` 可以使用本能力，因为它们直接下发事务
Operation。Toasty 0.10 的普通 batch 规划仍以 `self.capability().sql()` 决定是否自动包装事务；本
driver 必须保持 `sql = None`，所以文档不得把普通 batch 描述为原子事务。共享事务套件目前以
`requires(sql)` 跳过 KV driver，因此由本仓库提供专用事务集成测试。

## 7. 验收标准

1. kv-mem 与 kv-surrealkv 都通过 commit、rollback 与 read-your-writes 核心闭环；
2. 多个写 Operation 可在同一事务中一起提交，drop 未 finalize 时自动 rollback；
3. 只读事务写入返回 `read_only_transaction`，随后仍可 rollback；
4. isolation、非默认 mode 与三种 savepoint Operation 返回 `unsupported_feature`；
5. 事务内的全部既有数据 Operation 都路由到 SDK 事务 query；
6. 结构化 `TransactionConflict` 分类为 `serialization_failure`，稳定可构造时增加 SurrealKV 冲突 e2e；
7. fmt、check、clippy、默认测试、SurrealKV/RocksDB e2e、rustdoc 与 cargo-deny 门禁通过；
8. 用 Semifold CLI 创建独立 changeset 并确认发布计划。
