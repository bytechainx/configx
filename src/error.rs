//! 错误类型与错误分类。
//!
//! 本模块只描述「失败原因」，不描述「失败发生在哪个源」——后者由错误消息承载。

/// 配置错误的粗分类。
///
/// 供调用方按类别做决策（重试、回退、告警），无需匹配具体的错误变体。
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// 参数、配置或配置键非法。
    Invalid,
    /// 缺失配置键，或没有任何已成功加载的配置源。
    Missing,
    /// 键存在但值无法转换成目标类型。
    TypeMismatch,
    /// 与当前状态冲突（空快照策略、监听已关闭、锁中毒等）。
    Conflict,
    /// 配置文本（TOML / `KEY=VALUE`）解析失败。
    Parse,
    /// 当前能力不支持该操作。
    Unsupported,
    /// 配置源暂不可用；这是唯一可安全重试的分类。
    Unavailable,
}

/// `configx` 错误类型。
///
/// # 错误消息与敏感数据
///
/// 所有变体的消息都不得回显配置值：类型转换失败只报告键名与目标类型名，
/// TOML 解析失败只报告 `toml` 的错误摘要（不含源码片段），
/// `KEY=VALUE` 解析失败只报告行号。这样才能安全地把错误写入日志。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConfigxError {
    /// 参数、配置或配置键非法。
    #[error("配置无效: {0}")]
    Invalid(String),
    /// 缺失配置键或配置源。
    #[error("缺失配置: {0}")]
    Missing(String),
    /// 键存在但值无法转换成目标类型。
    #[error("类型不匹配: {0}")]
    TypeMismatch(String),
    /// 当前状态与请求冲突。
    #[error("状态冲突: {0}")]
    Conflict(String),
    /// 配置文本解析失败。
    #[error("解析失败: {0}")]
    Parse(String),
    /// 当前能力不支持该操作。
    #[error("不支持的操作: {0}")]
    Unsupported(String),
    /// 配置源暂不可用；同一操作稍后重试可能成功。
    #[error("配置源暂不可用: {0}")]
    Unavailable(String),
    /// 读取配置源时的 I/O 失败；底层错误通过 [`std::error::Error::source`] 保留。
    #[error("I/O 失败: {message}")]
    Io {
        /// 人类可读的上下文（例如配置文件路径）。
        message: String,
        /// 底层 I/O 错误。
        #[source]
        source: std::io::Error,
    },
}

impl ConfigxError {
    /// 构造「非法输入」错误。
    #[must_use]
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }

    /// 构造「缺失」错误。
    #[must_use]
    pub fn missing(message: impl Into<String>) -> Self {
        Self::Missing(message.into())
    }

    /// 构造「类型不匹配」错误。
    #[must_use]
    pub fn type_mismatch(message: impl Into<String>) -> Self {
        Self::TypeMismatch(message.into())
    }

    /// 构造「状态冲突」错误。
    #[must_use]
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::Conflict(message.into())
    }

    /// 构造「解析失败」错误。
    #[must_use]
    pub fn parse(message: impl Into<String>) -> Self {
        Self::Parse(message.into())
    }

    /// 构造「不支持的操作」错误。
    #[must_use]
    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::Unsupported(message.into())
    }

    /// 构造「配置源暂不可用」错误。
    #[must_use]
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::Unavailable(message.into())
    }

    /// 构造「读取配置源 I/O 失败」错误，并保留底层错误。
    #[must_use]
    pub fn io(message: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            message: message.into(),
            source,
        }
    }

    /// 错误分类。
    ///
    /// 配置读取失败（[`ConfigxError::Io`]）一律归入 [`ErrorKind::Invalid`]：
    /// 文件缺失或权限不足都需要人工修复，不属于瞬时故障。
    #[must_use]
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::Invalid(_) | Self::Io { .. } => ErrorKind::Invalid,
            Self::Missing(_) => ErrorKind::Missing,
            Self::TypeMismatch(_) => ErrorKind::TypeMismatch,
            Self::Conflict(_) => ErrorKind::Conflict,
            Self::Parse(_) => ErrorKind::Parse,
            Self::Unsupported(_) => ErrorKind::Unsupported,
            Self::Unavailable(_) => ErrorKind::Unavailable,
        }
    }

    /// 是否属于可安全重试的瞬时错误。
    ///
    /// 配置错误绝大多数是确定性的（键名写错、格式不合法、文件不存在），重试没有意义；
    /// 只有 [`ErrorKind::Unavailable`] 表示「源暂不可用」，稍后重试可能恢复。
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(self.kind(), ErrorKind::Unavailable)
    }
}

