# 原生 JSON 实施清单

> 状态：Implemented
> 最后核对：2026-08-30
> 需求来源：[Driver Spec](../specs/driver.md)
> 决策来源：[RFC 0004](../rfcs/0004-native-json.md)

- [x] 通过 Cargo CLI 增加 `serde_json` 生产依赖并审查 manifest/lockfile；
- [x] 实现 `db::Type::Json` 列感知写入 codec，保持数据库空值与 JSON null 区分；
- [x] 实现 JSON-compatible SurrealDB native value 到规范 JSON 文本的读取 codec；
- [x] 把列元数据传播到 Insert、UpdateByKey、Upsert、表达式绑定与结果解码；
- [x] 设置 `Capability.native_json = true`，保持 `native_jsonb = false`；
- [x] 增加 kv-mem 专用测试，覆盖 `Json<T>`、`serde_json::Value`、null、更新、upsert 与谓词；
- [x] 增加 SurrealKV/RocksDB 文件引擎原生 JSON 往返验证；
- [x] 记录共享 suite 的 SQL-only native JSON 日志断言限制；
- [x] 更新 README 支持矩阵与用法示例；
- [x] 运行完整质量门禁；
- [x] 用 Semifold CLI 创建独立 changeset并确认发布计划。
