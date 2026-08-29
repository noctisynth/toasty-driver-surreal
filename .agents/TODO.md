# TODO 索引

> 状态：Active（当前无未完成实施项）
> 作用：全项目实施清单入口
> 最后核对：2026-08-30

任务必须来自 [Driver Spec](specs/driver.md) 或已接受 RFC（当前为
[RFC 0001](rfcs/0001-surrealdb-driver.md)、[RFC 0002](rfcs/0002-surrealkv-engine.md)、
[RFC 0003](rfcs/0003-explicit-transactions.md)）。
本索引与 `todos/*.md` 只记录规范与实现之间的差异。

## 实施清单

| 范围 | 清单 | 状态 | 当前重点 |
|---|---|---|---|
| SurrealDB Driver | [todos/driver.md](todos/driver.md) | Implemented（首阶段） | 无；后续范围需先更新 Spec/RFC 与实施清单 |
| SurrealKV 引擎 | [todos/surrealkv.md](todos/surrealkv.md) | Implemented | feature、构造器、持久化 e2e 与 CI |
| 显式事务 | [todos/transactions.md](todos/transactions.md) | Implemented | 顶层生命周期、只读保护、冲突分类与专用测试 |

## 使用规则

- 先从 [设计索引](DESIGN.md) 定位 Active Spec 或已接受 RFC，再进入实施清单。
- 完成项必须同时满足设计、实现和必要验证。
