//! 存储配置与构建器。

use std::env;

use crate::error::{ConfigxError, ConfigxResult};

/// 是否启用密钥脱敏（布尔）的环境变量名。
pub const ENV_REDACT_SECRETS: &str = "FOUNDATIONX_CONFIGX_REDACT_SECRETS";
/// 是否允许空快照（布尔）的环境变量名。
pub const ENV_ALLOW_EMPTY_SNAPSHOT: &str = "FOUNDATIONX_CONFIGX_ALLOW_EMPTY_SNAPSHOT";
/// 全局配置文件路径覆盖（非空则不再用 XDG/HOME + `app_id`）。
pub const ENV_GLOBAL_FILE: &str = "FOUNDATIONX_CONFIGX_GLOBAL_FILE";

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
/// # Examples
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
    /// # 错误消息
    ///
    /// 解析失败的消息**只报告位置**（第 N 行第 M 列）与固定类别文本，
    /// **不含**配置值字节，也不渲染源码片段——见 [`ConfigxError`] 顶部的脱敏约定。
    ///
    /// # Errors
    ///
    /// TOML 语法/类型错误返回 [`ConfigxError::Parse`]；解析成功但未通过
    /// [`validate`](Self::validate) 时返回 [`ConfigxError::Invalid`]。
    pub fn from_toml(text: &str) -> ConfigxResult<Self> {
        let config: Self = toml::from_str(text)
            .map_err(|error| ConfigxError::parse(describe_toml_failure(text, &error)))?;
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

/// 把 `toml` 的解析失败压成**不含配置值**的摘要。
///
/// 不能直接透传 `toml::de::Error` 的文本：`message()` 会把非法值内联进消息
/// （实测 `invalid type: string "…", expected a boolean`），`Display` 还会额外渲染
/// 源码行。两者都违反 [`ConfigxError`] 的「所有变体的消息都不得回显配置值」。
/// 这里只保留**位置**（由 `span()` 换算的行/列），其余一概不输出。
fn describe_toml_failure(text: &str, error: &toml::de::Error) -> String {
    match error.span() {
        Some(span) => {
            let (line, column) = line_and_column(text, span.start);
            format!("TOML 解析失败：第 {line} 行第 {column} 列")
        }
        None => "TOML 解析失败：文档格式不合法".to_string(),
    }
}

/// 把字节偏移换算成 1-based 的（行号，列号）；列按字符计数。
///
/// 偏移可能落在多字节字符内部（或被越界传入），一律向前退到字符边界，
/// 保证切片安全。
fn line_and_column(text: &str, byte_offset: usize) -> (usize, usize) {
    let mut offset = byte_offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    let before = &text[..offset];
    let line = before.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |index| index + 1);
    let column = text[line_start..offset].chars().count() + 1;
    (line, column)
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

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable
    )]

    use super::*;
    use crate::ErrorKind;

    #[test]
    fn defaults_enable_redaction_and_empty_snapshots() {
        let config = ConfigxConfig::default();
        assert!(config.redact_secrets);
        assert!(config.allow_empty_snapshot);
        config.validate().expect("默认配置必须通过校验");
    }

    #[test]
    fn builder_starts_from_defaults_and_overrides_fields() {
        let built = ConfigxConfigBuilder::new().build().expect("构建成功");
        assert_eq!(built, ConfigxConfig::default());
        assert_eq!(
            ConfigxConfig::builder().build().expect("构建成功"),
            ConfigxConfig::default(),
            "builder() 与 new() 起点一致"
        );

        let overridden = ConfigxConfig::builder()
            .redact_secrets(false)
            .allow_empty_snapshot(false)
            .build()
            .expect("构建成功");
        assert!(!overridden.redact_secrets);
        assert!(!overridden.allow_empty_snapshot);
    }

    #[test]
    fn validate_accepts_every_boolean_combination() {
        // 两个字段均为布尔量，不存在可被拒绝的取值。
        for redact in [true, false] {
            for allow in [true, false] {
                let config = ConfigxConfig::builder()
                    .redact_secrets(redact)
                    .allow_empty_snapshot(allow)
                    .build()
                    .expect("布尔字段无取值约束");
                assert_eq!(config.redact_secrets, redact);
                assert_eq!(config.allow_empty_snapshot, allow);
                config.validate().expect("恒为通过");
            }
        }
    }

    #[test]
    fn from_toml_uses_per_field_defaults() {
        let partial = ConfigxConfig::from_toml("redact_secrets = false\n").expect("解析成功");
        assert!(!partial.redact_secrets);
        assert!(partial.allow_empty_snapshot, "未出现的字段回落到默认值");

        assert_eq!(
            ConfigxConfig::from_toml("").expect("空文本解析成功"),
            ConfigxConfig::default()
        );

        let full =
            ConfigxConfig::from_toml("redact_secrets = false\nallow_empty_snapshot = false\n")
                .expect("解析成功");
        assert_eq!(
            full,
            ConfigxConfig::builder()
                .redact_secrets(false)
                .allow_empty_snapshot(false)
                .build()
                .expect("构建成功")
        );
    }

    #[test]
    fn from_toml_classifies_syntax_and_type_errors_as_parse() {
        for text in ["redact_secrets = ", "redact_secrets = \"yes\"", "= 1"] {
            let error = ConfigxConfig::from_toml(text).expect_err("必须报解析失败");
            assert_eq!(error.kind(), ErrorKind::Parse, "文本：{text:?}");
            assert!(error.to_string().starts_with("解析失败: "), "{error}");
        }
    }

    #[test]
    fn from_toml_parse_error_reports_position_without_value_or_source() {
        // 契约（`src/error.rs` 顶部文档）：解析失败的消息**不得回显配置值**，
        // 也不得渲染源码片段；只保留位置。
        let probe = "MARKER_LEAK_PROBE";
        let error = ConfigxConfig::from_toml(&format!("redact_secrets = \"{probe}\"\n"))
            .expect_err("类型错误必须报错");
        assert_eq!(error.kind(), ErrorKind::Parse);
        let rendered = error.to_string();
        assert!(!rendered.contains(probe), "不得回显配置值：{rendered}");
        assert!(
            !rendered.contains("redact_secrets ="),
            "不得回显源码行：{rendered}"
        );
        assert!(!rendered.contains(" |"), "不得渲染行号块：{rendered}");
        assert!(rendered.contains("第 1 行"), "应保留位置信息：{rendered}");
    }

    #[test]
    fn syntax_error_also_omits_value_and_reports_position() {
        // 语法错误（缺值）走同一条映射：同样不带值、不渲染片段。
        let error = ConfigxConfig::from_toml("redact_secrets = \n").expect_err("语法错误必须报错");
        assert_eq!(error.kind(), ErrorKind::Parse);
        let rendered = error.to_string();
        assert!(
            rendered.starts_with("解析失败: TOML 解析失败："),
            "{rendered}"
        );
        assert!(rendered.contains("第 1 行"), "应保留位置信息：{rendered}");
    }

    #[test]
    fn line_and_column_maps_offsets_to_1_based_positions() {
        let text = "a\nbb\nccc\n";
        assert_eq!(line_and_column(text, 0), (1, 1));
        assert_eq!(line_and_column(text, 2), (2, 1));
        assert_eq!(line_and_column(text, 5), (3, 1));
        assert_eq!(line_and_column(text, 8), (3, 4), "第三行末尾（换行符前）");
        assert_eq!(line_and_column(text, 9), (4, 1), "文本末尾之后");

        // 列按字符计数，而不是字节。
        assert_eq!(line_and_column("中=1", 3), (1, 2));
        // 偏移落在多字节字符内部或越界时，安全退到字符边界。
        assert_eq!(line_and_column("中", 1), (1, 1));
        assert_eq!(line_and_column("中", 99), (1, 2));
    }

    #[test]
    fn parse_bool_accepts_documented_aliases_case_insensitively() {
        for value in ["1", "true", "TRUE", "True", "yes", "YES", "on", "ON"] {
            assert!(
                parse_bool(ENV_REDACT_SECRETS, value).expect("真值别名"),
                "{value}"
            );
        }
        for value in ["0", "false", "FALSE", "False", "no", "NO", "off", "OFF"] {
            assert!(
                !parse_bool(ENV_REDACT_SECRETS, value).expect("假值别名"),
                "{value}"
            );
        }
    }

    #[test]
    fn parse_bool_rejects_unknown_values_without_echoing_them() {
        // 注意不要用 `tru` 这类与错误消息内提示词（true/false/yes/no/on/off）重叠的值，
        // 否则「不得回显」的断言会把提示词本身当成回显。
        for value in ["maybe", "2", "", "enabled", " yes"] {
            let error = parse_bool(ENV_REDACT_SECRETS, value).expect_err("非法布尔值必须报错");
            assert_eq!(error.kind(), ErrorKind::Invalid, "值：{value:?}");
            let rendered = error.to_string();
            assert!(
                rendered.contains(ENV_REDACT_SECRETS),
                "应报告变量名：{rendered}"
            );
            if !value.is_empty() {
                assert!(!rendered.contains(value), "不得回显原始值：{rendered}");
            }
        }
    }

    #[test]
    fn read_env_treats_absent_and_blank_as_unset() {
        // 测试专属变量名，避免与其它用例或真实环境竞争。
        const PROBE: &str = "CONFIGX_TEST_READ_ENV_PROBE";
        std::env::remove_var(PROBE);
        assert!(read_env(PROBE).expect("未设置不是错误").is_none());

        std::env::set_var(PROBE, "   \t ");
        assert!(read_env(PROBE).expect("仅空白视为未设置").is_none());

        std::env::set_var(PROBE, "  value  ");
        assert_eq!(read_env(PROBE).expect("读取成功").as_deref(), Some("value"));

        std::env::remove_var(PROBE);
        assert!(read_env(PROBE).expect("清理后未设置").is_none());
    }
}
