# Toasty SurrealDB Driver 规范

> 状态：Active Spec
> 规范基线：2026-08-26
> 适用范围：`toasty-driver-surreal` crate
> 设计入口：[设计索引](../DESIGN.md)
> 决策来源：[RFC 0001：Toasty SurrealDB Driver](../rfcs/0001-surrealdb-driver.md)、
> [RFC 0002：SurrealKV 引擎](../rfcs/0002-surrealkv-engine.md)
> 验证证据：[SurrealDB SDK Spike](../spikes/surrealdb-sdk-3.2.4.md)
> 实施清单：[Driver TODO](../todos/driver.md)

本文冻结 driver 的公共接口、能力画像、值编解码、记录 ID 映射、每个 `Operation` 的 SurrealQL
翻译、schema 推送、错误分类和测试门禁。没有被本文纳入的行为（事务、远程引擎、图边、迁移
生成）不得由实现自行补全。

## 1. 范围

### 1.1 首阶段目标

- 实现 `toasty_core::driver::{Driver, Connection}`，作为 KV/文档 driver（`sql = None`）。
- 支持嵌入式引擎：内存（`kv-mem`）、文件 RocksDB（`kv-rocksdb`）与文件 SurrealKV
  （`kv-surrealkv`）。
- 覆盖 `Insert`、`GetByKey`、`QueryPk`、`FindPkByIndex`、`Scan`、`UpdateByKey`、`DeleteByKey`、
  `Upsert` 八个 `Operation`。
- `push_schema` 定义表与二级索引。
- 接入 `toasty-driver-integration-suite` 共享测试（`kv-mem`）+ 嵌入式 RocksDB e2e。

### 1.2 明确非目标

- 事务（`Operation::Transaction`）；引擎按 `capability().sql()` 门控，KV 路径默认不下发。
- 远程引擎（`ws://`/`http://`）、鉴权、多租户。
- 图边（`RELATE`）、live query、full-text/vector 检索。
- SurrealDB schema 迁移生成（`generate_migration` 首阶段 `unimplemented!()`）。
- `Db::builder().connect(url)` 的 scheme 注册（out-of-tree 不可扩展）。
- 用户裸 SurrealQL（`RawSql` 仅发 SQL driver）。

## 2. crate 与依赖边界

### 2.1 依赖

- 生产依赖：`toasty-core`（driver 契约）、`surrealdb`（嵌入式 SDK，Spike 验证的稳定版
  `3.2.4`，features `kv-mem`、`kv-surrealkv`，可选 `kv-rocksdb`）、`async-trait`、`tracing`。
- dev 依赖：`toasty`、`toasty-driver-integration-suite`、`tokio`（`macros`、`rt-multi-thread`）、
  `uuid`。
- 不依赖 `toasty`（生产）、`toasty-sql`、`Dialect`。

### 2.2 边界

- `surrealdb` 类型不得出现在 driver 公共 API 中，除值编解码内部与错误转换外。
- driver 公共表面只暴露：`SurrealDb`（`Driver` 实现）、其构造器、以及必要的错误/配置类型。

## 3. 公共接口

```rust
/// SurrealDB Toasty driver，基于嵌入式 surrealdb SDK。
#[derive(Debug, Clone)]
pub struct SurrealDb { /* private */ }

impl SurrealDb {
    /// 进程内内存引擎（kv-mem）。每次 build 得到全新空库。
    pub fn mem() -> Self;

    /// 嵌入式 RocksDB 文件引擎（kv-rocksdb），path 为数据目录。
    pub fn rocksdb(path: impl Into<std::path::PathBuf>) -> Self;

    /// 嵌入式 SurrealKV 文件引擎（kv-surrealkv），path 为数据目录。
    pub fn surrealkv(path: impl Into<std::path::PathBuf>) -> Self;

    /// 设置 namespace 与 database，默认 "toasty" / "toasty"。
    pub fn namespace(self, ns: impl Into<String>) -> Self;
    pub fn database(self, db: impl Into<String>) -> Self;
}
```

