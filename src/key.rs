//! 配置键校验：schema 边界上的最小门禁。
//!
//! 这里**不做**大小写折叠、分隔符归一或路径分段——键按原样写入、按原样查询，
//! 与键值存储的 `HashMap` 语义保持一致。只拒绝必然导致歧义的键。

/// 配置键的字节长度上限。
const MAX_KEY_BYTES: usize = 512;

/// 校验配置键是否可接受。
///
/// 规则：非空、不含控制字符、UTF-8 字节长度不超过 512。
///
/// # Errors
///
/// 键为空、包含控制字符或超过 512 字节时返回 `ConfigxError::Invalid`。
pub(crate) fn validate_key(key: &str) -> crate::ConfigxResult<()> {
    if key.is_empty() {
        return Err(crate::ConfigxError::invalid("配置键不能为空"));
    }
    if key.chars().any(char::is_control) {
        return Err(crate::ConfigxError::invalid("配置键不能包含控制字符"));
    }
    if key.len() > MAX_KEY_BYTES {
        return Err(crate::ConfigxError::invalid("配置键长度超过 512 字节"));
    }
    Ok(())
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
    use crate::{ConfigxError, ErrorKind};

    #[test]
    fn accepts_ordinary_keys_without_normalizing() {
        // 本模块只做最小门禁：不做大小写折叠、分隔符归一或路径分段。
        assert!(matches!(validate_key("app.host"), Ok(())));
        assert!(matches!(validate_key("A-b_c/9"), Ok(())));
        assert!(matches!(validate_key("a b"), Ok(())));
        assert!(matches!(validate_key("中文键"), Ok(())));
    }

    #[test]
    fn rejects_empty_key() {
        let error = validate_key("").expect_err("空键必须被拒绝");
        assert_eq!(error.kind(), ErrorKind::Invalid);
        assert!(matches!(error, ConfigxError::Invalid(_)));
        assert!(error.to_string().contains("不能为空"), "实际消息：{error}");
    }

    #[test]
    fn rejects_keys_with_control_characters() {
        for key in ["a\nb", "a\rb", "a\tb", "a\u{7}b"] {
            let error = validate_key(key).expect_err("控制字符必须被拒绝");
            assert_eq!(error.kind(), ErrorKind::Invalid);
            assert!(
                error.to_string().contains("控制字符"),
                "键 {key:?} 的实际消息：{error}"
            );
        }
    }

    #[test]
    fn accepts_key_at_byte_limit_and_rejects_beyond() {
        let at_limit = "a".repeat(MAX_KEY_BYTES);
        assert!(matches!(validate_key(&at_limit), Ok(())));

        let beyond = "a".repeat(MAX_KEY_BYTES + 1);
        let error = validate_key(&beyond).expect_err("超长键必须被拒绝");
        assert_eq!(error.kind(), ErrorKind::Invalid);
        assert!(error.to_string().contains("512"), "实际消息：{error}");
    }

    #[test]
    fn length_limit_counts_utf8_bytes_not_characters() {
        // 每个「字」为 3 字节：170 个 = 510 字节（通过），171 个 = 513 字节（拒绝）。
        assert!(matches!(validate_key(&"字".repeat(170)), Ok(())));
        let error = validate_key(&"字".repeat(171)).expect_err("按字节超限必须被拒绝");
        assert_eq!(error.kind(), ErrorKind::Invalid);
    }
}
