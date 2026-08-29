# SurrealDB 3.2.4 原生 JSON Spike

> 状态：Completed
> 日期：2026-08-30
> SDK：`surrealdb = 3.2.4`
> Toasty：`toasty = 0.10.0`

## 验证问题

1. Toasty 的 native JSON 在 Driver 边界是什么 `stmt::Value` 与列类型；
2. SurrealDB SDK 能否绑定和返回原生 JSON object/array/scalar/null；
3. 数据库空值与 JSON literal `null` 能否保持区分；
4. 共享 integration suite 是否能直接覆盖 KV driver 的 native JSON 路径。

## Toasty 契约检查

`toasty 0.10.0/src/stmt/json.rs` 显示 `toasty::Json<T>` 与 `serde_json::Value` 的逻辑类型都是
`stmt::Type::String`，setter 先用 `serde_json::to_string` 生成 `stmt::Value::String`；显式
`#[column(type = json)]` 让 `Column.storage_ty` 成为 `db::Type::Json`。`Option<Json<T>>::None` 在进入
inner `Load` 前由 `Option<T>` 拦截为 Toasty `Value::Null`，`Json<Option<T>>::None` 则以非空字符串
`"null"` 进入 JSON 反序列化。

Toasty 的 KV `prepare_for_driver` 不抽取 typed params，Insert 包装里保留 inline value，更新走
`UpdateByKey`。因此 Driver 必须从 schema column 而不是 `TypedValue` 获得 storage type。

## SDK 值检查

`surrealdb-types 3.2.4` 的 `Value` 公开 `None`、`Null`、`Bool`、`Number`、`String`、`Array` 与
`Object`，现有 `#[document]` 测试已经验证这些 native 值可通过 kv-mem、SurrealKV 与 RocksDB
绑定并往返。该 crate 同时为 `serde_json::Value` 实现 `SurrealValue` 并提供
`Value::into_json_value()`，证明 SDK 官方支持 JSON 树转换。

Driver 不直接采用 SDK 的全量 `into_json_value()` 作为读取策略，因为它会把 `None` 与 `Null` 都
折叠成 JSON null，并会把 JSON 不允许的 SurrealDB 类型做 best-effort 字符串化。列感知 codec 先
单独处理 `None`，再仅接受 JSON-compatible 变体，能保持 Toasty 的空值与错误语义。

## 共享测试检查

`toasty-driver-integration-suite 0.10.0/src/tests/type_serialize.rs` 的四个
`requires(native_json)` 用例运行时行为覆盖合理，但日志断言无条件要求 SQL `QuerySql`、`Expr::Arg`
与 typed params。该假设和 `Capability.sql = None` 的 KV 路由冲突，需要 driver 专用回归测试；不能
为了复用断言把本 driver 改成 SQL driver。

## 实现后探针

列感知 codec 落地后，driver-owned kv-mem 测试实际通过 `serde_json::Value`、`Json<T>`、数据库空值
与 JSON null、UpdateByKey、Upsert 和整值谓词；SurrealKV 与 RocksDB e2e 均完成 object/array/null
文件引擎往返。codec 单元探针还验证了顶层 bool/string/number、`u64::MAX`、object 内部 null 与非法
JSON 的安全错误路径。

## 结论

- SurrealDB 3.2.4 的 runtime value 足以实现 Toasty native JSON；
- codec 必须由 `Column.storage_ty == Json` 驱动，不能只看 `stmt::Type::String`；
- `Value::None` 表示数据库空值，`Value::Null` 表示 JSON literal null；
- 首期只开放 `native_json`，不开放 `native_jsonb`；
- schema 可继续保持 SCHEMALESS，字段类型强制属于独立设计范围。