- 首阶段用 `Db::builder().models(..).build(SurrealDb::mem())` 接入，不经 `connect(url)`。
- `Driver::url()` 返回信息性字符串：内存 `surrealdb:mem`，文件
  `surrealdb:rocksdb:<path>` / `surrealdb:surrealkv:<path>`。
  出现在日志中的 URL 不含凭证（首阶段嵌入式无凭证）。

### 3.1 连接与共享实例

- driver 内部缓存一个共享 `Surreal<Db>` 句柄（对标 Turso 的 `Arc<Mutex<Option<..>>>`），
  所有 `connect()` 复用同一底层库，保证内存库在连接池多 slot 间可见。
- `connect()` 返回的 `Connection` 持有该句柄克隆并已 `use_ns/use_db`。
- `reset_db()`：内存引擎丢弃缓存句柄；RocksDB/SurrealKV 文件引擎删除数据目录后重建。
- `max_connections()`：内存引擎返回 `Some(1)`（与 in-memory SQLite 一致，避免多 slot 各开空
  库），RocksDB/SurrealKV 文件引擎返回 `None`。

## 4. 能力画像

以 `..Capability::DYNAMODB` 为 FRU 基线，覆盖差异位：

```rust
fn capability(&self) -> &'static Capability {
    const CAP: Capability = Capability {
        driver_name: "SurrealDB",
        // KV 路径
        sql: None,
        sql_placeholder: None,
        // SurrealQL 原生能力，优于 DynamoDB 保守取值
        scan: true,
        // 扫描不排序：unindexed 读走 scan 路径，排序读走 QueryPk 索引路径；
        // 与共享合约「非 SQL scan 无序」一致。
        scan_supports_sort: false,
        index_or_predicate: true,
        upsert_primary_key: true,
        primary_key_ne_predicate: true,
        ..Capability::DYNAMODB
    };
    &CAP
}
```

- **必须**用 FRU；新增字段自动继承 DYNAMODB。
- `native_starts_with` 继承 DYNAMODB 的 `true`（SurrealDB 有 `string::starts_with`），driver
  在 `expr::render` 里以带 NULL 守卫的形式渲染。
- 其余位（`native_json`、`vec_*`、`document_collections`、日期/decimal 等）随 DYNAMODB 基线。
- `validate()` 必须通过（单元测试覆盖），运行时 `generate_driver_tests!` 的能力校验也通过。

## 5. 记录 ID 与主键映射

- 记录 ID 一律用 `type::record($tb, $id)` 构造（**禁止** `type::thing`，3.x 已移除）。
- 主键列取自 `table.primary_key_columns()`（按序）。映射规则：

| 主键形态 | `stmt::Value` | SurrealDB record id key |
|---|---|---|
| 单列整数 | `I8..I64`/`U8..U64` | `RecordIdKey::Number(i64)` |
| 单列字符串 | `String` | `RecordIdKey::String` |
| 单列 UUID | `Uuid` | `RecordIdKey::Uuid` |
| 复合（≥2 列） | `Record([..])` | `RecordIdKey::Array`（元素按列序编码为 surreal `Value`） |

- 引擎按键操作传入的 `keys: Vec<stmt::Value>`：**实测中即便单列主键，引擎也可能把键包成
  单元素 `Value::Record`**（如 `GetByKey`），而 insert 行里单列键是裸标量。因此 record id 的
  形态由**主键列数（arity）**决定，而非值是否为 record：单列 → 标量 key（必要时解包单元素
  record），复合 → `Array` key。
- 读取行时，SurrealDB 返回的行含 `id` 字段（native `RecordId`）。driver 在投影里用
  `record::id(id) AS <col>`（复合键用 `record::id(id)[k] AS <col>`）把主键列别名回列名，其余
  列直接取；缺失字段填 `Value::Null`。
- 无符号整数超过 `i64::MAX`：**以 `Number::Decimal` 编码**（不丢失量级），`U64` 解码路径从
  Decimal 还原；不再返回 `unsupported_feature`。

## 6. 值编解码

