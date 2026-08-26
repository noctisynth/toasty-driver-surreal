# SurrealDB SDK + Toasty crate 可行性 Spike

> 日期：2026-08-26
> 结论：通过
> SDK 依赖：`surrealdb = "3.2.4"`（features `kv-mem`、`kv-rocksdb`）；当前 crates.io
> `max_stable_version = 3.2.4`，`3.3.0-beta.*` 为预发布，不采用
> Toasty 依赖：`toasty-core = "0.10"`（driver 契约）、dev 用 `toasty = "0.10"` 与
> `toasty-driver-integration-suite = "0.10"`
> 证据脚本：`spike-tmp/src/main.rs`（临时 crate，已 gitignore；本文件保留结论）

本 Spike 在冻结驱动契约前，验证 SurrealDB 官方 Rust SDK（embedded engine）能否支撑把 Toasty
的 KV/文档 `Operation` 翻译成 SurrealQL 的全部设计假设，并确认 `surrealdb` 与已发布的 Toasty
crate 可以在同一依赖图内解析。所有 21 个探针均通过。

## 1. 运行环境

- 本机 `surreal` CLI：`3.2.3`（仅参考；驱动使用嵌入式 SDK，不依赖外部 server）。
- Rust：`1.98.0`（本项目不锁定 MSRV）。
- crates.io 可达；`surrealdb 3.2.4` 与 `toasty-core/toasty/toasty-driver-integration-suite 0.10`
  的 `cargo add --dry-run` 均成功解析，无版本冲突。

## 2. SDK 关键事实（探针证据）

### 2.1 引擎与连接

- `Surreal::new::<Mem>(())` 打开进程内内存库；`Surreal::new::<RocksDb>(path)` 打开嵌入式
  RocksDB 文件库。二者都要求随后 `use_ns(..).use_db(..)`。
- RocksDB 目录可指向工作目录下的 `.e2e-data/`（已 gitignore），供 e2e 测试使用。

### 2.2 公共类型路径（重要）

- 值类型是 `surrealdb::types::Value`（**不是** `surrealdb::Value`，根路径无 `Value` 导出）。
  `surrealdb-types` 从 crate root 重导出 `Value`、`Number`、`RecordId`、`RecordIdKey`、
  `Object`、`Array`、`Datetime`、`Uuid`、`Bytes` 等。
- `Value` 变体：`None | Null | Bool | Number(Number) | String | Bytes | Duration | Datetime |
  Uuid | Geometry | Table | RecordId | File | Range | Regex | Array | Object | Set`。
- `Number` 变体：`Int(i64) | Float(f64) | Decimal(rust_decimal::Decimal)`。
- `RecordId { table: Table, key: RecordIdKey }`；`RecordIdKey` 变体：
  `Number(i64) | String | Uuid | Array(Array) | Object(Object) | Range(Box<..>)`。

### 2.3 记录 ID 构造 —— 关键坑

- **SurrealDB 3.x 用 `type::record(tb, id)` 构造记录 ID，`type::thing` 已被移除**（会报
  `Invalid function/constant path, did you maybe mean type::record`）。这是 2.x→3.x 的破坏性
  变化，驱动必须使用 `type::record`。
- `id` 可绑定为字符串、整数或数组（复合键）。回显：`user:alice`、`item:42`、
  `pair:['us-east', 7]`。数组键即天然的复合主键载体。

### 2.4 CRUD → SurrealQL 映射（全部验证通过）

| Toasty Operation | SurrealQL | 证据 |
|---|---|---|
| `Insert` | `CREATE type::record($tb,$id) CONTENT $data` | 冲突时报 `Database record user:alice already exists`（可作唯一冲突分类） |
| `GetByKey` | `SELECT <cols> FROM type::record($tb,$id)` | 命中返回行；未命中返回**空数组而非错误** |
| `QueryPk`/`Scan`/`FindPkByIndex` | `SELECT <cols> FROM <tb> WHERE <expr> ORDER BY .. LIMIT .. START ..` | 投影/过滤/排序/分页均可 |
| `UpdateByKey` | `UPDATE type::record($tb,$id) MERGE $patch RETURN AFTER` | 返回更新后行 |
| `Upsert` | `UPSERT type::record($tb,$id) CONTENT $data RETURN AFTER` | 原生 UPSERT 可用（2.x+） |
| `DeleteByKey` | `DELETE type::record($tb,$id) [RETURN BEFORE]` | 返回删除前行 |

