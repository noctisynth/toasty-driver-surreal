# SurrealKV 3.2.4 Spike

> 状态：Completed
> 日期：2026-08-30
> SDK：`surrealdb = 3.2.4`，feature `kv-surrealkv`

## 验证问题

1. `kv-surrealkv` 是否可与当前稳定 SDK 编译；
2. 是否仍返回现有 driver 使用的 `Surreal<Db>`；
3. 是否支持文件持久化与跨进程重开；
4. 是否与 SDK 客户端事务共存。

## 探针

在 `/private/tmp` 创建一次性 Rust binary，使用 `Surreal::new::<SurrealKv>(path)` 打开数据库，
选择 namespace/database，执行 schema、写入、事务 commit/cancel、会话隔离，并在第二个进程中重开
同一路径读取已提交记录。

实际输出：

```text
surrealkv-open=ok transaction-commit=ok transaction-cancel=ok isolation=ok savepoint=unsupported
surrealkv-reopen-persistence=ok
```

## 结论

- `surrealdb 3.2.4` 原生暴露 `engine::local::SurrealKv`，连接类型为 `Surreal<Db>`；
- `kv-surrealkv` 在当前 macOS/stable Rust 环境编译并运行通过；
- 数据跨进程重开保持；
- 可直接复用现有 driver 的句柄、值编解码和 Operation 翻译；
- SDK 解析到 `surrealkv 0.21.4`（`0.21.x` 兼容范围），license 为 Apache-2.0。
