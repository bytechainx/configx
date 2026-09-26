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
//! | 配置源 | [`ConfigSource`]、[`MemorySource`]、[`EnvSource`]、[`FileSource`]、[`GlobalFileSource`]、[`parse_key_value_file`]、[`resolve_global_file_path`] |
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
//! - **异步互操作**：所有公开 API 均为同步阻塞实现。若在 tokio 异步上下文中调用，
//!   必须用 `tokio::task::spawn_blocking` 隔离，否则会冻结运行时工作线程。
//!   详见 [`ConfigSource::load`]、[`ConfigSubscription::wait_timeout_outcome`] 等
//!   方法的文档中的 `# 阻塞调用` 小节。
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
    ConfigxConfig, ConfigxConfigBuilder, ENV_ALLOW_EMPTY_SNAPSHOT, ENV_GLOBAL_FILE,
    ENV_REDACT_SECRETS,
};
pub use diff::{diff_snapshots, ConfigDiff};
pub use error::{ConfigxError, ConfigxResult, ErrorKind};
pub use layered::LayeredConfig;
pub use secret::{is_secret_key, redact_map, redact_value, REDACTED_VALUE, SECRET_KEY_PREFIX};
pub use source::{
    parse_key_value_file, resolve_global_file_path, resolve_global_file_path_from_env,
    ConfigSource, EnvSource, FileSource, GlobalFileSource, MemorySource,
};
pub use store::{ConfigxHealth, ConfigxStore};
pub use view::{snapshots_agree, subset_snapshot, try_subset_snapshot};
pub use watch::{ConfigChange, ConfigSubscription, ConfigWaitOutcome, ConfigWatch};

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable
    )]

    #[test]
    fn public_reexports_compose_through_the_crate_root() {
        // 顶层再导出必须可用：以 crate 根路径组装一次最小工作流（不触碰进程环境）。
        let mut store = crate::ConfigxStore::new();
        store.register_source(crate::MemorySource::from_pairs([
            ("app.port", "5432"),
            ("secret:token", "top-secret"),
        ]));
        store.reload().expect("reload 成功");

        assert_eq!(store.get("app.port"), Some("5432"));
        assert_eq!(store.get_typed::<u16>("app.port").expect("类型读取"), 5432);
        store.ping().expect("已加载源必须健康");

        let subset = crate::try_subset_snapshot(&store, &["app.port"]).expect("子集合法");
        assert!(crate::subset_snapshot(&store, &["app.port"]).contains_key("app.port"));
        assert!(crate::snapshots_agree(
            &subset,
            &store.snapshot(),
            &["app.port"]
        ));
        assert!(
            !crate::diff_snapshots(&subset, &store.snapshot()).is_empty(),
            "子集相对完整快照在被排除的键上应有差异"
        );
        let full = crate::subset_snapshot(&store, &["app.port", "secret:token"]);
        assert!(
            crate::diff_snapshots(&full, &store.snapshot()).is_empty(),
            "覆盖全部键时差异必须为空"
        );

        assert_eq!(
            crate::redact_value("secret:token", "v"),
            crate::REDACTED_VALUE
        );
        assert!(crate::is_secret_key(crate::SECRET_KEY_PREFIX));
        assert_eq!(
            crate::ErrorKind::Invalid,
            crate::ConfigxError::invalid("x").kind()
        );
        assert!(crate::parse_key_value_file("A=1\n")
            .expect("解析成功")
            .contains_key("A"));
        assert_eq!(
            crate::ConfigxConfig::default(),
            crate::ConfigxConfig::builder().build().expect("构建成功")
        );

        // 类型别名与其余公开类型可从根路径命名。
        let _: crate::ConfigxResult<()> = Ok(());
        let _: crate::ConfigDiff = crate::ConfigDiff::default();
        let _: crate::LayeredConfig = crate::LayeredConfig::new();
        let _: crate::ConfigxHealth = store.health_check().expect("health_check 不报错");
        let _: crate::ConfigChange = crate::ConfigChange { generation: 0 };
        assert_eq!(store.generation(), 1);
    }
}
