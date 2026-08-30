# SurrealDB 3.2.4 迁移事务 Spike

> 状态：Completed
> 日期：2026-08-30
> SDK：`surrealdb = 3.2.4`，feature `kv-mem`
> Toasty：`toasty-core/toasty = 0.10.0`

## 验证问题

1. Toasty migration generation 与 apply 的 driver 边界是什么；
2. SurrealDB DDL、索引、tracking 写入能否共用 SDK 客户端事务；
3. migration 中途失败后 DDL 是否回滚；
4. 无物理 schema 变化时的 `RETURN NONE` 是否可执行；
5. Toasty 完整 `u64` migration ID 能否无损存入 SurrealDB record id。

## Toasty 契约检查

`toasty 0.10.0/src/migration/generate.rs` 先创建 `diff::Schema`，再调用
`Driver::generate_migration(&diff)`。返回类型不是 `Result`。`migration/embed.rs` 查询
`Connection::applied_migrations()`，以 ID 集合跳过已执行文件，并逐个调用
`Connection::apply_migration(id, name, Migration::new_sql(contents))`。

`toasty-core 0.10.0` 的 `Migration` 当前只有 `Sql(String)`，用
`-- #[toasty::breakpoint]` 分隔 statement；`AppliedMigration` 当前只保存 `u64 id`，不提供 checksum
或 name 校验契约。

## SDK 探针

在 `/private/tmp/toasty-surreal-migration-spike-20260830` 创建一次性 Rust binary，以 kv-mem 打开
数据库并执行：

- 在一个 `db.begin()` 客户端事务中 DEFINE tracking table、DEFINE 业务表、DEFINE UNIQUE INDEX、
  执行 `RETURN NONE`、CREATE tracking record，然后 commit；
- tracking record 使用 `u64::MAX.to_string()` 作为 record id，随后以 `record::id(id)` 读回；
- 新事务先 DEFINE 另一张表，再执行 `THROW`，检查 statement error 后 cancel；
- 通过 `INFO FOR DB` 确认第一组 DDL 已提交、失败事务中的表不存在。

实际输出：

```text
tracking=Array(Array([Object(Object({"migration_id": String("18446744073709551615")}))]))
ddl-commit=ok ddl-rollback=ok noop=ok u64-tracking=ok
```

SurrealDB core 3.2.4 的 index tests 还覆盖了 DEFINE/REMOVE INDEX 在事务 cancel 后恢复 catalog 的
行为，和探针结论一致。

## 实现后索引时序探针

driver-owned kv-mem 测试对带 UNIQUE index 的非主键字段执行 rename migration。最初的
REMOVE index → UPDATE 搬值 → DEFINE index 顺序虽然全部 statement 成功，但随后插入重复值也会
成功：SDK 3.2.4 的 index builder 看不到同一事务里先前写入、尚未提交的新字段值。

调整为 REMOVE 旧 index → DEFINE 新字段 index → UPDATE 搬值/UNSET 后，原记录可由新字段模型读取，
后续重复值被 UNIQUE index 拒绝。最终 generation 固定采用后一顺序，并由
`tests/migrations.rs::generated_column_rename_moves_data_and_rebuilds_index` 回归。

## 结论

- 迁移语句与 tracking CREATE 可以共用 SDK 客户端事务，达到同成同败；
- 每条 query 必须同时检查 transport error 与 statement-level `response.check()`；
- `THROW` 可作为 generation trait 无 `Result` 时的安全人工迁移门禁；
- `RETURN NONE` 是合法 no-op，可承载只有 SCHEMALESS 元数据变化的 migration；
- migration ID 应以十进制 String record key 保存，不能窄化为 `i64`；
- 字段搬迁时 replacement index 必须先于 DML 定义，让同事务 UPDATE 维护索引；
- 该能力不需要 SQL capability 或 `toasty-sql`。
