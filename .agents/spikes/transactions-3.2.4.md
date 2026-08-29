# SurrealDB 3.2.4 客户端事务 Spike

> 状态：Completed
> 日期：2026-08-30
> SDK：`surrealdb = 3.2.4`，features `kv-mem`、`kv-surrealkv`
> Toasty：`toasty = 0.10.0`

## 验证问题

1. SDK 客户端事务是否支持独立 query 的 commit/cancel 与 read-your-writes；
2. kv-surrealkv 提交后是否跨重开持久化；
3. SDK 是否提供 savepoint；
4. Toasty 的显式事务与普通 batch 是否都受 `Capability::sql()` 门控；
5. 事务冲突是否有可供 driver 分类的结构化错误。

## SDK 探针

在 `/private/tmp` 创建一次性 Rust binary，使用 `surrealdb 3.2.4` 的 `kv-surrealkv` 引擎：打开
数据库、选择 namespace/database，以 `db.begin()` 创建两个客户端事务，分别验证事务句柄 query、
commit/cancel、会话外不可见未提交写入，并在重开数据库后读取已提交记录。探针同时尝试在事务
query 中执行 savepoint 语句。

实际输出：

```text
surrealkv-open=ok transaction-commit=ok transaction-cancel=ok isolation=ok savepoint=unsupported
surrealkv-reopen-persistence=ok
```

SDK 源码确认 `method::Transaction<Db>` 持有事务 UUID 和 `Surreal<Db>` 客户端；`query()` 自动附加
该事务 ID，`commit(self)` / `cancel(self)` 消费句柄。`surrealdb-types 3.2.4` 的公共
`Error::query_details()` 暴露 `QueryError::TransactionConflict`；SurrealKV 的
`TransactionWriteConflict` 在 query 执行路径会转换为该类型。但嵌入式 commit 走
`engine/local/mod.rs` 的 `std_error_to_types_error` 后，实测返回 `details: Internal, cause: None`，只保留
稳定文本 `Transaction conflict: …`。SurrealDB core 3.2.4 自身测试也对这一已知差异使用
`contains("Transaction conflict:")`，driver 因此在类型/错误链检查之后使用同一兼容回退。

## Toasty 路由检查

`toasty 0.10.0/src/db/tx.rs` 显示 `Db::transaction()` 与 `transaction_builder().begin()` 直接向连接
发送 `Operation::Transaction::Start`，commit/rollback 同理。嵌套事务发送 savepoint Operation。

普通 batch 的自动事务包装则在查询引擎中使用 `use_transactions: self.capability().sql()`；因此
KV driver 保持 `Capability.sql = None` 时，显式事务可用，但普通 batch 不会自动获得事务原子性。
共享 `tx_interactive` 用例当前标记 `requires(sql)`，无法验证本 driver，需要专用测试。

## 结论

- 顶层 Start/Commit/Rollback 可以直接映射到 SDK 客户端事务；
- 所有事务内 query 必须通过事务句柄，不能继续使用普通 `Surreal<Db>` query；
- 只读、isolation 与 Toasty lock mode 没有 SDK begin 参数，需要 driver 限制或拒绝；
- SDK 3.2.4 不支持 savepoint，嵌套事务必须明确拒绝；
- 冲突可按 `QueryError::TransactionConflict` 稳定分类，不应匹配错误字符串；
- 该能力不要求也不允许把 driver 改为 SQL driver。