实现独立模块 `value.rs`，提供 `stmt::Value → surrealdb::types::Value`（写入/绑定）与
`surrealdb::types::Value + stmt::Type → stmt::Value`（读取解码）。**不经 JSON 中转。**

| `stmt::Value` | `surrealdb::types::Value` |
|---|---|
| `Null` | `Null`（或 `None`，读取时二者都归为 `Null`） |
| `Bool` | `Bool` |
| `I8/I16/I32/I64` | `Number(Number::Int(i64))` |
| `U8/U16/U32` | `Number(Number::Int(i64))` |
| `U64` | `Int` 若 ≤ i64::MAX，否则 `Number::Decimal`（不丢失量级） |
| `F32/F64` | `Number(Number::Float(f64))` |
| `String` | `String` |
| `Bytes` | `Bytes` |
| `Uuid` | `Uuid` |
| `List(items)` | `Array` |
| `Record(items)` | `Array`（用于复合键；行记录用 object） |
| `Object(fields)` | `Object`（`#[document]` 列，**跳过 null 字段**以支持 `IS NONE`） |
| `Timestamp/Date/Time/DateTime/Decimal/Cidr/...` | `String`（canonical text，读取时 `Type::cast` 还原） |

读取解码按目标 `stmt::Type` 定向（对标 SQLite `from_sql`）：`Number::Int/Decimal` 依
`Type::{I8..U64, Bool}` 收窄（`U64` 从 Decimal 还原）；`Number::Float` 依 `Type::{F32,F64}`；
`String` 依目标类型（`Uuid` 解析；temporal/decimal/net 走 `Type::cast`；`String`/`Unknown`
保持字符串——`Unknown` 用于 `#[document]` 叶子，由引擎在 raise embed 时再 cast）；`RecordId`
→ 依主键列类型反解为标量或 `Record`。未覆盖的组合返回结构化错误，不 panic。

crate 特性：本 crate 生产依赖启用 `toasty-core` 的 `jiff` + `rust_decimal`，使 temporal 与
decimal 值可编解码。`net` 未启用。SurrealKV 随默认构建可用；RocksDB 由本 crate 的 `rocksdb`
feature 开启，默认关闭，因为会额外编译较慢的 `librocksdb`。

## 7. Operation → SurrealQL 翻译

所有标量值以绑定参数（`.bind((name, value))`）下发，name 用稳定生成规则（`p0,p1,…`）；记录 ID
以 native `RecordId` 绑定为 `$rid`/`$kN`（不做字符串拼接）。列投影用列名列表。

> **重要（Insert 路由）**：实测 toasty 0.10 引擎对 KV driver 的插入不走
> `Operation::Insert`，而是把 `Statement::Insert` 包在 `Operation::QuerySql` 里下发。driver 的
> `exec` 因此在 `QuerySql` 分支里判定内层是否 `Statement::Insert` 并转到插入处理；`Insert`
> 变体也保留处理以防其它引擎版本使用它。

### 7.1 Insert

- 语句：`CREATE $rid CONTENT $data`（有 RETURNING 时加 `RETURN AFTER`）。
- `$data` 为 object，键为列名；主键列不放入 CONTENT（由 record id 承担），其余非空列放入。
  多行 insert 循环执行。
- 返回：有 RETURNING 投影时解码为行；否则返回受影响计数。冲突（`already exists`）见 §9。

### 7.2 GetByKey

- 语句：`SELECT <cols> FROM $k0, $k1, …`。主键列在投影里用 `record::id(id) AS <col>` 别名。
- 未命中键不产生行（不报错）。

### 7.3 QueryPk

- 语句：`SELECT <cols>[, <sort_ref> AS __toasty_sort_key] FROM <tb> WHERE <pred>
  [ORDER BY __toasty_sort_key ASC|DESC] [LIMIT n] [START m]`。
- `op.index` 为 `Some` 时按索引字段过滤（SurrealDB 索引对查询透明）。
- `op.pk_filter` 与 `op.filter` 经 `expr::render()` 翻译（§8）AND 合并。
- **排序**：SurrealQL 拒绝把 `record::id(id)[k]` 直接作为 ORDER BY idiom，故排序键投影为隐藏
  别名 `__toasty_sort_key` 再 `ORDER BY` 该别名。
