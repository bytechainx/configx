//! 存储配置与构建器。

use std::env;

use crate::error::{ConfigxError, ConfigxResult};

/// 是否启用密钥脱敏（布尔）的环境变量名。
pub const ENV_REDACT_SECRETS: &str = "FOUNDATIONX_CONFIGX_REDACT_SECRETS";
/// 是否允许空快照（布尔）的环境变量名。
pub const ENV_ALLOW_EMPTY_SNAPSHOT: &str = "FOUNDATIONX_CONFIGX_ALLOW_EMPTY_SNAPSHOT";

fn default_redact_secrets() -> bool {
    true
}

fn default_allow_empty_snapshot() -> bool {
    true
}

/// `configx` 存储配置。
///
/// 两个字段都带默认值，因此 TOML / 环境变量都可以只覆盖其中一部分。
/// 字段本身不做语义推导，全部约束集中在 [`validate`](Self::validate)。
///
/// # 示例
///
/// ```
/// use configx::ConfigxConfig;
///
/// let config = ConfigxConfig::builder().allow_empty_snapshot(false).build()?;
/// assert!(!config.allow_empty_snapshot);
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
            allow_empty_snapshot: default_allow_empty_snapshot(),
        }
    }
}

impl ConfigxConfig {
    /// 从环境变量加载配置并覆盖默认值。
    ///
    /// 只识别前缀为 `FOUNDATIONX_CONFIGX_` 的变量：
    /// [`ENV_REDACT_SECRETS`]、[`ENV_ALLOW_EMPTY_SNAPSHOT`]。
    /// 变量未设置或只含空白时使用默认值；布尔值接受 `1/0`、`true/false`、`yes/no`、`on/off`（忽略大小写）。
    ///
    /// # Errors
    ///
    /// 变量值不是合法布尔值、不是有效 Unicode，或最终配置未通过
    /// [`validate`](Self::validate) 时返回错误。
    pub fn from_env() -> ConfigxResult<Self> {
        let mut config = Self::default();
        if let Some(value) = read_env(ENV_REDACT_SECRETS)? {
            config.redact_secrets = parse_bool(ENV_REDACT_SECRETS, &value)?;
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
    /// [`ConfigxConfig`] 当前的字段（`redact_secrets`、`allow_empty_snapshot`）都是布尔量，
    /// 取值范围只有 `true` / `false` 两种可能，没有任何数值区间或字段间约束，因此不存在
    /// 可以被拒绝的取值——本方法在该字段集合下返回 `Ok(())`。
    ///
    /// 之所以保留一个恒为通过的方法，是为了维持本库与同工作区其它适配器一致的统一 API：
    /// [`from_env`](Self::from_env)、[`from_toml`](Self::from_toml) 与
    /// [`build`](ConfigxConfigBuilder::build) 都无条件调用它，将来新增带约束的字段
    /// （数值区间、互斥组合等）时，只需在此处补上检查，无需改动调用方与函数签名。
    ///
    /// # Errors
    ///
    /// 当前字段集合下不会返回错误；`ConfigxResult` 返回值是为将来新增约束预留的。
    pub fn validate(&self) -> ConfigxResult<()> {
        // 两个字段均为布尔量，无取值约束，故无检查项可写。
        // 这里是将来新增字段时的校验入口。
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
