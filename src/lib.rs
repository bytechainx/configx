#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(unreachable_pub)]

//! # `configx` — 分层配置存储
//!
//! 线程安全的分层配置存储：把「内存 / 环境变量 / `KEY=VALUE` 文件」等配置源按注册顺序合并成
//! 一份不可变快照，对外提供读取、类型化读取、差异比较、变更订阅与密钥脱敏。
//!
//! ## 能力一览
//!
//! | 面 | 类型 / 函数 |
//! |----|-------------|
//! | 存储门面 | [`ConfigxStore`]、[`ConfigxHealth`] |
//! | 配置与构建器 | [`ConfigxConfig`]、[`ConfigxConfigBuilder`] |
//! | 配置源 | [`ConfigSource`]、[`MemorySource`]、[`EnvSource`]、[`FileSource`]、[`parse_key_value_file`] |
//! | 多层合并 | [`LayeredConfig`]（后注册源覆盖先注册源） |
//! | 变更通知 | [`ConfigWatch`]、[`ConfigSubscription`]、[`ConfigChange`]、[`ConfigWaitOutcome`] |
//! | 快照视图 | [`diff_snapshots`]、[`ConfigDiff`]、[`subset_snapshot`]、[`try_subset_snapshot`]、[`snapshots_agree`] |
//! | 错误 | [`ConfigxError`]、[`ConfigxResult`]、[`ErrorKind`] |
//! | 脱敏（纯函数） | [`is_secret_key`]、[`redact_value`]、[`redact_map`] |
//!
//! ## 最小示例
//!
//! ```
//! use configx::{ConfigxStore, EnvSource, MemorySource};
//!
//! // 低优先级：内置默认值；高优先级：环境变量（后注册者覆盖前者）
//! let mut store = ConfigxStore::new();
//! store.register_source(MemorySource::from_pairs([("app.host", "db.local"), ("app.port", "5432")]));
//! store.register_source(EnvSource::new("APP_"));
//! store.reload()?;
//!
//! assert_eq!(store.get("app.host"), Some("db.local"));
//! assert_eq!(store.get_typed::<u16>("app.port")?, 5432);
//! assert!(store.ping().is_ok());
//! # Ok::<(), configx::ConfigxError>(())
//! ```
//!
//! ## 设计约定
//!
//! - **单写多读**：读取用 `&self` 且可并发；替换快照（`reload` / `register_source`）需要 `&mut self`。
//! - **不做键规范化**：键大小写敏感、按原样存储；只拒绝空键、控制字符与超过 512 字节的键。
//! - **失败不改状态**：源加载或校验失败时，快照与变更序号保持原样。
//! - **读取永不脱敏**：脱敏只作用于 `Debug`/日志路径，[`ConfigxStore::get`] 始终返回原始值。
//! - **纯同步**：变更通知基于 `Condvar`，不引入异步运行时，也不启动自动文件 watcher。
//!
//! ## 非目标
//!
//! 类型化 schema 推导、分布式配置中心、远端 secret manager、自动文件监听。

mod config;
mod diff;
mod error;
mod key;
mod layered;
mod secret;
mod source;
mod store;
mod view;
mod watch;

pub use config::{
    ConfigxConfig, ConfigxConfigBuilder, DEFAULT_WATCH_CHANNEL_CAPACITY, ENV_ALLOW_EMPTY_SNAPSHOT,
    ENV_REDACT_SECRETS, ENV_WATCH_CHANNEL_CAPACITY, MAX_WATCH_CHANNEL_CAPACITY,
};
pub use diff::{diff_snapshots, ConfigDiff};
pub use error::{ConfigxError, ConfigxResult, ErrorKind};
pub use layered::LayeredConfig;
pub use secret::{is_secret_key, redact_map, redact_value, REDACTED_VALUE, SECRET_KEY_PREFIX};
pub use source::{parse_key_value_file, ConfigSource, EnvSource, FileSource, MemorySource};
pub use store::{ConfigxHealth, ConfigxStore};
pub use view::{snapshots_agree, subset_snapshot, try_subset_snapshot};
pub use watch::{ConfigChange, ConfigSubscription, ConfigWaitOutcome, ConfigWatch};
