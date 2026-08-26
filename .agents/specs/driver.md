# Toasty SurrealDB Driver 规范

> 状态：Active Spec
> 规范基线：2026-08-26
> 适用范围：`toasty-driver-surreal` crate
> 设计入口：[设计索引](../DESIGN.md)
> 决策来源：[RFC 0001：Toasty SurrealDB Driver](../rfcs/0001-surrealdb-driver.md)
> 验证证据：[SurrealDB SDK Spike](../spikes/surrealdb-sdk-3.2.4.md)
> 实施清单：[Driver TODO](../todos/driver.md)

本文冻结 driver 的公共接口、能力画像、值编解码、记录 ID 映射、每个 `Operation` 的 SurrealQL
翻译、schema 推送、错误分类和测试门禁。没有被本文纳入的行为（事务、远程引擎、图边、迁移
生成）不得由实现自行补全。

## 1. 范围

### 1.1 首阶段目标

- 实现 `toasty_core::driver::{Driver, Connection}`，作为 KV/文档 driver（`sql = None`）。
- 支持嵌入式引擎：内存（`kv-mem`）与文件 RocksDB（`kv-rocksdb`）。
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
  `3.2.4`，features `kv-mem`、`kv-rocksdb`）、`async-trait`、`tracing`。
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

    /// 设置 namespace 与 database，默认 "toasty" / "toasty"。
    pub fn namespace(self, ns: impl Into<String>) -> Self;
    pub fn database(self, db: impl Into<String>) -> Self;
}
```

- 首阶段用 `Db::builder().models(..).build(SurrealDb::mem())` 接入，不经 `connect(url)`。
- `Driver::url()` 返回信息性字符串：内存 `surrealdb:mem`，文件 `surrealdb:rocksdb:<path>`。
  出现在日志中的 URL 不含凭证（首阶段嵌入式无凭证）。

### 3.1 连接与共享实例

- driver 内部缓存一个共享 `Surreal<Db>` 句柄（对标 Turso 的 `Arc<Mutex<Option<..>>>`），
  所有 `connect()` 复用同一底层库，保证内存库在连接池多 slot 间可见。
- `connect()` 返回的 `Connection` 持有该句柄克隆并已 `use_ns/use_db`。
- `reset_db()`：内存引擎丢弃缓存句柄；文件引擎删除数据目录后重建。
- `max_connections()`：内存引擎返回 `Some(1)`（与 in-memory SQLite 一致，避免多 slot 各开空
  库），文件引擎返回 `None`。

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
        scan_supports_sort: true,
        index_or_predicate: true,
        upsert_primary_key: true,
        primary_key_ne_predicate: true,
        ..Capability::DYNAMODB
    };
    &CAP
}
```

- **必须**用 FRU；新增字段自动继承 DYNAMODB。
- 其余位（`native_json`、`vec_*`、`document_collections`、日期/decimal 等）首阶段随 DYNAMODB
  基线；由集成测试逐位收敛，调整时同步本节。
- `validate()` 必须通过（`sql`/`sql_placeholder` 同为 None；`native_varchar=false` 且
  `varchar=None`，DYNAMODB 基线已满足）。

## 5. 记录 ID 与主键映射

- 记录 ID 一律用 `type::record($tb, $id)` 构造（**禁止** `type::thing`，3.x 已移除）。
- 主键列取自 `table.primary_key_columns()`（按序）。映射规则：

| 主键形态 | `stmt::Value` | SurrealDB record id key |
|---|---|---|
| 单列整数 | `I8..I64`/`U8..U64` | `RecordIdKey::Number(i64)` |
| 单列字符串 | `String` | `RecordIdKey::String` |
| 单列 UUID | `Uuid` | `RecordIdKey::Uuid` |
| 复合（≥2 列） | `Record([..])` | `RecordIdKey::Array`（元素按列序编码为 surreal `Value`） |

- 引擎按键操作传入的 `keys: Vec<stmt::Value>`：单列时元素即标量；复合时元素为
  `Value::Record`。driver 用上表构造 record id。
