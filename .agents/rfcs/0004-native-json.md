# RFC 0004：原生 JSON 列

> 状态：Accepted
> 接受日期：2026-08-30
> 设计入口：[设计索引](../DESIGN.md)
> 落地规范：[Driver Active Spec](../specs/driver.md)
> 验证证据：[原生 JSON Spike](../spikes/native-json-3.2.4.md)

## 1. 问题

当前 driver 可以把 `#[document]` embed 编码为 SurrealDB 的 `Object` / `Array`，但能力画像仍继承
`Capability::DYNAMODB.native_json = false`，所以 Toasty 会拒绝模型中的
`#[column(type = json)]`。即使只打开能力位，现有无类型 codec 仍会把 Toasty 传来的 JSON 序列化
字符串存为 SurrealDB `String`，并在读取时把 SurrealDB `NONE` 与 `NULL` 都解码成 Toasty
`Value::Null`，无法满足原生 JSON 的空值契约。

Toasty 0.10 在 Driver 边界把 `toasty::Json<T>` 与 `serde_json::Value` 都表示为
`stmt::Value::String(<JSON text>)`，列的数据库存储类型则为 `db::Type::Json`。Driver 必须联合值与列
元数据进行编解码，不能从 `stmt::Value` 变体单独推断。

## 2. 目标与非目标

目标：

- 报告 `Capability.native_json = true`，支持 `#[column(type = json)]`；
- 让 `toasty::Json<T>` 与 `serde_json::Value` 以 SurrealDB native `Object` / `Array` / 标量存储；
- 在 Insert、UpdateByKey、Upsert、表达式绑定和结果解码中传递列存储类型；
- 严格区分数据库空值与 JSON literal `null`；
- 在 kv-mem 与文件引擎上验证创建、读取、更新、upsert、过滤及空值往返。

非目标：

- 不报告 `native_jsonb`；SurrealDB 没有 PostgreSQL 式 JSON/JSONB 双存储语义；
- 不把表改成 `SCHEMAFULL`，也不在本阶段用 `DEFINE FIELD ... TYPE ...` 强制 JSON 字段类型；
- 不新增裸 SurrealQL 或一套新的 JSON 查询 API；
- 不改变 `Capability.sql = None` 或 KV Operation 架构。

## 3. 候选方案

| 方案 | 结论 | 原因 |
|---|---|---|
| 按 `Column.storage_ty` 做列感知 native codec | 接受 | 精确对应 Toasty Driver 契约，并能区分普通字符串、数据库空值与 JSON null |
| 只打开 `native_json`，继续存字符串 | 拒绝 | 能力声明与实际存储不符，不能称为原生 JSON，也不能利用 SurrealDB 的结构化值 |
| 复用 `#[document]` 的 `Value::Object` codec | 拒绝 | `Json<T>` 在边界是序列化字符串，且 document codec 会省略 null object 字段，语义不同 |
| 同时把 `Json` 与 `Jsonb` 映射为同一实现 | 拒绝 | 会虚构 SurrealDB 不存在的 JSONB 契约，妨碍未来能力演进 |

## 4. 已接受设计

### 4.1 能力与依赖

能力画像显式设置 `native_json: true`，`native_jsonb` 继续继承/保持 `false`。生产依赖通过 Cargo CLI
加入 `serde_json`，用于验证 Toasty 提供的 JSON 文本并在 native SurrealDB 值与规范 JSON 文本之间
转换。`surrealdb` 类型仍不穿透公共 API。

### 4.2 写入语义

仅当目标列 `storage_ty == db::Type::Json` 时启用 JSON codec：

| Toasty 值 | SurrealDB 值 | 含义 |
|---|---|---|
| `stmt::Value::Null` | 字段省略或 `Value::None` | 数据库空值（`Option<Json<T>>::None`） |
| `stmt::Value::String("null")` | `Value::Null` | JSON literal `null` |
| 其它合法 JSON 字符串 | native Bool/Number/String/Array/Object | 原生 JSON 值 |

非法 JSON 或 JSON 列收到非 String/Null Toasty 值时返回 `serialization_failure`，错误不得包含完整载荷。
普通列与 `#[document]` 列继续使用现有 native codec；document object 的 null 字段省略规则不应用到
native JSON object 内部。

### 4.3 读取语义

JSON 列读取 `Value::None` 时返回 Toasty `Value::Null`；读取其余 native JSON 值（包括
`Value::Null`）时序列化为 Toasty `Value::String(<JSON text>)`，交给 `Json<T>::load` 或
`serde_json::Value::load`。非 JSON-compatible 的 SurrealDB 运行时值返回 `serialization_failure`，
不做有损字符串化。

### 4.4 元数据传播

- Insert 与 Upsert 按 `target.columns` 查出具体 `Column` 后编码源值；
- UpdateByKey 与 Upsert shared/default mutation 使用赋值投影解析出的 `Column`；
- `row_to_record` 按每个返回列的 `storage_ty` 解码；
- filter/condition 中，与列比较的 literal 从相邻列引用推导 `storage_ty` 后绑定，保证整值 JSON 比较
  不退化为字符串比较；
- RecordId、分页 cursor 和无列上下文的内部值继续使用现有无类型 codec。

### 4.5 Schema 边界

表继续定义为 `SCHEMALESS`。原生性由 runtime bound `Value` 决定：JSON 列写入 native value，而不是
SurrealDB String。数据库层字段类型约束需要单独解决 nullable、document 和迁移兼容性，不纳入本
RFC。

## 5. 测试边界

`toasty-driver-integration-suite 0.10` 的 `requires(native_json)` 用例把 insert/query/update 的日志
形态硬编码为 SQL `QuerySql` + typed params；KV driver 实际分别收到 inline-value Insert 包装、
GetByKey 和 UpdateByKey，因此这些断言无法作为本 driver 的验收依据。共享 suite 的生成开关暂保持
`native_json: false` 并记录原因；真实 Driver capability 为 `true`，由本仓库专用测试覆盖同等运行时
行为。生成开关仍设为 `native_json: true` 以校验真实 Capability，但质量门禁按测试名跳过这四个
SQL-only 断言。后续应向 Toasty 上游把这些断言按 SQL/KV 分支拆开。

## 6. 验收标准

1. 使用 `toasty::Json<T>` 和 `serde_json::Value` 的 `#[column(type = json)]` 模型可 build/push schema；
2. object、array、string、bool、number 与 JSON null 原生往返；
3. `Option<Json<T>>::None` 与 `Json<Option<T>>::None` 读取结果保持不同；
4. Insert、UpdateByKey、Upsert 与 JSON 整值谓词均使用列感知 codec；
5. JSON object 内部的 null 字段不被 document codec 省略；
6. 非法 JSON 返回 `serialization_failure` 且错误不泄漏载荷；
7. `native_json = true`、`native_jsonb = false`，`Capability::validate()` 通过；
8. fmt、check、clippy、默认测试、文件引擎 e2e、rustdoc 与 cargo-deny 门禁通过；
9. 用 Semifold CLI 创建独立 changeset 并确认发布计划。
