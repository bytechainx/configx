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
