//! 存储配置与构建器。

use std::env;

use crate::error::{ConfigxError, ConfigxResult};

/// 是否启用密钥脱敏（布尔）的环境变量名。
pub const ENV_REDACT_SECRETS: &str = "FOUNDATIONX_CONFIGX_REDACT_SECRETS";
/// watch 通道容量（非负整数）的环境变量名。
pub const ENV_WATCH_CHANNEL_CAPACITY: &str = "FOUNDATIONX_CONFIGX_WATCH_CHANNEL_CAPACITY";
/// 是否允许空快照（布尔）的环境变量名。
pub const ENV_ALLOW_EMPTY_SNAPSHOT: &str = "FOUNDATIONX_CONFIGX_ALLOW_EMPTY_SNAPSHOT";

/// watch 通道容量的默认值。
pub const DEFAULT_WATCH_CHANNEL_CAPACITY: usize = 64;

/// watch 通道容量的上限。
pub const MAX_WATCH_CHANNEL_CAPACITY: usize = 65_536;

fn default_redact_secrets() -> bool {
    true
}

fn default_watch_channel_capacity() -> usize {
    DEFAULT_WATCH_CHANNEL_CAPACITY
}

fn default_allow_empty_snapshot() -> bool {
    true
}

/// `configx` 存储配置。
///
/// 三个字段都带默认值，因此 TOML / 环境变量都可以只覆盖其中一部分。
/// 字段本身不做语义推导，全部约束集中在 [`validate`](Self::validate)。
///
/// # 示例
///
/// ```
/// use configx::ConfigxConfig;
///
/// let config = ConfigxConfig::builder().watch_channel_capacity(8).build()?;
/// assert_eq!(config.watch_channel_capacity, 8);
/// assert!(config.redact_secrets);
/// # Ok::<(), configx::ConfigxError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ConfigxConfig {
    /// 是否在 `Debug` / 日志等展示路径上对敏感键脱敏。默认 `true`。
    ///
    /// 关闭后 `Debug` 会输出原始值，只应在本地排查时使用；读取接口不受影响。
    #[serde(default = "default_redact_secrets")]
    pub redact_secrets: bool,
    /// 变更通知通道容量。默认 [`DEFAULT_WATCH_CHANNEL_CAPACITY`]。
    ///
    /// 当前实现的通知总线不缓冲历史变更（只传播 generation），该值用于校验订阅端预留容量，
    /// 必须在 `1..=`[`MAX_WATCH_CHANNEL_CAPACITY`] 之间。
    #[serde(default = "default_watch_channel_capacity")]
    pub watch_channel_capacity: usize,
    /// 是否允许 `reload` 把存储替换成空快照。默认 `true`。
    ///
    /// 设为 `false` 时，合并结果为空的 `reload` 会返回 [`ConfigxError::Conflict`]，
    /// 旧快照保持不变——用于防止「源暂时返回空内容」把线上配置清空。
    #[serde(default = "default_allow_empty_snapshot")]
    pub allow_empty_snapshot: bool,
}

impl Default for ConfigxConfig {
    fn default() -> Self {
        Self {
            redact_secrets: default_redact_secrets(),
            watch_channel_capacity: default_watch_channel_capacity(),
            allow_empty_snapshot: default_allow_empty_snapshot(),
        }
    }
}

impl ConfigxConfig {
    /// 从环境变量加载配置并覆盖默认值。
    ///
    /// 只识别前缀为 `FOUNDATIONX_CONFIGX_` 的变量：
    /// [`ENV_REDACT_SECRETS`]、[`ENV_WATCH_CHANNEL_CAPACITY`]、[`ENV_ALLOW_EMPTY_SNAPSHOT`]。
    /// 变量未设置或只含空白时使用默认值；布尔值接受 `1/0`、`true/false`、`yes/no`、`on/off`（忽略大小写）。
    ///
    /// # Errors
    ///
    /// 变量值不是合法布尔值/非负整数、不是有效 Unicode，或最终配置未通过
    /// [`validate`](Self::validate) 时返回错误。
    pub fn from_env() -> ConfigxResult<Self> {
        let mut config = Self::default();
        if let Some(value) = read_env(ENV_REDACT_SECRETS)? {
            config.redact_secrets = parse_bool(ENV_REDACT_SECRETS, &value)?;
        }
        if let Some(value) = read_env(ENV_WATCH_CHANNEL_CAPACITY)? {
            config.watch_channel_capacity = parse_usize(ENV_WATCH_CHANNEL_CAPACITY, &value)?;
        }
        if let Some(value) = read_env(ENV_ALLOW_EMPTY_SNAPSHOT)? {
            config.allow_empty_snapshot = parse_bool(ENV_ALLOW_EMPTY_SNAPSHOT, &value)?;
        }
        config.validate()?;
        Ok(config)
    }

