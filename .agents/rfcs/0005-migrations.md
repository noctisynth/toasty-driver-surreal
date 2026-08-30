# RFC 0005：迁移跟踪与自动生成

> 状态：Accepted
> 接受日期：2026-08-30
> 设计入口：[设计索引](../DESIGN.md)
> 落地规范：[Driver Active Spec](../specs/driver.md)
> 验证证据：[迁移 Spike](../spikes/migrations-3.2.4.md)

## 1. 问题

Toasty 0.10 的迁移框架已经定义两层 driver 契约：`migration::generate()` 计算数据库 schema diff 后
调用 `Driver::generate_migration()`；`MigrationSet::apply()` 则通过 Connection 的
`applied_migrations()` / `apply_migration()` 查询、执行并记录迁移。本 driver 当前只有
`push_schema()`：两个 tracking 方法返回 `unsupported_feature`，而 generation 使用
`unimplemented!()`，既无法使用 Toasty 的嵌入式迁移流程，也违反生产代码不 panic 的仓库约束。

SurrealDB SDK 不理解 Toasty schema diff；它只负责执行最终生成的 SurrealQL。因此 tracking 表、
diff 翻译、安全降级与事务边界都必须由本 driver 实现。

## 2. 目标与非目标

目标：

- 实现 Toasty migration ID 的查询、原子执行与落库；
- 把可安全表达的 Toasty schema diff 自动翻译成 SurrealQL；
- 复用 SurrealDB 3.2.4 客户端事务，使迁移语句和 tracking 记录同成同败；
- 对无法安全自动转换的数据变化生成可编辑、应用时明确失败的 migration，而不是 panic、静默 no-op
  或破坏记录；
- 保持 `Capability.sql = None`，不新增 `toasty-sql` 依赖。

非目标：

- 不实现 migration checksum、down migration、锁表或跨进程 migration lease；Toasty 0.10 的
  `AppliedMigration` 只暴露 ID；
- 不自动完成表重命名、主键布局变化或字段存储类型转换；这些变化需要用户把生成的 `THROW` 门禁
  替换为经过业务验证的 SurrealQL；
- 不保证任意手写 SurrealQL 可回滚；验收范围是 SDK 3.2.4 已验证的嵌入式引擎与本 driver 生成的
  DDL/DML；
- 不改变 `push_schema` 的开发/测试定位，也不做数据库 schema introspection。

## 3. 候选方案

| 方案 | 结论 | 原因 |
|---|---|---|
| Driver 自管 tracking + SurrealQL migration | 接受 | 完整接入 Toasty 现有契约，不引入 SQL 方言层 |
| 只实现 tracking，generation 继续 panic | 拒绝 | 用户请求包含两项能力，且生产 panic 违反仓库约束 |
| 依赖 SurrealDB 自动比较/生成 migration | 拒绝 | SDK 没有 Toasty schema diff 语义，也无法理解 record-id 主键映射 |
| 所有 diff 都 drop/recreate | 拒绝 | 表/主键/类型变化会静默丢失或错误重编码数据 |
| generation 改为 `Result<Migration>` | 不可用 | Toasty 0.10 的 `Driver` trait 固定返回 `Migration`，out-of-tree driver 无法改上游签名 |

## 4. 已接受设计

### 4.1 Tracking 表与 ID

driver 使用保留表 `__toasty_migrations`，以 `DEFINE TABLE IF NOT EXISTS ... SCHEMALESS` 幂等创建。
每个 migration 用 `type::record("__toasty_migrations", <decimal-id-string>)` 作为 record id，并保存
绑定参数 `name` 与数据库生成的 `applied_at = time::now()`。

ID 使用十进制字符串而不是 SurrealDB number：Toasty 的 ID 是完整 `u64`，SurrealDB record-id
Number 只有 `i64`。`applied_migrations()` 读取 `record::id(id)`，严格解析回 `u64`；缺失、类型错误
或越界返回 `serialization_failure`，错误不回显整条 tracking 记录。

`__toasty_migrations` 是 driver 保留名，应用模型不得占用。当前上游契约没有 checksum/name 校验
输入，driver 不虚构相应保证。

### 4.2 原子应用

`apply_migration(id, name, migration)` 的顺序为：

1. 幂等确保 tracking 表存在；
2. 拒绝 Connection 上已经存在的用户显式事务；
3. 用 `db.begin()` 创建独立 SDK 客户端事务；
4. 按 Toasty breakpoint 顺序执行所有非空 migration statement，并对每次响应调用 `check()`；
5. 在同一事务内 `CREATE` 确定性 tracking record；
6. commit；任一语句或 tracking 写入失败则 cancel，且不记录 ID。

