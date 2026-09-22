# Changelog — configx

本文件记录 `configx` 的用户可见变更，遵循 [Keep a Changelog](https://keepachangelog.com/)
与 [Semantic Versioning](https://semver.org/)。

本仓库代码自 `xhyper.rs` 的 `crates/infra/configx` 抽取而来（抽取时点为 `0.1.5`）。
该工程内的版本线不在本文件中延续，本仓库从 `0.1.0` 重新起算。

## [Unreleased]

## [0.1.1] - 2026-09-23

### 修正

- **错误消息不再回显配置值**（`ConfigxConfig::from_toml`）：原先直接透传 `toml` 的类型
  错误文本，会把非法**值**内联进消息（实测
  `invalid type: string "…", expected a boolean`），违反 `src/error.rs` 声明的
  「所有变体的消息都不得回显配置值」；`Display` 版本还会额外渲染源码行。
  现改为只报告位置（`第 N 行第 M 列`），不透传底层文本、不渲染源码片段。
  属**「实现向契约靠拢」**的 PATCH 修复，无公开 API 变更。
  （由 `tests/aidd_boundary.rs::error_messages_never_echo_values` 补上 TOML 路径的
  回显断言后先红后绿逼出。）

### 新增

- 三类测试（特性 002）：`tests/tdd_contracts.rs`（逐公开入口的行为契约与变异探测红绿）、
  `tests/sdd_spec.rs`（与 `docs/标准.md` 章节 1:1 的规格断言）、
  `tests/aidd_boundary.rs`（键边界 / 控制字符 / 并发 / 错误不回显等对抗用例）；
  均为离线用例，不引入新的运行时依赖。

## [0.1.0] - 2026-09-21

### 新增

- 存储门面 `ConfigxStore`：`register_source` / `register_shared_source` / `with_source` /
  `reload` / `get` / `get_typed` / `contains_key` / `len` / `snapshot` / `ping` /
  `health_check` / `watch` / `subscribe` / `notifier` / `generation` 首次以独立 crate 形式提供。
- 多层合并 `LayeredConfig`：源按注册顺序合并，后注册源覆盖先注册源，先注册源独有的键保留。
- 配置源 `ConfigSource`（trait）与内置实现 `MemorySource` / `EnvSource` / `FileSource`，
  以及 `KEY=VALUE` 文件解析函数 `parse_key_value_file`。
- 变更通知 `ConfigWatch` / `ConfigSubscription` / `ConfigChange` / `ConfigWaitOutcome`：
  基于 `Condvar` 的同步阻塞等待、限时等待与关闭语义，返回
  `Changed` / `TimedOut` / `Closed`。
- 快照视图 `diff_snapshots` / `ConfigDiff` / `subset_snapshot` / `try_subset_snapshot` /
  `snapshots_agree`；键差集、交集与新增项均按键升序排列。
- 密钥脱敏纯函数 `is_secret_key` / `redact_value` / `redact_map`，以及常量
  `SECRET_KEY_PREFIX` 与 `REDACTED_VALUE`。
- 配置面 `ConfigxConfig` / `ConfigxConfigBuilder`：`from_env()`（前缀
  `FOUNDATIONX_CONFIGX_`）/ `from_toml()` / `validate()` / `builder()`，
  开关为 `redact_secrets` 与 `allow_empty_snapshot`。

### 变更

- **解耦**：错误模型从主工程的 `kernel::XError` 下沉为 crate 内 `src/error.rs` 的
  `ConfigxError` / `ConfigxResult` / `ErrorKind`，`Cargo.toml` 不再声明任何内部 crate 依赖。
- **破坏性变更**：存储门面由源模块的 `ConfigStore` 改为 `ConfigxStore`，快照改为不可变的
  `Arc<BTreeMap<String, String>>`，并以单调递增的 `generation` 支撑订阅判断。
- **破坏性变更**：移除源模块的 `ConfigSnapshot` 与 `require_keys` / `require_nonempty` /
  `validate_key` / `set_checked` / `merge_into` / `store_from_pairs` 等公开函数；
  键校验下沉为 crate 内 `src/key.rs`。
- **破坏性变更**：移除 `SecretString` 与 `get_secret` / `set_secret` / `try_get_secret`，
  脱敏收敛为纯函数 `is_secret_key` / `redact_value` / `redact_map`，不再提供「秘密类型」包装。
- 配置面为本仓库新增（源模块没有 `ConfigxConfig` 这一层）：环境变量前缀
  `FOUNDATIONX_CONFIGX_`，开关为 `redact_secrets` 与 `allow_empty_snapshot`。

### 说明

- **纯同步**：变更通知基于 `Condvar`，不引入异步运行时、不启动后台线程或文件 watcher；
  变更需要调用方显式 `reload` 触发。
- **读取永不脱敏**：`ConfigxStore::get` 始终返回原始值，脱敏只作用于 `Debug` / 日志路径。
- **键不做规范化**：大小写敏感、按原样存储；只拒绝空键、含控制字符的键与超过 512 字节的键。
- **失败不改状态**：源加载或校验失败时，快照与变更序号保持原样。
- 非目标：类型化 schema 推导、分布式配置中心、远端 secret manager、自动文件监听。
- 本 crate **不发布到 crates.io**，仅以 GitHub 源码 / git 依赖形式复用，安装方式见 `README.md`。