- 读取行时，SurrealDB 返回的行含 `id` 字段（native `RecordId`）。driver 按 `op.select` 的列
  顺序取值：主键列从 `RecordId` 反解，其余列从 object 字段取；缺失字段填 `Value::Null`。
- 无符号整数超过 `i64::MAX` 的映射遵循 DYNAMODB 基线（`max_unsigned_integer` 未限制，但
  SurrealDB `Number::Int` 是 i64；`U64` 溢出应返回 `unsupported_feature` 或按字符串编码，
  首阶段以整数为主，溢出留待测试暴露后处理）。

## 6. 值编解码

实现独立模块 `value.rs`，提供 `stmt::Value → surrealdb::types::Value`（写入/绑定）与
`surrealdb::types::Value + stmt::Type → stmt::Value`（读取解码）。**不经 JSON 中转。**

| `stmt::Value` | `surrealdb::types::Value` |
|---|---|
| `Null` | `Null`（或 `None`，读取时二者都归为 `Null`） |
| `Bool` | `Bool` |
| `I8/I16/I32/I64` | `Number(Number::Int(i64))` |
| `U8/U16/U32` | `Number(Number::Int(i64))` |
| `U64` | `Number(Number::Int(i64))`（溢出 → `unsupported_feature`） |
| `F32/F64` | `Number(Number::Float(f64))` |
| `String` | `String` |
| `Bytes` | `Bytes` |
| `Uuid` | `Uuid` |
| `List(items)` | `Array` |
| `Record(items)` | `Array`（用于复合键；行记录用 object） |
| `Object(fields)` | `Object`（`#[document]` 列） |

读取解码按目标 `stmt::Type` 定向（对标 SQLite `from_sql`）：`Number::Int` 依 `Type::{I8..U64,
Bool}` 收窄；`Number::Float` 依 `Type::{F32,F64}`；`String` 依 `Type::{Uuid, String, ...}`；
`RecordId` → 依主键列类型反解为标量或 `Record`。未覆盖的 `Value`/`Type` 组合返回
`Error::driver_operation_failed`（或 `unsupported_feature`），不 panic。

可选 crate 特性 `jiff`/`rust_decimal`/`net` 首阶段不启用；若启用则 `Datetime`↔`jiff`、
`Number::Decimal`↔`rust_decimal`、网络类型走字符串，需另加测试。

## 7. Operation → SurrealQL 翻译

所有标量值以绑定参数（`.bind((name, value))`）下发，name 用稳定生成规则（如 `$p0,$p1,…`）；
记录 ID 用 `type::record($tb,$id)`。列投影用列名列表；`SELECT *` 仅在需要全列时使用。

### 7.1 Insert

- 语句：`CREATE type::record($tb,$id) CONTENT $data`。
- `$data` 为 object，键为列名，值为编码后 surreal `Value`；主键列不放入 CONTENT（由 record id
  承担），其余非空列放入。多行 insert 循环执行（或多语句），逐行构造。
- 返回：`op.ret` 为 `None` 时返回 `ExecResponse::count(n)`；`Some(types)` 时 `RETURN AFTER` 并
  解码投影行。
- 冲突：SurrealDB 报 `already exists` → 映射为唯一冲突错误（见 §9）。

### 7.2 GetByKey

- 语句：`SELECT <cols> FROM type::record($tb,$id0), type::record($tb,$id1), …`（多键一条查询）
  或逐键查询。
- 未命中键不产生行（不报错）。返回 `ValueStream`，每行按 `op.select` 解码。

### 7.3 QueryPk

- 语句：`SELECT <cols> FROM <tb> WHERE <pk_filter> [AND <filter>] [ORDER BY <sort>] [LIMIT n]
  [START m]`。
- `op.index` 为 `Some` 时改为按索引字段过滤（SurrealDB 索引对查询透明，`WHERE` 引用索引列
  即可，`FindPkByIndex` 亦然）。