    /// 从 TOML 字符串解析配置，并立即校验。
    ///
    /// 未出现的字段使用各自默认值；未知字段被忽略（向前兼容）。
    ///
    /// # Errors
    ///
    /// TOML 语法/类型错误返回 [`ConfigxError::Parse`]；解析成功但未通过
    /// [`validate`](Self::validate) 时返回 [`ConfigxError::Invalid`]。
    pub fn from_toml(text: &str) -> ConfigxResult<Self> {
        let config: Self = toml::from_str(text)
            .map_err(|error| ConfigxError::parse(format!("TOML 解析失败：{}", error.message())))?;
        config.validate()?;
        Ok(config)
    }

    /// 校验配置合法性。
    ///
    /// # Errors
    ///
    /// `watch_channel_capacity` 为 0 或超过 [`MAX_WATCH_CHANNEL_CAPACITY`] 时返回
    /// [`ConfigxError::Invalid`]。
    pub fn validate(&self) -> ConfigxResult<()> {
        if self.watch_channel_capacity == 0 {
            return Err(ConfigxError::invalid("watch 通道容量必须大于 0"));
        }
        if self.watch_channel_capacity > MAX_WATCH_CHANNEL_CAPACITY {
            return Err(ConfigxError::invalid(format!(
                "watch 通道容量不能超过 {MAX_WATCH_CHANNEL_CAPACITY}"
            )));
        }
        Ok(())
    }

    /// 创建链式构建器（以默认值为起点）。
    #[must_use]
    pub fn builder() -> ConfigxConfigBuilder {
        ConfigxConfigBuilder::default()
    }
}

/// [`ConfigxConfig`] 的链式构建器。
///
/// 每个 setter 都消费并返回 `self`，最后的 [`build`](Self::build) 负责校验。
#[derive(Debug, Clone, Default)]
pub struct ConfigxConfigBuilder {
    config: ConfigxConfig,
}

impl ConfigxConfigBuilder {
    /// 以默认值为起点创建构建器。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置是否在展示路径上脱敏。
    #[must_use]
    pub fn redact_secrets(mut self, redact_secrets: bool) -> Self {
        self.config.redact_secrets = redact_secrets;
        self
    }

    /// 设置 watch 通道容量。
    #[must_use]
    pub fn watch_channel_capacity(mut self, capacity: usize) -> Self {
        self.config.watch_channel_capacity = capacity;
        self
    }

    /// 设置是否允许空快照。
    #[must_use]
    pub fn allow_empty_snapshot(mut self, allow_empty: bool) -> Self {
        self.config.allow_empty_snapshot = allow_empty;
        self
    }

    /// 完成构建并校验。
    ///
    /// # Errors
    ///
    /// 配置未通过 [`ConfigxConfig::validate`] 时返回 [`ConfigxError::Invalid`]。
    pub fn build(self) -> ConfigxResult<ConfigxConfig> {
        self.config.validate()?;
        Ok(self.config)
    }
}

/// 读取环境变量；未设置或仅含空白时返回 `None`。
fn read_env(name: &str) -> ConfigxResult<Option<String>> {
    match env::var(name) {
        Ok(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed.to_string()))
            }
        }
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => Err(ConfigxError::invalid(format!(
            "环境变量不是有效 Unicode：{name}"
        ))),
    }
}

/// 解析布尔值；不接受的值只报告变量名，不回显原始值。
fn parse_bool(name: &str, value: &str) -> ConfigxResult<bool> {
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(ConfigxError::invalid(format!(
            "环境变量 {name} 不是合法布尔值：期望 true/false、yes/no、on/off 或 1/0"
        ))),
    }
}

/// 解析非负整数；不接受的值只报告变量名，不回显原始值。
fn parse_usize(name: &str, value: &str) -> ConfigxResult<usize> {
    value
        .parse::<usize>()
        .map_err(|_| ConfigxError::invalid(format!("环境变量 {name} 不是合法非负整数")))
}
