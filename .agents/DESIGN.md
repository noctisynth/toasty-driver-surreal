# 设计索引

> 状态：Active
> 更新日期：2026-08-26
> 作用：`toasty-driver-surreal` 的权威工程设计入口

本仓库为 [Toasty ORM](https://github.com/tokio-rs/toasty) 实现一个 out-of-tree 的 SurrealDB
driver。工程事实按成熟度分为已生效的 `specs/`、决策记录 `rfcs/`、实现差异 `todos/` 和技术
验证证据 `spikes/`。面向使用者的文档放在根目录 `docs/`。

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
   │  下发 KV Operation（sql=None 路径）
   ▼
toasty-driver-surreal            —— 本仓库
   │  Operation → SurrealQL + 值编解码
   ▼
surrealdb 嵌入式 SDK (kv-mem / kv-rocksdb)
```

依赖只从上指向下。driver 生产依赖仅 `toasty-core` 与 `surrealdb`；不依赖 `toasty`、
`toasty-sql`。

## 3. 权威工程文档

| 文档 | 类型与状态 | 权威范围 |
|---|---|---|
| [Driver Spec](specs/driver.md) | Active Spec | 公共接口、能力画像、值编解码、记录 ID 映射、Operation→SurrealQL、schema、错误、测试门禁 |
| [RFC 0001：SurrealDB Driver](rfcs/0001-surrealdb-driver.md) | Accepted RFC | KV/文档归类、SDK 选择、PK 映射、依赖方向、被拒方案 |
| [SurrealDB SDK Spike](spikes/surrealdb-sdk-3.2.4.md) | Completed Spike | SDK 行为证据、`type::record` 坑、值往返、版本锁定依据 |

实现前先在本索引定位 Active Spec 或已接受 RFC，再从 [TODO 索引](TODO.md) 进入实施清单。

## 4. 当前工作顺序

1. 按 Spec 创建 crate 并用 Cargo CLI 添加依赖；
2. 实现 Driver/Connection 骨架、能力位、值编解码；
3. 实现八个 Operation 的 SurrealQL 翻译与 push_schema；
4. 接入集成套件（kv-mem）并逐位调能力至绿；
5. 补充嵌入式 RocksDB e2e；
6. 运行质量门禁并交付。

## 5. 变更规则

- 已生效契约变化：先更新 [Driver Spec](specs/driver.md)，再更新 [Driver TODO](todos/driver.md)
  与实现。
- 新的架构提案：先进入 `rfcs/`；本仓库无人工门禁，RFC 由实现方自证后接受。
- 跨层责任/依赖变化：先更新本索引，再更新受影响 Spec/RFC。
- 不得先改实现再补 Spec/RFC；重要决策不得只藏在代码里。