- `op.pk_filter` 与 `op.filter` 经 `surreal_expression()` 翻译（§8）。
- `op.order`：单向 `ORDER BY <col> ASC|DESC`。
- 分页 `Pagination`：`Offset{limit,offset}` → `LIMIT limit START offset`；`Cursor{page_size,
  after}` → 首阶段用 keyset 谓词或 `LIMIT/START` 近似，游标以最后一行主键编码；
  `backward_pagination` 依 DYNAMODB 基线为 `false`。

### 7.4 FindPkByIndex

- 语句：`SELECT <pk cols> FROM <tb> WHERE <index filter>`。
- 返回主键值行，供引擎后续 `GetByKey` 使用。

### 7.5 Scan

- 语句：`SELECT <cols> FROM <tb> [WHERE <filter>] [LIMIT/START]`。
- `op.columns` 为列下标（相对表列表），映射为列名投影。

### 7.6 UpdateByKey

- 引擎已把多键更新拆成每键一次，故 `op.keys` 恒为单键。
- 语句：`UPDATE type::record($tb,$id) SET <assignments> [WHERE <filter>] [RETURN AFTER]`。
- `op.assignments`：`Set(expr)` → `col = $v`（`Null` → `col = NONE` 或 `UNSET col`）；
  `Add/Subtract` → `col = col + $v` / `col = col - $v`。`Append/Remove/Pop/RemoveAt` 由
  `vec_*` 能力位（DYNAMODB 基线为 false）在引擎侧拦截，未拦截到则返回 `unsupported_feature`。
- `op.filter`：附加 `WHERE`。`op.condition`：前置条件失败应报错而非静默跳过（首阶段可用
  `WHERE` 近似 + 返回行数判定；条件失败映射为 `Error::condition_failed`）。
- `op.returning`：`Some(cols)` → `RETURN AFTER` 并解码这些列；`None` → 返回受影响计数。

### 7.7 DeleteByKey

- 单键。语句：`DELETE type::record($tb,$id) [WHERE <filter>] [RETURN BEFORE]`。
- 纯 `filter` 未命中 → count 0；`condition` 失败 → `condition_failed`。

### 7.8 Upsert

- 语句：`UPSERT type::record($tb,$id) CONTENT $data RETURN AFTER`（或按 `on_conflict` 用
  `MERGE`）。`op.stmt` 携带已降级的 insert + upsert 目标；`upsert_primary_key=true` 允许主键
  目标。`upsert_unique`/`upsert_branch_assignments` 依 DYNAMODB 基线（false）。
- 返回：按 `op.ret` 解码 `AFTER` 行或不返回。

## 8. 过滤表达式翻译（`surreal_expression`）

对标 DynamoDB 的 `ddb_expression()`：递归把 `stmt::Expr` 翻译为 SurrealQL 布尔表达式，标量
以绑定参数下发。至少支持：

- `BinaryOp`：`Eq/Ne/Gt/Ge/Lt/Le` → `=/!=/>/>=/</<=`；`Add/Sub` 仅在赋值上下文。
- `And`/`Or`（`Or` 用括号包裹）/`Not`。
- `Reference`（列引用 → 列名；bool 裸列 → `col = true`）。
- `Value` → 绑定参数。
- `IsNull` → `col IS NONE`（SurrealDB 用 `NONE`/`NULL`；以 `IS NONE` 为主，读取时 `Null`
  归一）。
- `InList` → `col IN [$v0,$v1,…]`。
- `StartsWith` → 首阶段可用 `string::starts_with(col, $p)`（需测试验证）；未验证前
  `native_starts_with=false`，由引擎降级路径处理。
- 未覆盖的 `Expr` 变体返回 `Error::unsupported_feature`（**带 `_` 通配臂**，不 panic）。

列引用通过 `ExprContext`（`toasty_core::stmt::ExprContext::new_with_target`）解析为列，取
`column.name`。

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

### 11.3 单元测试

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
