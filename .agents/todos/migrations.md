# 迁移跟踪与自动生成实施清单

> 状态：Implemented
> 最后核对：2026-08-30
> 需求来源：[Driver Spec](../specs/driver.md)
> 决策来源：[RFC 0005](../rfcs/0005-migrations.md)

- [x] 提取 table/index SurrealQL renderer，由 `push_schema` 与 migration generation 共享；
- [x] 实现安全 schema diff 的自动 migration generation 与无物理变化 no-op；
- [x] 为表 rename、PK 变化、字段类型转换与未知 diff 生成非 panic 的人工迁移门禁；
- [x] 实现 `__toasty_migrations` 初始化与完整 `u64` ID 解码；
- [x] 在独立 SDK 客户端事务内原子执行 migration statements 与 tracking CREATE；
- [x] 增加 generation 单元测试和 kv-mem apply/rollback/tracking 集成测试；
- [x] 增加 SurrealKV tracking 持久化验证；
- [x] 通过 Cargo CLI 为现有 `toasty` dev-dependency 启用 `migration` feature，验证
  `MigrationSet::apply()` 首次应用与重复跳过；
- [x] 更新 README 的迁移用法、支持矩阵和人工迁移边界；
- [x] 运行完整质量门禁；
- [x] 用 Semifold CLI 创建独立 changeset并确认发布计划。