/// crate 专用 `Result` 别名。
pub type ConfigxResult<T> = Result<T, ConfigxError>;

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable
    )]

    use super::*;

    #[test]
    fn constructors_map_to_expected_kinds() {
        assert_eq!(ConfigxError::invalid("x").kind(), ErrorKind::Invalid);
        assert_eq!(ConfigxError::missing("x").kind(), ErrorKind::Missing);
        assert_eq!(
            ConfigxError::type_mismatch("x").kind(),
            ErrorKind::TypeMismatch
        );
        assert_eq!(ConfigxError::conflict("x").kind(), ErrorKind::Conflict);
        assert_eq!(ConfigxError::parse("x").kind(), ErrorKind::Parse);
        assert_eq!(
            ConfigxError::unsupported("x").kind(),
            ErrorKind::Unsupported
        );
        assert_eq!(
            ConfigxError::unavailable("x").kind(),
            ErrorKind::Unavailable
        );
    }

    #[test]
    fn io_error_is_invalid_kind_and_keeps_source() {
        let io = ConfigxError::io(
            "读取配置文件失败：路径=/tmp/x",
            std::io::Error::new(std::io::ErrorKind::NotFound, "gone"),
        );
        assert_eq!(
            io.kind(),
            ErrorKind::Invalid,
            "文件缺失需人工修复，不可重试"
        );
        let source = std::error::Error::source(&io).expect("必须保留底层 I/O 错误");
        assert_eq!(source.to_string(), "gone");
    }

    #[test]
    fn only_unavailable_is_retryable() {
        let retryable = [
            ConfigxError::invalid("x"),
            ConfigxError::missing("x"),
            ConfigxError::type_mismatch("x"),
            ConfigxError::conflict("x"),
            ConfigxError::parse("x"),
            ConfigxError::unsupported("x"),
            ConfigxError::io("x", std::io::Error::other("boom")),
        ];
        for error in retryable {
            assert!(!error.is_retryable(), "{error} 不应被判为可重试");
        }
        assert!(ConfigxError::unavailable("x").is_retryable());
    }

    #[test]
    fn display_uses_stable_chinese_prefixes() {
        assert_eq!(ConfigxError::invalid("原因").to_string(), "配置无效: 原因");
        assert_eq!(ConfigxError::missing("原因").to_string(), "缺失配置: 原因");
        assert_eq!(
            ConfigxError::type_mismatch("原因").to_string(),
            "类型不匹配: 原因"
        );
        assert_eq!(ConfigxError::conflict("原因").to_string(), "状态冲突: 原因");
        assert_eq!(ConfigxError::parse("原因").to_string(), "解析失败: 原因");
        assert_eq!(
            ConfigxError::unsupported("原因").to_string(),
            "不支持的操作: 原因"
        );
        assert_eq!(
            ConfigxError::unavailable("原因").to_string(),
            "配置源暂不可用: 原因"
        );
        assert_eq!(
            ConfigxError::io("路径=/tmp/x", std::io::Error::other("boom")).to_string(),
            "I/O 失败: 路径=/tmp/x"
        );
    }

    #[test]
    fn kind_is_deterministic_for_all_variants() {
        // 逐变体确认 kind() 覆盖完整：不依赖 ErrorKind 的派生顺序。
        let pairs = [
            (ConfigxError::Invalid(String::new()), ErrorKind::Invalid),
            (ConfigxError::Missing(String::new()), ErrorKind::Missing),
            (
                ConfigxError::TypeMismatch(String::new()),
                ErrorKind::TypeMismatch,
            ),
            (ConfigxError::Conflict(String::new()), ErrorKind::Conflict),
            (ConfigxError::Parse(String::new()), ErrorKind::Parse),
            (
                ConfigxError::Unsupported(String::new()),
                ErrorKind::Unsupported,
            ),
            (
                ConfigxError::Unavailable(String::new()),
                ErrorKind::Unavailable,
            ),
        ];
        for (error, expected) in pairs {
            assert_eq!(error.kind(), expected);
        }
    }
}
