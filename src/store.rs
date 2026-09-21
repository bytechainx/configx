//! 分层配置存储门面。

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use serde::de::DeserializeOwned;

use crate::config::ConfigxConfig;
use crate::error::{ConfigxError, ConfigxResult};
use crate::layered::LayeredConfig;
use crate::secret::RedactedEntries;
use crate::source::ConfigSource;
use crate::watch::{ConfigSubscription, ConfigWatch};

/// 存储的运行时状态摘要。
///
/// 由 [`ConfigxStore::health_check`] 返回。健康语义是「至少有一个已注册且成功加载的源」，
/// 与键的数量无关——一个源可以合法地加载出空映射。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigxHealth {
    /// 最近一次成功 `reload` 时成功加载的源数量。
    pub sources: usize,
    /// 当前快照中的键数量。
    pub keys: usize,
    /// 是否至少有一个已成功加载的源。
    pub healthy: bool,
}

/// 分层配置存储门面。
///
/// # 语义
///
/// - **层序**：源按注册顺序合并，后注册者优先级更高（同键覆盖先注册者）；
/// - **原子替换**：[`reload`](Self::reload) 先完整加载并校验全部源，再整体替换快照，
///   因此读取方只会看到「旧快照」或「新快照」，不会看到部分提交；
/// - **失败保持**：源加载失败、键校验失败、空快照被拒或通知失败时，
///   快照与 generation 都保持原样；
/// - **键不做规范化**：键按原样存储与查询，大小写敏感，只拒绝空键、控制字符与超长键；
/// - **读取不脱敏**：[`get`](Self::get) 始终返回原始值，脱敏只作用于 `Debug`/日志路径。
///
/// # 可变性
///
/// [`get`](Self::get) 返回借用自存储的 `&str`，因此所有会替换快照的操作
/// （[`reload`](Self::reload)、[`register_source`](Self::register_source)）都要求 `&mut self`。
/// 这对应「单写多读」模型：读取可以并发，重载需要独占。
///
/// # Examples
///
/// ```
/// use configx::{ConfigxStore, MemorySource};
///
/// let mut store = ConfigxStore::new();
/// store.register_source(MemorySource::from_pairs([("app.host", "db.local")]));
/// store.reload()?;
/// assert_eq!(store.get("app.host"), Some("db.local"));
/// assert_eq!(store.health_check()?.sources, 1);
/// # Ok::<(), configx::ConfigxError>(())
/// ```
#[derive(Default)]
pub struct ConfigxStore {
    config: ConfigxConfig,
    layered: LayeredConfig,
    snapshot: Arc<BTreeMap<String, String>>,
    loaded_sources: usize,
    watch: Arc<ConfigWatch>,
}

impl ConfigxStore {
    /// 创建空存储：无源、空快照、使用 [`ConfigxConfig::default`]。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 按给定配置创建空存储。
    ///
    /// # Errors
    ///
    /// 配置未通过 [`ConfigxConfig::validate`] 时返回 [`ConfigxError::Invalid`]。
    pub fn from_config(config: ConfigxConfig) -> ConfigxResult<Self> {
        config.validate()?;
        let mut store = Self::new();
        store.config = config;
        Ok(store)
    }

    /// 追加一个配置源；该源成为当前最高优先级层。
    pub fn register_source(&mut self, source: impl ConfigSource + 'static) {
        self.layered.push(Arc::new(source));
    }

    /// 追加一个已装箱的配置源（用于共享或 `dyn` 场景）。
    pub fn register_shared_source(&mut self, source: Arc<dyn ConfigSource>) {
        self.layered.push(source);
    }

    /// 链式追加配置源。
    #[must_use]
    pub fn with_source(mut self, source: impl ConfigSource + 'static) -> Self {
        self.register_source(source);
        self
    }

    /// 当前配置。
    #[must_use]
    pub fn config(&self) -> &ConfigxConfig {
        &self.config
    }

    /// 已注册的源数量（不代表已成功加载）。
    #[must_use]
    pub fn source_count(&self) -> usize {
        self.layered.len()
    }

    /// 按层序重新加载全部源，成功后整体替换快照并广播一次变更。
    ///
    /// # 提交顺序
    ///
    /// 先推进 generation，再替换快照。由于本方法需要 `&mut self`，调用期间不存在并发读者，
    /// 因此两种顺序对安全代码等价；而失败路径（源加载/校验失败、空快照被拒、通知失败）
    /// 都发生在替换之前，旧快照与旧 generation 完整保留。
    ///
    /// # Errors
    ///
    /// - 任一源加载失败或键非法时返回对应错误；
    /// - 合并结果为空且 [`ConfigxConfig::allow_empty_snapshot`] 为 `false` 时返回
    ///   [`ConfigxError::Conflict`]；
    /// - 变更总线的锁中毒、已关闭或 generation 溢出时返回 [`ConfigxError::Conflict`]。
    pub fn reload(&mut self) -> ConfigxResult<()> {
        let merged = self.layered.load_merged()?;
        if merged.is_empty() && !self.config.allow_empty_snapshot {
            return Err(ConfigxError::conflict(
                "不允许把配置替换为空快照（allow_empty_snapshot=false）",
            ));
        }
        self.watch.notify()?;
        self.snapshot = Arc::new(merged);
        self.loaded_sources = self.layered.len();
        Ok(())
    }