- **分页**：`Offset` → `LIMIT/START`；`Cursor` → keyset（默认排序键升序，`after` 加
  `sort_ref > $cursor`，降序用 `<`；页满时从末行排序键别名取 `next_cursor`）。
  `backward_pagination=false`，不产 prev。

### 7.4 FindPkByIndex

- 语句：`SELECT <pk cols> FROM <tb> WHERE <index filter>`，返回主键行供后续 `GetByKey`。

### 7.5 Scan

- 语句：`SELECT <cols> FROM <tb> [WHERE <filter>] [LIMIT/START]`。无 ORDER BY
  （`scan_supports_sort=false`），cursor 分页用 record id 排序键做 keyset 续页。

### 7.6 UpdateByKey

- `op.keys` 恒为单键。语句：`UPDATE $rid SET <assignments> [WHERE <filter/condition>]
  [RETURN AFTER]`。
- `op.assignments`：`Set` → `col = $v`（`Null` → `col = NONE`）；`Add/Subtract` →
  `col = col ± $v`；`Append`（push/extend）→ `col += $list`（单元素自动包成 list）。其它变体
  返回 `unsupported_feature`（带 `_` 通配臂）。
- `op.condition` 失败（更新 0 行）→ `Error::condition_failed`。
- `op.returning`：`Some` → `RETURN AFTER` 解码；`None` → 返回计数。

### 7.7 DeleteByKey

- 单键。语句：`DELETE $rid [WHERE <filter/condition>] RETURN BEFORE`（用返回行数计数）。
- 纯 `filter` 未命中 → count 0；`condition` 失败 → `condition_failed`。

### 7.8 Upsert

- `Update` action：`UPSERT $rid SET …`。SurrealDB UPSERT 在 create 与 update 都执行 SET：
  - create-only 列用 `col = col ?? $v`（等价 `if_not_exists`）；
  - shared 操作用 `col = (col ?? default) ± $v` / `array::concat(col ?? default, $list)`，
    把声明的 `#[default]` 折叠进去；
  - 无可写项时退化为 `MERGE $content`。
- `Ignore` action：`CREATE $rid CONTENT $data`，`already exists` 冲突被吞（空/0）。
- 仅支持主键目标（非主键目标 → `unsupported_feature`）。

## 8. 过滤表达式翻译（`expr::render`）

对标 DynamoDB 的 `ddb_expression()`：递归把 `stmt::Expr` 翻译为 SurrealQL 布尔表达式，标量以
绑定参数下发。已支持：

- `BinaryOp`：`Eq/Ne/Gt/Ge/Lt/Le` → `=/!=/>/>=/</<=`；`Add/Sub` 用于赋值上下文。
- `And`/`Or`（括号包裹）/`Not`（用 `!( … )` 前缀，SurrealQL 拒绝 `NOT x IS NONE`）。
- `IsNull` → `<field> IS NONE`（操作数按字段引用渲染，避免 bool 列 `= true` 干扰）。
- `Between` → `(x >= lo AND x <= hi)`（无链式关系运算符）。
- `InList` → `x IN [ … ]`；`List` → 数组字面量。
- `AnyOp`（`= ANY`）→ `list CONTAINS value`（其它运算符 unsupported）。
- `IsSuperset` → `CONTAINSALL`；`Intersects` → `CONTAINSANY`；`Length` → `array::len(x)`。
- `StartsWith` → `(x IS NOT NONE AND string::starts_with(x, $p))`（守卫 NULL/非串）。
- `Func(JsonExtract)`（`#[document]` 路径）→ 点号字段引用 `col.a.b`（各段转义）。
- `Reference`（主键列 → `record::id(id)[/k]`；bool 裸列 → `col = true`；其余 → 反引号列名）。
- 未覆盖变体返回 `Error::unsupported_feature`（**带 `_` 通配臂**，不 panic）。

主键列不是普通字段，必须经 `record::id(id)` 引用。`column_ref` 依 `Table::primary_key.columns`
判定主键成员，**而非** `Column::primary_key`——schema builder 不设置后者。

