# toasty-driver-surreal Agent 协作约定

本文件适用于本仓库的全部目录和任务。所有后续 Agent 在分析、设计、实现、测试和交付工作时都
必须遵守以下约定。本仓库为 [Toasty ORM](https://github.com/tokio-rs/toasty) 实现一个 out-of-tree
的 SurrealDB driver。

## 0. 工作方式

- **不锁定 MSRV**：不声明也不校验最低 Rust 版本。
- **无人工审阅门禁**：开发过程中不需要人工确认，Agent 自行推进 RFC → Spec → 实现 → 测试。
  只有在完成全部 TODO 后才向用户汇报结果、遇到的问题与技术方案变更。
- **RFC 由 Agent 自行接受**：本仓库为单一目标任务，RFC 的 Accepted 状态不需要外部确认，但
  仍必须先写 RFC 再据其写 Spec 与实现，保留可追溯的设计顺序。

## 1. 权威文档与实施清单

- 设计入口：[.agents/DESIGN.md](.agents/DESIGN.md)
- Driver Active Spec：[.agents/specs/driver.md](.agents/specs/driver.md)
- Driver RFC：[.agents/rfcs/0001-surrealdb-driver.md](.agents/rfcs/0001-surrealdb-driver.md)
- RFC 工作流：[.agents/rfcs/README.md](.agents/rfcs/README.md)
- 实施清单索引：[.agents/TODO.md](.agents/TODO.md)
- Driver 实施清单：[.agents/todos/driver.md](.agents/todos/driver.md)
- 技术验证证据：[.agents/spikes/](.agents/spikes/)

`.agents/DESIGN.md` 负责设计索引与跨层边界；具体契约以 Active Spec 或已接受 RFC 为技术事实
来源。`.agents/TODO.md` 与 `.agents/todos/*.md` 只记录规范与实现之间的差异，不是独立需求来源。
面向使用者的文档放在 `docs/`。

- 实现前先从 `.agents/DESIGN.md` 定位对应 Active Spec 或已接受 RFC。
- 只有代码和必要验证均满足 Active Spec 或已接受 RFC 后，才勾选对应实施项。
- 不得让实施清单、代码或测试与 Active Spec 或已接受 RFC 冲突。

## 2. 设计优先与同步顺序

出现新的架构决定、公共契约变化、依赖选择或范围变化时，按以下顺序处理：

1. 跨层变化先更新 `.agents/DESIGN.md`；
2. 尚未确认的新决策先进入 RFC，接受后同步受影响的 Active Spec；
3. 已确认的契约变化更新对应 Active Spec；
4. 更新 `.agents/todos/` 中的实施清单；
5. 修改代码、配置、测试、示例和受影响的用户文档。

- 不得先实现方案变化，再补写 Spec 或 RFC。
- 不得把重要设计决定只隐藏在代码、测试或依赖中。

## 3. Driver 架构边界

- 本 driver 实现 `toasty_core::driver::{Driver, Connection}`，作为 KV/文档驱动
  （`Capability.sql = None`），以 in-tree 的 DynamoDB driver 为结构蓝本。
- 不实现新的 SQL 方言，不依赖 `toasty-sql`；SurrealQL 语句由 driver 内部拼装。
- `surrealdb` crate 类型不得穿透 driver 的公共 API 边界（值、错误、连接 URL 之外）。
- `toasty-core` 的未封闭枚举（`Capability`、`Operation`、`stmt::Value`、`stmt::Type`）：构造
  `Capability` 必须用 `..Capability::DYNAMODB` 式 FRU；匹配这些枚举必须带 `_` 通配臂，以对冲
  上游 SemVer 加变体的风险。

## 4. Rust 错误处理与 panic 约束

- 生产 Rust 代码不得使用可能 panic 的 `unwrap()`。
- I/O、解析、配置、网络、SurrealDB 响应等可恢复失败必须返回结构化错误，转换为
  `toasty_core::Error` 的恰当变体（`driver_operation_failed`、`connection_lost`、
  `serialization_failure`、`unsupported_feature`、`invalid_connection_url`）。
- 只有类型系统或同函数内穷尽分支已证明不变量时才允许 `expect()`，且消息须具体。
- 测试代码可用 `unwrap()`/`expect()` 表达断言与前置条件。
- 未实现的 `Operation` 分支返回 `unsupported_feature`，不得 `todo!()`/`panic!` 逃逸到用户。

## 5. Cargo manifest 与依赖管理

- 除非用户明确要求直接编辑，否则不得手动编辑 `Cargo.toml`。
- 新增/移除依赖、crate、feature 时使用 Cargo CLI（`cargo add`、`cargo remove`、`cargo new`）。
- 执行 Cargo CLI 后检查 manifest 与 lockfile 的实际变化。
- `surrealdb` 使用经 Spike 验证的稳定版本（当前 `3.2.4`，非预发布）；升级前先复跑 SDK Spike。
- package `version` 由 GitHub Actions 的 Semifold CI 依据 changeset 修改；不得由 Agent 手工
  编辑或用 Cargo CLI 本地调整。

## 6. Semifold changeset 与发布

- 仓库使用 Semifold（命令 `smif`/`semifold`），配置位于 `.changes/config.toml`。
- `main` 是 base branch，Semifold 管理独立的 `release` branch；stable 发布通道。
- 配置与 package 列表通过 `semifold init` / `semifold config sync` 维护；执行后审查生成内容。
- 影响 crate 行为（功能、修复、重构、依赖、测试能力）的任务用 `semifold commit` 创建 changeset；
  纯文档、CI、仓库管理可不创建。不得手工编写 changeset 绕过 CLI。
- 创建 changeset 后运行 `semifold status` 确认解析成功、发布计划符合预期。
- 本地与 Agent 环境**严禁**执行 `semifold version` 与 `semifold publish`（含 `--dry-run`）；
  版本更新、release branch 写入和发布由 GitHub Actions 的 `semifold ci` 独占。

## 7. 安全与日志

- 不得在 `Debug`、错误 `Display`、tracing、fixture 或快照中泄漏完整记录内容或绑定参数值。
- 连接 URL 中的凭证必须脱敏后才能出现在日志或错误中。
- 依赖安全由 `cargo deny`（`deny.toml`）与 gitleaks 在 CI 中把关；`deny.toml` 的 license 例外
  和 advisory ignore 必须逐条注明原因。

## 8. 测试与验证

- driver 必须接入 `toasty-driver-integration-suite` 的共享测试（`Setup` +
  `generate_driver_tests!`），跑内存引擎（`kv-mem`）。
- 端到端测试使用嵌入式 RocksDB（`rocksdb` feature），数据目录在工作目录下的 `.e2e-data/`
  （已 gitignore）。
- 交付前运行与改动相称的检查；CI 的完整门禁如下（本地可行时执行）。三个 DynamoDB 专属负向
  用例与四个硬编码 SQL Operation 外形的 native JSON 用例在测试步骤中显式 `--skip`（原因见
  `tests/mem.rs`）；native JSON 运行时行为由 `tests/native_json.rs` 覆盖。

```sh
cargo fmt --all --check
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked -- \
  --skip composite_index_too_many_range_columns \
  --skip composite_unique_index_unsupported_on_dynamodb \
  --skip starts_with_empty_prefix \
  --skip json_native_round_trip \
  --skip json_value_native_round_trip \
  --skip json_native_nulls \
  --skip json_value_native_nulls
cargo test --test e2e_rocksdb --features rocksdb --locked -- --test-threads=1
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
cargo deny check advisories licenses bans sources
```

## 9. Shell 命令

- 搜索文件或文本时优先使用专用搜索工具。
- e2e RocksDB 数据目录必须保持在 `.gitignore` 覆盖范围内，不得提交。

## 10. 任务完成与交付说明

完成全部 TODO 后，最终回复必须：

- 总结业务与技术产出；
- 明确说明本次技术方案相对任务开始时的设计发生了什么变化（若无变化，明确写明“无变动”），
  以及如何同步到 DESIGN/Spec/RFC/实施清单；
- 列出实际完成的验证，以及任何被阻塞或需要用户决定的事项。
