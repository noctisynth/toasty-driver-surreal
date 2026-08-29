# 显式事务实施清单

> 状态：Implemented
> 最后核对：2026-08-30
> 需求来源：[Driver Spec](../specs/driver.md)
> 决策来源：[RFC 0003](../rfcs/0003-explicit-transactions.md)

- [x] 为 `Connection` 增加单一活动 SDK 事务与只读状态；
- [x] 实现 Start/Commit/Rollback，并结构化拒绝 isolation、非默认 mode、重复 Start 与无活动 finalize；
- [x] 把八个数据 Operation 的 query 统一路由到普通或事务句柄；
- [x] 在值编码/SDK query 前拒绝只读事务中的写 Operation；
- [x] 结构化拒绝 Savepoint/Release/RollbackToSavepoint，保持顶层状态；
- [x] 把 `QueryError::TransactionConflict` 映射为 `serialization_failure`；
- [x] 增加 kv-mem 与 kv-surrealkv 专用事务测试及错误边界测试；
- [x] 更新 README，明确显式事务与普通 batch 的原子性边界；
- [x] 运行完整质量门禁；
- [x] 用 Semifold CLI 创建独立 changeset 并确认发布计划。
