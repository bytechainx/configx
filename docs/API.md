# configx 公开 API

**版本 / 角色**：`configx 0.1.0` · 分层配置存储（多源合并 → 不可变快照 + 差异比较 + 变更订阅 + 密钥脱敏）

## 公开消费面

| 面 | 类型 / 函数 | 说明 |
|----|-------------|------|
| 存储门面 | `ConfigxStore`、`ConfigxHealth` | `new` / `from_config` / `register_source` / `register_shared_source` / `with_source` / `reload` / `get` / `get_typed` / `contains_key` / `len` / `snapshot` / `ping` / `health_check` / `watch` / `subscribe` / `notifier` / `generation` |
| 配置与构建器 | `ConfigxConfig`、`ConfigxConfigBuilder` | `from_env` / `from_toml` / `validate` / `builder`；开关：`redact_secrets`、`allow_empty_snapshot` |
| 配置源 | `ConfigSource`（trait）、`MemorySource`、`EnvSource`、`FileSource`、`parse_key_value_file` | 内存键值对 / 带前缀的环境变量 / `KEY=VALUE` 文件 |
| 多层合并 | `LayeredConfig` | 后注册源覆盖先注册源 |
| 变更通知 | `ConfigWatch`、`ConfigSubscription`、`ConfigChange`、`ConfigWaitOutcome` | 基于 `Condvar` 的同步订阅，无异步运行时 |
| 快照视图 | `diff_snapshots`、`ConfigDiff`、`subset_snapshot`、`try_subset_snapshot`、`snapshots_agree` | 差异比较与子集视图 |
| 错误 | `ConfigxError`、`ConfigxResult`、`ErrorKind` | thiserror 枚举 |
| 脱敏（纯函数） | `is_secret_key`、`redact_value`、`redact_map`、`REDACTED_VALUE`、`SECRET_KEY_PREFIX` | 仅作用于展示路径 |

## 最小用法

```rust
use configx::{ConfigxStore, EnvSource, MemorySource};

// 低优先级：内置默认值；高优先级：环境变量（后注册者覆盖前者）
let mut store = ConfigxStore::new();
store.register_source(MemorySource::from_pairs([("app.host", "db.local"), ("app.port", "5432")]));
store.register_source(EnvSource::new("APP_"));
store.reload()?;

assert_eq!(store.get("app.host"), Some("db.local"));
assert_eq!(store.get_typed::<u16>("app.port")?, 5432);
assert!(store.ping().is_ok());
# Ok::<(), configx::ConfigxError>(())
```

## 语义要点

- **单写多读**：读取用 `&self` 且可并发；替换快照（`reload` / `register_source`）需要 `&mut self`。
- **键不做规范化**：大小写敏感、按原样存储；只拒绝空键、控制字符与超过 512 字节的键。
- **失败不改状态**：源加载或校验失败时，快照与变更序号保持原样。
- **读取永不脱敏**：`get` 始终返回原始值；脱敏只发生在 `Debug`/日志路径。
- **差异有序**：`diff_snapshots` 的三个列表按键升序排列（输入为 `BTreeMap`）。

## 能力边界

本 crate 提供进程内分层配置合并与订阅；**不提供**类型化 schema 推导、分布式配置中心、远端 secret manager、自动文件监听（file watcher）。变更通知需要调用方显式 `reload` 触发。