## 9. 错误分类

| 情况 | `toasty_core::Error` |
|---|---|
| 连接/会话建立失败、句柄不可用 | `connection_lost` |
| 唯一冲突（`already exists`）| 由 insert/upsert 路径识别并映射为冲突（`condition_failed` 或引擎期望的冲突变体，依集成测试对齐）|
| 前置 `condition` 失败 | `condition_failed` |
| 未支持的 Operation/Expr/特性 | `unsupported_feature` |
| 无效连接配置 | `invalid_connection_url`（若走 URL 解析路径）|
| 其它 SurrealDB 错误 | `driver_operation_failed` |

- 错误 `Display`/`Debug`/tracing 不得携带完整记录内容或绑定参数值。SurrealDB 原始错误文本在
  保留诊断价值前提下透传给 `driver_operation_failed`，不额外拼接用户数据。

## 10. push_schema 与迁移

- `push_schema(schema)`：对 `schema.db.tables` 每张表执行 `DEFINE TABLE <name> SCHEMALESS`；
  对每个非主键索引执行 `DEFINE INDEX <name> ON TABLE <tb> COLUMNS <cols> [UNIQUE]`。主键由
  record id 承担，不建独立索引。
- `applied_migrations`/`apply_migration`：首阶段可用一张 `__toasty_migrations` 表记录（对标
  SQLite），或随 DynamoDB `todo!()`——**但不得 panic 逃逸到用户**；若不实现则返回
  `unsupported_feature`。首阶段实现选择记录在 Driver TODO 中并保持与测试一致。
- `generate_migration`：`unimplemented!()`（与 DynamoDB 一致，测试不覆盖迁移生成）。

## 11. 测试门禁

### 11.1 集成套件（kv-mem）

- 实现 `toasty_driver_integration_suite::Setup`：`driver()` 返回
  `Box::new(SurrealDb::mem())`；`delete_table(name)` 对内存库可 no-op（每次 build 新库）或
  执行 `REMOVE TABLE`。
- 用 `generate_driver_tests!(SurrealSetup::new(), <capability flags>)` 生成共享测试；不支持的
  能力位以 `flag: false` 声明，使套件跳过对应用例。
- 目标：create/get/update/delete/upsert/query/index/分页/值往返用例通过；逐位调能力直至绿。

### 11.2 端到端（kv-rocksdb）

- 独立测试文件用 `SurrealDb::rocksdb(".e2e-data/<test>")` 建库，跑一遍核心 CRUD + 查询闭环，
  证明文件引擎可用。
- 数据目录在工作目录 `.e2e-data/` 下，**必须**被 `.gitignore` 覆盖；测试开始前清理残留目录。

### 11.3 端到端（kv-surrealkv）

- 独立测试文件用 `SurrealDb::surrealkv(".e2e-data/surrealkv-<test>")` 运行与 RocksDB 对等的
  CRUD、过滤扫描和跨重开持久化场景。
- SurrealKV 随默认测试运行，并可与 `rocksdb` 同时启用；CI 的默认测试覆盖 SurrealKV e2e。
- 数据目录同样必须位于 `.e2e-data/` 下并由 `.gitignore` 覆盖。

### 11.4 单元测试

- 值编解码 round-trip（各 `stmt::Value` 变体）。
- 记录 ID 映射（单列 i64/String/Uuid、复合）。
- `surreal_expression` 对各 `Expr` 变体的输出片段。

## 12. SemVer 稳健性

- `Capability` 用 `..Capability::DYNAMODB` FRU 构造。
- `match` `Operation`/`stmt::Value`/`stmt::Type` 必须带 `_` 通配臂，落到
  `unsupported_feature` 或定向解码错误，不 `todo!()`/`panic!`。
- 升级 `toasty-core` 或 `surrealdb` 后必须复跑集成 + e2e 测试。

## 13. 实现与文档状态

首个可用实现与 e2e 通过前，README 不得宣称该 driver 已发布可用。实施差异由
[Driver TODO](../todos/driver.md) 跟踪。