    /// 按 key 查询配置值。
    ///
    /// 返回 [`None`] 当且仅当 key 不存在。返回值始终是原始值，与脱敏开关无关。
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.snapshot.get(key).map(String::as_str)
    }

    /// key 是否存在。
    #[must_use]
    pub fn contains_key(&self, key: &str) -> bool {
        self.snapshot.contains_key(key)
    }

    /// 当前快照的键数量。
    #[must_use]
    pub fn len(&self) -> usize {
        self.snapshot.len()
    }

    /// 当前快照是否为空。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.snapshot.is_empty()
    }

    /// 查询并把值反序列化为 `T`。
    ///
    /// 值先按 JSON 解析；失败时再按「JSON 字符串字面量」解析一次，
    /// 因此 `8080` 可以转成 `u16`，而 `db.local` 可以转成 `String`。
    ///
    /// # Errors
    ///
    /// - key 不存在时返回 [`ConfigxError::Missing`]；
    /// - 值无法转换为 `T` 时返回 [`ConfigxError::TypeMismatch`]。
    ///
    /// 错误消息只包含键名与目标类型名，**不回显配置值**，避免敏感值进入日志。
    pub fn get_typed<T: DeserializeOwned>(&self, key: &str) -> ConfigxResult<T> {
        let raw = self
            .get(key)
            .ok_or_else(|| ConfigxError::missing(format!("配置键不存在：{key}")))?;
        if let Ok(value) = serde_json::from_str::<T>(raw) {
            return Ok(value);
        }
        serde_json::from_value::<T>(serde_json::Value::String(raw.to_string())).map_err(|_| {
            ConfigxError::type_mismatch(format!(
                "配置键 {key} 的值无法转换为类型 {}",
                std::any::type_name::<T>()
            ))
        })
    }

    /// 拍摄当前快照。
    ///
    /// 返回的 [`Arc`] 是不可变视图：后续 [`reload`](Self::reload) 会替换存储持有的
    /// `Arc`，但已取得的快照内容保持不变。脱敏不作用于本方法。
    #[must_use]
    pub fn snapshot(&self) -> Arc<BTreeMap<String, String>> {
        Arc::clone(&self.snapshot)
    }

    /// 健康检查：至少有一个源已成功加载时返回 `Ok(())`。
    ///
    /// # Errors
    ///
    /// 没有任何已成功加载的源时返回 [`ConfigxError::Unavailable`]（可重试：
    /// 注册源并 [`reload`](Self::reload) 之后即可恢复）。
    pub fn ping(&self) -> ConfigxResult<()> {
        let health = self.health_check()?;
        if health.healthy {
            return Ok(());
        }
        Err(ConfigxError::unavailable(format!(
            "没有已成功加载的配置源（已注册 {} 个，已加载 {} 个）",
            self.layered.len(),
            self.loaded_sources
        )))
    }

    /// 健康检查：返回结构化状态，不因不健康而报错。
    ///
    /// # Errors
    ///
    /// 当前实现不产生错误；返回 `Result` 以保持与其他适配器一致的调用形状。
    pub fn health_check(&self) -> ConfigxResult<ConfigxHealth> {
        Ok(ConfigxHealth {
            sources: self.loaded_sources,
            keys: self.snapshot.len(),
            healthy: self.loaded_sources > 0,
        })
    }

    /// 订阅变更通知。
    #[must_use]
    pub fn watch(&self) -> ConfigSubscription {
        self.watch.subscribe()
    }

    /// 订阅变更通知（[`watch`](Self::watch) 的别名）。
    #[must_use]
    pub fn subscribe(&self) -> ConfigSubscription {
        self.watch()
    }

    /// 取得变更总线句柄，用于手动 [`notify`](ConfigWatch::notify)/[`close`](ConfigWatch::close)。
    #[must_use]
    pub fn notifier(&self) -> Arc<ConfigWatch> {
        Arc::clone(&self.watch)
    }

    /// 当前变更 generation。
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.watch.generation()
    }
}

impl fmt::Debug for ConfigxStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = f.debug_struct("ConfigxStore");
        debug
            .field("sources", &self.layered.len())
            .field("loaded_sources", &self.loaded_sources)
            .field("generation", &self.watch.generation());
        // 展示路径才脱敏；redact_secrets=false 时输出原始值，仅用于本地排查。
        if self.config.redact_secrets {
            debug.field("entries", &RedactedEntries(self.snapshot.as_ref()));
        } else {
            debug.field("entries", &self.snapshot);
        }
        debug.finish()
    }
}