- 绑定参数用 `.bind((name, value))`，支持 `serde_json::Value` 与原生 surreal 类型。
- `DEFINE INDEX <name> ON TABLE <tb> COLUMNS <cols>` 建二级索引后，按索引字段 `WHERE` 查询正常。
- `.check()` 用于只需成功/失败、不取行的语句。

### 2.5 结果解码（驱动 decode 路径）

- `Response::take(i)` 可按语句序号取第 i 条结果，支持多语句查询（`stmt0`/`stmt1` 分别取回）。
- 解码目标：`SurValue`（native `Value`）或任意 `R: SurrealValue`（含 `Vec<serde_json::Value>`）。
  **不能**直接对任意 `#[derive(serde::Deserialize)]` 用户结构体 `take` —— 需要 `SurrealValue`
  bound。驱动应把行解码为 native `Value` 或 `serde_json::Value`，再自行映射到 `stmt::Value`。
- ID 在 JSON 投影里回显为 `"table:key"` 字符串（如 `"user:alice"`）；在 native `Value` 里是
  `RecordId` 结构。驱动做 PK↔record-id 映射时应基于 native `RecordId`，避免解析字符串。

### 2.6 类型 round-trip

- `bool / i64（含 >2^53 的 9007199254740993）/ f64 / UTF-8（héllo）` 原样往返。
- 原生 `Uuid` / `Datetime` / `Bytes` 绑定后，native `Value` 分别回显为 `Uuid/Datetime/Bytes`；
  转 JSON 时 `Bytes` 变为 `[1,2,3,4]` 数组、`Datetime` 变 RFC3339 字符串、`Uuid` 变字符串。
  → 结论：**值编解码走 native `Value` 保真度最高**，JSON 路径对 bytes/datetime 有歧义。

### 2.7 错误面

- 解析错误、表不存在、记录已存在等都作为 `surrealdb::Error` 返回，`Display` 文本稳定可读
  （如 `Database record user:alice already exists`）。驱动可据此分类唯一冲突。

## 3. 对驱动设计的影响

1. **归类确认**：走 KV/文档路径（`Capability.sql = None`），以 DynamoDB 驱动为模板；不涉及
   `toasty-sql` / `Dialect`。SurrealQL 由驱动内部拼装，与三种内置方言无关。
2. **必须用 `type::record`**（非 `type::thing`），否则 3.x 全部记录 ID 操作失败。
3. **值编解码以 native `surrealdb::types::Value` 为准**，实现 `stmt::Value ↔ Value` 双向映射；
   避免走 JSON 丢失 bytes/datetime 语义。
4. **公共类型从 `surrealdb::types` 取**，不要用 `surrealdb::Value`。
5. **PK ↔ RecordId**：单列键 → `RecordIdKey::{Number,String,Uuid}`；复合键 → `Array`。基于
   native `RecordId` 双向转换，读取投影时用 native 解码而非解析 `"tb:key"`。
6. **能力位**：`scan=true`、`scan_supports_sort=true`、`index_or_predicate=true`、
   `upsert_primary_key=true` 可开（均由 SurrealQL 原生支持），优于 DynamoDB 的保守取值。
7. **未命中/冲突语义**：GetByKey 未命中→空；Insert 冲突→错误可分类为唯一冲突。

## 4. 已知限制

- 本 Spike 仅覆盖嵌入式 `kv-mem` 与 `kv-rocksdb`；远程 `ws://`/`http://` 引擎未验证（首阶段
  以 build(driver) + 嵌入式为准，不做 URL scheme 注册）。
- 未验证事务：Toasty KV 路径默认不向驱动下发 `Operation::Transaction`（引擎按 `capability().sql()`
  门控），首阶段驱动可拒绝事务，与 DynamoDB 一致。
- 复合/对象记录 ID 的排序与游标反向分页语义尚未逐一压测，留待 Spec 与集成测试细化。
- `surrealdb 3.2.4` 为当前稳定版；3.3 尚在 beta。锁定 3.2.x，升级前需复跑本 Spike。

## 5. 结论

SurrealDB 官方 SDK 完全支撑 KV/文档驱动所需的 create/get/update/delete/upsert/scan/index 全部
操作与值往返；`surrealdb 3.2.4` 与 Toasty `0.10` 系列可共存解析。可以进入 RFC 与 Spec。