确定性 record id 同时阻止同一 ID 被重复记录。迁移文本可以包含用户手写 SurrealQL，因此日志只
记录 statement 序号而不记录文本；migration name 通过绑定值写入，不作为日志字段或拼接进语句。
statement error 也转换为不回显原始 query/value 的结构化错误；driver 自生成的人工门禁单独映射为
`unsupported_feature`。

### 4.3 Migration 容器

Toasty 0.10 的 `schema::db::Migration` 目前只有 `Sql(String)`，migration 文件扩展名和
`embed_migrations!` 也固定为 `.sql`。本 driver 把该字符串视为 **SurrealQL**，使用现有
`-- #[toasty::breakpoint]` 标记分割语句；不因此宣称 SQL capability，也不依赖 `toasty-sql`。

### 4.4 自动生成范围

`Driver::generate_migration()` 直接遍历 `diff::Schema`，按依赖安全的顺序生成：

| Diff | 生成行为 |
|---|---|
| Create table | `DEFINE TABLE ... SCHEMALESS`，随后定义所有非主键索引 |
| Drop table | `REMOVE TABLE ...`（索引随表删除） |
| Create index | `DEFINE INDEX ... COLUMNS ... [UNIQUE]` |
| Drop index | `REMOVE INDEX ... ON TABLE ...` |
| Alter index | 先 REMOVE 旧定义，再 DEFINE 新定义 |
| Add SCHEMALESS column | 无数据库 DDL |
| Drop non-PK column | `UPDATE <table> UNSET <column> RETURN NONE` |
| Rename non-PK column（类型不变） | 先把引用索引切换到新字段，再复制旧字段并 UNSET 旧字段 |
| Rename PK column（类型与 PK 顺序不变） | 无数据 DML；值仍位于 record id，仅投影别名改变 |
| nullable/auto-increment/versionable 元数据变化 | 无数据库 DDL；表保持 SCHEMALESS |

完全没有物理语句的非空 diff 生成 `RETURN NONE`，保证 migration 文件仍可执行和记录。

索引与字段 DML 的固定顺序是：REMOVE 旧索引 → DEFINE 新索引 → 搬迁/清理字段。SDK 3.2.4 若在
同一事务中先写入尚未提交的新字段、再 DEFINE INDEX，index builder 看不到这些未提交值；先定义
空的新字段索引后再执行 UPDATE，DML 会正常维护新索引并执行唯一约束。

### 4.5 人工迁移门禁

SurrealDB 3.2.4 没有表/字段 rename DDL，且 record id 把表名与主键值编码进物理身份。以下变化不能
从 Toasty diff 安全推导通用数据转换：

- 表重命名；
- 主键列集合、顺序或存储类型变化；
- 非主键字段 `stmt::Type` / `storage_ty` 变化（包括普通 String 与 native JSON 间转换）；
- 任何未来未识别的 diff 变体。

由于上游 generation trait 不能返回 `Result`，driver 为这些变化生成
`THROW 'manual migration required: ...'`。生成过程本身不 panic，用户可以审阅并替换该 statement；
若未替换，apply 会结构化失败并由事务回滚，绝不把 migration ID 记为成功。

### 4.6 DDL 复用与 SemVer

`push_schema` 与 migration generation 复用同一套 table/index statement renderer，避免索引列、
identifier escaping 与 `UNIQUE` 语义漂移。匹配 Toasty 未封闭 diff 枚举时保留 `_` 通配臂；未知
变化生成上述人工门禁。

## 5. 测试边界

- 单元测试覆盖 create/drop table、create/drop/alter index、SCHEMALESS add/drop/rename、特殊标识符、
  no-op 与人工门禁生成；
- kv-mem 集成测试通过真实 Connection 验证首次应用、查询 ID、重复跳过语义、breakpoint 顺序以及
  `u64::MAX` tracking；
- 失败迁移验证 DDL/DML 与 tracking record 同时回滚，后续连接仍可使用；
- SurrealKV 文件引擎验证 tracking 和已应用 migration ID 跨重开持久化；
- fmt、check、clippy、默认测试、RocksDB e2e、rustdoc 与 cargo-deny 继续作为完整门禁。

## 6. 验收标准

1. `generate_migration()` 不再 panic，支持 §4.4 的安全 diff；
2. 不安全 diff 生成明确可编辑的 `THROW` 门禁，应用失败且不落 tracking；
3. `MigrationSet::apply()` 所需的两个 Connection 方法均可用；
4. migration statement 与 tracking record 在一个 SDK 客户端事务内提交或回滚；
5. `u64::MAX` ID 无损往返，重复 ID 不会被重复记录；
6. tracking 表可在 kv-mem 与 SurrealKV 上使用，文件引擎重开后记录仍在；
7. 无新增生产依赖，不改变 KV capability 或现有显式事务语义；
8. 用 Semifold CLI 创建独立 changeset 并确认发布计划。
