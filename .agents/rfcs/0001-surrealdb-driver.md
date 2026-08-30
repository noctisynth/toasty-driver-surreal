# RFC 0001：Toasty SurrealDB Driver

> 状态：Accepted
> 接受日期：2026-08-26
> 设计入口：[设计索引](../DESIGN.md)
> 落地规范：[Driver Active Spec](../specs/driver.md)
> 验证证据：[SurrealDB SDK Spike](../spikes/surrealdb-sdk-3.2.4.md)
> 后续扩展：[RFC 0005：迁移跟踪与自动生成](0005-migrations.md)

本 RFC 记录为 [Toasty ORM](https://github.com/tokio-rs/toasty) 实现 out-of-tree SurrealDB
driver 的架构决定：把 SurrealDB 作为 Toasty 的 **KV/文档后端**接入，通过实现
`toasty_core::driver::{Driver, Connection}`，把引擎下发的键值 `Operation` 翻译为 SurrealQL，
使用官方 `surrealdb` 嵌入式 SDK 执行。

本仓库无人工审阅门禁，RFC 由实现方在边界自证清晰后自行接受。接受只表示架构边界确认；公共
行为、能力位、错误语义和测试门禁以 [Driver Active Spec](../specs/driver.md) 为准。

## 1. 背景

Toasty 内置 SQLite、PostgreSQL、MySQL、DynamoDB、Turso 五个 driver，并把 driver 契约
（`toasty-core`）与 SQL 序列化（`toasty-sql`）发布到 crates.io（当前 `0.10.0`）。driver 与
用户 API crate（`toasty`）解耦：内置的非 SQLite driver 只依赖 `toasty-core`（SQL driver 另加
`toasty-sql`）。因此第三方可以在独立 crate 中实现 driver，无需 fork 主仓库。

SurrealDB 是多模数据库，查询语言为 SurrealQL。表面上它像 SQL，但记录标识（`table:id`）、
`CREATE`/`UPSERT`/`type::record()` 等语义与三种内置 SQL 方言均不兼容。

## 2. 目标

1. 提供一个可用的 SurrealDB Toasty driver，覆盖 create/read/update/delete/upsert/scan/index。
2. 复用 Toasty 的 `toasty-driver-integration-suite` 共享测试证明行为正确。
3. 支持嵌入式引擎（内存 `kv-mem` 与文件 `kv-rocksdb`），端到端可运行。
4. 保持 out-of-tree 结构，只依赖已发布的 `toasty-core`。
5. 对 `toasty-core` 未封闭类型的 SemVer 漂移保持稳健。

## 3. 非目标

- 不实现新的 SQL 方言，不修改或依赖 `toasty-sql`、`Dialect`。
- 不实现 `Db::builder().connect("surreal://…")` 的 URL scheme 自动注册（Toasty 的 scheme→driver
  映射在 `toasty` crate 内硬编码，out-of-tree 无法扩展）；driver 通过 `Db::builder().build(driver)`
  接入。
- 首阶段不实现 SurrealDB 事务下发（引擎按 `capability().sql()` 门控，KV 路径默认不下发
  `Operation::Transaction`）。
- 首阶段不实现远程引擎（`ws://`/`http://`）、图边（`RELATE`）、live query、鉴权与多租户。
- 首阶段不实现 SurrealDB 的 schema 迁移生成（`generate_migration`）；该限制后由 RFC 0005 扩展。

## 4. 已接受决策

### 4.1 归类为 KV/文档 driver（`Capability.sql = None`）

**决策**：driver 报告 `sql = None`，走 Toasty 引擎的键值路径，以 in-tree DynamoDB driver 为
结构蓝本。

**依据**：核对 Toasty 引擎源码，决定 SQL 路径 vs KV 路径的唯一开关是 `capability().sql()`
布尔值（散布于 `legalize.rs`/`lower.rs`/`plan/statement.rs`/`verify.rs` 等十余处）；针对具体
`Dialect` 的分支全仓库仅一处（MySQL insert-id）。设 `sql = None` 后引擎完全不触碰 `Dialect`
与 `toasty-sql`，而是下发结构化 KV 操作：`Insert`、`GetByKey`、`QueryPk`、`FindPkByIndex`、
`Scan`、`UpdateByKey`、`DeleteByKey`、`Upsert`。SurrealQL 与三种内置方言不兼容，因此“做成第四
种 SQL 方言”不可行（`Dialect` 枚举封闭、`toasty-sql` 只吐三种方言）；KV 路径反而让 driver
完全掌控 SurrealQL 拼装。

### 4.2 使用官方 `surrealdb` 嵌入式 SDK

**决策**：依赖 `surrealdb` crate（经 Spike 验证的稳定版 `3.2.4`），使用嵌入式引擎
`engine::local::{Mem, RocksDb}`。值类型使用 `surrealdb::types::Value`。

**依据**：Spike 证明嵌入式 SDK 支持全部所需操作与值往返；`surrealdb 3.2.4` 与 `toasty-core
0.10` 可共存解析。远程引擎留待后续。

### 4.3 记录 ID 使用 `type::record()`，PK ↔ RecordId 结构化映射

**决策**：所有按主键定位的语句用 `type::record($tb, $id)` 构造记录 ID（**非** `type::thing`，
后者在 3.x 已移除）。主键与 SurrealDB `RecordId` 的映射规则：

- 单列主键 → `RecordIdKey::{Number(i64) | String | Uuid}`，按 `stmt::Value` 类型选择；
- 复合（多列）主键 → `RecordIdKey::Array`，元素按主键列顺序编码；
- 读取投影时基于 native `RecordId` 反解，不解析 `"tb:key"` 字符串。

**依据**：Spike 探针 2/5/6/11 验证 `type::record` 接受字符串/整数/数组 id 并正确回显；
`type::thing` 报错。

### 4.4 值编解码走 native `surrealdb::types::Value`

**决策**：实现 `stmt::Value ↔ surrealdb::types::Value` 双向映射模块（对标
`dynamodb/src/value.rs`），不经 JSON 中转。

**依据**：Spike 探针 21 显示 JSON 路径对 `Bytes`（变数组）、`Datetime`（变字符串）、`Uuid`
有歧义，而 native `Value` 保真。`stmt::Value` 的整数宽度（I8..U64）、`Bool`、`F32/F64`、
`String`、`Bytes`、`Uuid` 映射到 `Value::{Number(Int/Float), Bool, String, Bytes, Uuid}`；
`Null`→`Value::Null`；`List`→`Value::Array`；`Object`（`#[document]`）→`Value::Object`。
可选特性 `jiff`/`rust_decimal`/`net` 对应 `Datetime`/`Number::Decimal`/字符串编码，首阶段可先
不开这些特性。

### 4.5 能力位画像（优于 DynamoDB 的保守取值）

**决策**：以 `..Capability::DYNAMODB` 为基线（FRU），覆盖以下差异位：

| 能力 | 取值 | 依据 |
|---|---|---|
| `sql` / `sql_placeholder` | `None` / `None` | KV 路径 |
| `driver_name` | `"SurrealDB"` | 诊断 |
| `scan` | `true` | `SELECT * FROM tb` |
| `scan_supports_sort` | `true` | SurrealQL `ORDER BY`（Spike 探针 14） |
| `index_or_predicate` | `true` | `WHERE` 原生支持 `OR` |
| `upsert_primary_key` | `true` | 原生 `UPSERT`（Spike 探针 8） |
| `primary_key_ne_predicate` | `true` | SurrealQL `WHERE id != ..` 无 DynamoDB 限制 |
| `native_starts_with` | `false` | 首阶段用 `string::starts_with` 需另验；保守走通用路径 |

其余位（`native_json` 等）暂随 DYNAMODB 基线，由 Spec 与集成测试逐步收敛。**所有能力位必须
用 FRU 构造并提升为 `&'static`，新增字段自动继承 DYNAMODB。**

### 4.6 通过 `build(driver)` 接入，不注册 URL scheme

**决策**：driver 暴露 `SurrealDb::mem()`、`SurrealDb::rocksdb(path)` 等构造器；用户用
`Db::builder().models(..).build(SurrealDb::…)` 接入。

**依据**：`toasty` crate 的 `Connect::new(url)` scheme 映射硬编码且 feature-gate，out-of-tree
无法扩展；`build(driver: impl Driver)` 是内置 driver 在测试/示例中使用的等价入口，功能不受损。
driver 内部仍实现 `Driver::url()` 返回一个信息性 `surrealdb:...` 字符串。

### 4.7 SemVer 稳健性

**决策**：构造 `Capability` 必须用 `..Capability::DYNAMODB`；匹配 `Operation`、`stmt::Value`、
`stmt::Type` 必须带 `_` 通配臂，未支持分支返回 `Error::unsupported_feature`，不 `todo!()`。

**依据**：核对确认 `toasty-core` 的 `Capability`/`Operation`/`Value`/`Type` 均未标
`#[non_exhaustive]`，上游可在 minor 版本加字段/变体，风险由 driver 作者承担。

## 5. 依赖方向

```text
用户应用
   │  Db::builder().build(SurrealDb::…)
   ▼
toasty (用户 API + 引擎)
   │  下发 KV Operation
   ▼
toasty-driver-surreal  ──depends on──► toasty-core (driver 契约)
   │  拼装 SurrealQL + 值编解码
   ▼
surrealdb (嵌入式 SDK: kv-mem / kv-rocksdb)
```

- driver 只依赖 `toasty-core`；不依赖 `toasty`、`toasty-sql`。
- dev-dependency：`toasty`、`toasty-driver-integration-suite`、`tokio`、`uuid`。

## 6. Operation → SurrealQL 映射（验收基线）

| Operation | SurrealQL 形态 |
|---|---|
| `Insert` | `CREATE type::record($tb,$id) CONTENT $data`（可选 `RETURN AFTER`）；冲突→唯一冲突错误 |
| `GetByKey` | `SELECT <cols> FROM type::record($tb,$id), …`；未命中→空 |
| `QueryPk` | `SELECT <cols> FROM <tb> WHERE <pk_filter> [AND <filter>] [ORDER BY][LIMIT/START]` |
| `FindPkByIndex` | `SELECT <pk cols> FROM <tb> WHERE <index filter>` |
| `Scan` | `SELECT <cols> FROM <tb> [WHERE <filter>] [LIMIT/START]` |
| `UpdateByKey` | `UPDATE type::record($tb,$id) [SET/MERGE …] [WHERE <filter>] [RETURN AFTER]` |
| `DeleteByKey` | `DELETE type::record($tb,$id) [WHERE <filter>] [RETURN BEFORE]` |
| `Upsert` | `UPSERT type::record($tb,$id) CONTENT/MERGE … RETURN AFTER` |

`filter: stmt::Expr` 通过一个 `surreal_expression()` 翻译器（对标 DynamoDB 的
`ddb_expression()`）转成 SurrealQL `WHERE` 子句，标量值以绑定参数下发。

## 7. push_schema 与索引

- `push_schema`：对每个表 `DEFINE TABLE <name> SCHEMALESS`（或 SCHEMAFULL，待 Spec 定），并为
  每个二级索引 `DEFINE INDEX <name> ON TABLE <tb> COLUMNS <cols> [UNIQUE]`。主键由 record id
  承担，不单独建索引。
- `generate_migration`：首阶段 `unimplemented!()`（同 DynamoDB）；当前行为由 RFC 0005 与 Active
  Spec 的迁移章节替代。

## 8. 错误分类

- 连接/会话失败 → `Error::connection_lost`；
- 唯一冲突（`already exists`）→ 由 verify/insert 路径映射为可识别冲突（`Error` 相应变体）；
- 未支持操作/特性 → `Error::unsupported_feature`；
- 其它 SurrealDB 错误 → `Error::driver_operation_failed`。
- 错误 Display/Debug/tracing 不得泄漏完整记录内容或绑定值。

## 9. 主要取舍

### 9.1 收益

- 无需 fork Toasty，纯 out-of-tree crate。
- SurrealDB 原生 record id / UPSERT / 索引 / 排序与 KV 契约高度吻合，映射自然。
- 能力位比 DynamoDB 更宽（scan 排序、OR 谓词、PK upsert）。

### 9.2 成本与风险

- 关联查询被引擎拆成按键操作 + 内存 `NestedMerge`，无法下推为单条 SurrealQL（与 DynamoDB
  一致，功能不缺、非性能最优）。
- 用户无法通过 Toasty 直接跑裸 SurrealQL（`RawSql` 仅发 SQL driver）。
- `toasty-core` 未封闭类型带来的 SemVer 维护成本由本 crate 承担。
- 复合 record id 的排序/游标语义需集成测试验证。

## 10. 被拒绝的方案

| 方案 | 结论 | 原因 |
|---|---|---|
| 做成第四种 SQL 方言（`sql=Some(..)`）| 拒绝 | `Dialect` 封闭、`toasty-sql` 只吐三方言，需重写序列化器 |
| fork Toasty 主仓库内置 driver | 拒绝 | 目标是 out-of-tree；`toasty-core` 已发布可依赖 |
| 值编解码走 JSON 中转 | 拒绝 | JSON 对 bytes/datetime/uuid 有歧义（Spike 探针 21） |
| 注册 `surreal://` URL scheme | 拒绝 | `toasty` crate scheme 映射硬编码，out-of-tree 不可扩展 |
| 首阶段支持远程引擎与事务 | 推迟 | 缩小首阶段范围；KV 路径默认不下发事务 |

## 11. 验收场景

Active Spec 与集成/e2e 测试至少覆盖：

1. `build(SurrealDb::mem())` 成功建库并 push_schema；
2. create → get_by_key → update → delete 闭环（单列 PK：i64、String、Uuid）；
3. 复合主键的 create 与按键读取；
4. 过滤 + 排序 + 分页查询；
5. 二级索引查询（`FindPkByIndex`）；
6. 原生 upsert（insert 与 update 两条路径）；
7. 值类型往返（bool/整数各宽度/f64/string/bytes/uuid）；
8. 唯一冲突被正确分类；
9. 嵌入式 RocksDB e2e（数据目录 `.e2e-data/`，gitignore）；
10. `toasty-driver-integration-suite` 共享测试在 `kv-mem` 上通过（不支持项以能力位关闭）。

## 12. 实施门禁

- [SurrealDB SDK Spike](../spikes/surrealdb-sdk-3.2.4.md) 已完成（21 探针通过）。
- 后续若 `surrealdb` 升级、SDK 契约变化或能力位需调整，先复跑 Spike 并更新本 RFC 与 Spec，
  再改实现。
