# configx Agent 指南

> 本文件为 AI Agent 在本仓库工作时的入口指南。

## 项目定位

分层配置存储：把「内存 / 环境变量 / `KEY=VALUE` 文件」等配置源按注册顺序合并成一份不可变快照，对外提供读取、类型化读取、差异比较、变更订阅与密钥脱敏。

## 技术栈

- Rust edition 2021, rust-version 1.75
- 关键依赖: `thiserror`、`serde`、`serde_json`、`toml`
- 纯同步 crate（变更通知基于 `Condvar`），不引入异步运行时
- 零内部耦合，不依赖 kernel/contracts 等私有 crate

## 代码结构

```text
src/
├── lib.rs      # 入口：模块声明 + 受控 re-export
├── config.rs   # ConfigxConfig / ConfigxConfigBuilder + validate
├── diff.rs     # diff_snapshots / ConfigDiff
├── error.rs    # ConfigxError / ConfigxResult / ErrorKind
├── key.rs      # 键校验（空键、控制字符、512 字节上限）
├── layered.rs  # LayeredConfig 多层合并（后注册源覆盖先注册源）
├── secret.rs   # is_secret_key / redact_value / redact_map 脱敏纯函数
├── source.rs   # ConfigSource trait + MemorySource / EnvSource / FileSource
├── store.rs    # ConfigxStore 存储门面 + ConfigxHealth
├── view.rs     # subset_snapshot / try_subset_snapshot / snapshots_agree
└── watch.rs    # ConfigWatch / ConfigSubscription / ConfigChange

tests/          # config_validation.rs · layered_store_watch.rs · public_api.rs
benches/        # hot_path.rs（harness = false，离线基准）
docs/           # API.md · 标准.md
```

## 设计约定（改代码前必读）

- **单写多读**：读取用 `&self` 且可并发；替换快照（`reload` / `register_source`）需要 `&mut self`。
- **不做键规范化**：键大小写敏感、按原样存储；只拒绝空键、控制字符与超过 512 字节的键。
- **失败不改状态**：源加载或校验失败时，快照与变更序号保持原样。
- **读取永不脱敏**：脱敏只作用于 `Debug`/日志路径，`ConfigxStore::get` 始终返回原始值。
- **非目标**：类型化 schema 推导、分布式配置中心、远端 secret manager、自动文件监听——不要往这些方向加代码。

## 开发约定

- 注释与文档使用简体中文；标识符保持英文
- 错误类型：thiserror 枚举 + `#[non_exhaustive]` + `pub type ConfigxResult<T>`
- 配置：`ConfigxConfig` 结构体 + `builder()`/`from_env()`/`from_toml()` + `validate()` + fail-fast
- 禁止裸 `unwrap()`（库代码）/ 无注释 `expect()`
- `#![forbid(unsafe_code)]`、`#![deny(missing_docs)]` 已开启：新增 pub 项必须带中文 `///` 文档
- 导出集中在 `lib.rs`，禁止 glob re-export

## 门禁三件套（P0）

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

基准（离线，可选）：

```bash
cargo bench --bench hot_path             # 完整 50_000 次迭代
cargo bench --bench hot_path -- --quick  # 快速 1_000 次迭代
```

## 相关文档

- 组织 Rust 规范：`~/org-config/rulesets/rust/RULES.md`
- API 文档：`docs/API.md`
- 标准与验收：`docs/标准.md`
- 术语与领域语言：`CONTEXT.md`
- 贡献指南：`CONTRIBUTING.md`
- 变更记录：`CHANGELOG.md`
- 基准测试：`benches/hot_path.rs`
