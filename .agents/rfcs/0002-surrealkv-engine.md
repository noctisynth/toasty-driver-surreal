# RFC 0002：嵌入式 SurrealKV 引擎

> 状态：Accepted
> 接受日期：2026-08-30
> 设计入口：[设计索引](../DESIGN.md)
> 落地规范：[Driver Active Spec](../specs/driver.md)
> 验证证据：[SurrealKV Spike](../spikes/surrealkv-3.2.4.md)

## 1. 问题

首阶段只暴露内存与 RocksDB 引擎，遗漏了 SurrealDB 自有的嵌入式 SurrealKV。`surrealdb 3.2.4`
已经通过 `kv-surrealkv` feature 提供 `engine::local::SurrealKv`，且其连接类型与现有引擎同为
`Surreal<Db>`，因此可以在不改变 Operation 翻译和公共类型边界的前提下补齐。

## 2. 目标与非目标

目标：

- 在现有 `surrealdb` 生产依赖上启用 `kv-surrealkv`；
- 暴露 `SurrealDb::surrealkv(path)`；
- 保持 namespace/database、连接复用、reset 与持久化语义和 RocksDB 一致；
- 用独立 e2e 验证 CRUD、查询与跨重开持久化。

非目标：

- 不替换或弃用 RocksDB；
- 不暴露 SurrealKV 的 versioning、sync、vlog 等高级配置；
- 不把事务支持绑定到某一个存储引擎，事务由独立 RFC 规定。

## 3. 候选方案

| 方案 | 结论 | 原因 |
|---|---|---|
| 默认启用 `kv-surrealkv` | 接受 | SurrealKV 是原生文件引擎且不编译 librocksdb；构造器可始终可用，Cargo CLI 也能直接维护单一 SDK 依赖 |
| 用 `Any` URL 统一所有本地引擎 | 拒绝 | 会扩大公共/内部抽象并引入字符串配置，不如现有枚举的编译期 feature 清晰 |
| 可选 feature + 专用构造器 | 拒绝 | Cargo 不允许用可选别名重复依赖同一个 `surrealdb` 包；为 feature 开关增加 shim crate 得不偿失 |

## 4. 已接受设计

现有 `surrealdb` 依赖同时启用 `kv-mem` 与 `kv-surrealkv`。公共接口新增：

```rust
pub fn surrealkv(path: impl Into<std::path::PathBuf>) -> Self;
```

内部 `Engine` 新增 `SurrealKv(PathBuf)`。连接使用
`Surreal::new::<surrealdb::engine::local::SurrealKv>(path)`；`url()` 返回
`surrealdb:surrealkv:<path>`；`max_connections()` 返回 `None`；`reset_db()` 在释放缓存句柄后删除
数据目录。

SurrealKV 与 RocksDB 可同时编译和使用。两者使用相同的值编解码、RecordId、SurrealQL 与错误
分类，不新增 SurrealDB 类型到公共 API。

## 5. 依赖与安全

仍锁定 `surrealdb 3.2.4`；该版本的 `kv-surrealkv` 解析到兼容的 `surrealkv 0.21.x`。SurrealKV
为 Apache-2.0，已有 license allowlist 覆盖。项目不声明 MSRV；启用 feature 时实际 Rust 下限仍受
上游 `surrealkv` 约束。

## 6. 验收标准

1. `SurrealDb::surrealkv(path)` 能构建并完成 push_schema；
2. CRUD 与过滤扫描通过；
3. 数据库关闭并重开后数据仍存在；
4. `.e2e-data/surrealkv-*` 始终在 gitignore 范围；
5. 默认构建包含 SurrealKV；同时启用 `rocksdb` 时通过 check；
6. fmt、clippy、doc、默认测试与 SurrealKV e2e 通过。
