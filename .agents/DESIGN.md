# 设计索引

> 状态：Active
> 更新日期：2026-08-30
> 作用：`toasty-driver-surreal` 的权威工程设计入口

本仓库为 [Toasty ORM](https://github.com/tokio-rs/toasty) 实现一个 out-of-tree 的 SurrealDB
driver。工程事实按成熟度分为已生效的 `specs/`、决策记录 `rfcs/`、实现差异 `todos/` 和技术
验证证据 `spikes/`。面向使用者的入口文档为根目录 `README.md`；后续扩展文档放在 `docs/`。

## 1. 项目方向

把 SurrealDB 作为 Toasty 的 **KV/文档后端**接入：实现 `toasty_core::driver::{Driver,
Connection}`，把引擎下发的键值 `Operation` 翻译为 SurrealQL，用官方 `surrealdb` 嵌入式 SDK
执行。不做新的 SQL 方言，不 fork Toasty 主仓库。

## 2. 分层与依赖方向

```text
用户应用
   │  Db::builder().build(SurrealDb::…)
   ▼
toasty (用户 API + 查询引擎)      —— 上游已发布 crate，不修改
   │  下发 KV Operation（sql=None 路径）与显式事务生命周期
   ▼
toasty-driver-surreal            —— 本仓库
   │  Operation → SurrealQL + 值编解码
   ▼
surrealdb 嵌入式 SDK (kv-mem / kv-rocksdb / kv-surrealkv)
```

依赖只从上指向下。driver 以 `toasty-core` 与 `surrealdb` 为核心生产依赖，并用 `serde_json` 完成
Toasty native JSON 文本与 SurrealDB native value 的边界转换；不依赖 `toasty`、`toasty-sql`。

## 3. 权威工程文档

| 文档 | 类型与状态 | 权威范围 |
|---|---|---|
| [Driver Spec](specs/driver.md) | Active Spec（首阶段已实现）| 公共接口、能力画像、值编解码、记录 ID 映射、Operation→SurrealQL、schema、错误、测试门禁 |
| [RFC 0001：SurrealDB Driver](rfcs/0001-surrealdb-driver.md) | Accepted RFC | KV/文档归类、SDK 选择、PK 映射、依赖方向、被拒方案 |
| [RFC 0002：SurrealKV 引擎](rfcs/0002-surrealkv-engine.md) | Accepted RFC | SurrealKV SDK feature、构造器、持久化与验证边界 |
| [RFC 0003：显式事务](rfcs/0003-explicit-transactions.md) | Accepted RFC | 顶层事务生命周期、只读保护、错误分类及非目标 |
| [RFC 0004：原生 JSON 列](rfcs/0004-native-json.md) | Accepted RFC | `db::Type::Json` 列感知 codec、NULL 语义、能力与测试边界 |
| [RFC 0005：迁移跟踪与自动生成](rfcs/0005-migrations.md) | Accepted RFC | tracking 表、原子应用、schema diff→SurrealQL 与人工迁移门禁 |
| [SurrealDB SDK Spike](spikes/surrealdb-sdk-3.2.4.md) | Completed Spike | SDK 行为证据、`type::record` 坑、值往返、版本锁定依据 |
| [SurrealKV Spike](spikes/surrealkv-3.2.4.md) | Completed Spike | SurrealKV 编译、连接、持久化与客户端事务共存证据 |
| [事务 Spike](spikes/transactions-3.2.4.md) | Completed Spike | SDK commit/cancel、隔离、持久化、savepoint 与 Toasty 路由证据 |
| [原生 JSON Spike](spikes/native-json-3.2.4.md) | Completed Spike | Toasty JSON wire 契约、SDK native value 与共享测试限制 |
| [迁移 Spike](spikes/migrations-3.2.4.md) | Completed Spike | Toasty migration 契约、事务化 DDL/tracking、no-op 与完整 u64 ID |

实现前先在本索引定位 Active Spec 或已接受 RFC，再从 [TODO 索引](TODO.md) 进入实施清单。

## 4. 当前状态与后续顺序

首阶段已按 Driver Spec 完成：Driver/Connection、八个 Operation、值编解码、schema 推送、
kv-mem 共享集成测试、RocksDB e2e 与质量门禁均已落地。当前没有未完成的首阶段实施项，详见
[Driver TODO](todos/driver.md)。

SurrealKV 引擎扩展已由 RFC 0002 接受，并已按 [SurrealKV TODO](todos/surrealkv.md) 落地。

显式顶层事务已由 RFC 0003 接受，并已按 [事务 TODO](todos/transactions.md) 落地；driver 继续保持
`Capability.sql = None`，普通 Toasty batch 不因此宣称自动原子化。

原生 JSON 列已由 RFC 0004 接受，并已按 [原生 JSON TODO](todos/native-json.md) 落地。该能力只
开放 `native_json`，不把 SurrealDB 的单一 JSON 值模型冒充为 JSONB。

迁移跟踪与自动生成已由 RFC 0005 接受，并已按 [迁移 TODO](todos/migrations.md) 落地。安全的
表/索引/SCHEMALESS 字段变化自动生成 SurrealQL；表 rename、PK 变化与字段类型转换保留人工门禁。

远程引擎、嵌套事务/savepoint、图边、live query、URL scheme 注册与裸 SurrealQL 均属于后续范围。
开始其中任何一项前，先按本文件第 5 节更新设计/RFC/Spec 与实施清单，再修改实现和测试。

## 5. 变更规则

- 已生效契约变化：先更新 [Driver Spec](specs/driver.md)，再更新 [Driver TODO](todos/driver.md)
  与实现。
- 新的架构提案：先进入 `rfcs/`；本仓库无人工门禁，RFC 由实现方自证后接受。
- 跨层责任/依赖变化：先更新本索引，再更新受影响 Spec/RFC。
- 不得先改实现再补 Spec/RFC；重要决策不得只藏在代码里。
