//! 密钥脱敏：独立于读取路径的纯函数集合。
//!
//! 脱敏**只作用于日志与 `Debug` 展示路径**。键值存储的读取接口（如
//! [`ConfigxStore::get`](crate::ConfigxStore::get)）永远返回原始值，
//! 与脱敏开关无关——否则调用方拿到 `***` 会直接破坏业务语义。

use std::collections::{BTreeMap, HashMap};
use std::fmt;

/// 密钥键的统一前缀。
///
/// 键以该前缀开头即视为敏感值，需要在展示时脱敏。
pub const SECRET_KEY_PREFIX: &str = "secret:";

/// 脱敏后的占位文本。
pub const REDACTED_VALUE: &str = "***";

/// 判断键是否被标记为敏感。
///
/// 判定只依赖 [`SECRET_KEY_PREFIX`] 前缀，大小写敏感；不猜测
/// `password` / `token` 之类的词形，避免出现难以预期的误判。
#[must_use]
pub fn is_secret_key(key: &str) -> bool {
    key.starts_with(SECRET_KEY_PREFIX)
}

/// 按需脱敏单个值：敏感键返回 [`REDACTED_VALUE`]，其余原样返回。
///
/// 返回的引用要么指向 `value`，要么指向 `'static` 常量，不产生分配。
#[must_use]
pub fn redact_value<'a>(key: &str, value: &'a str) -> &'a str {
    if is_secret_key(key) {
        REDACTED_VALUE
    } else {
        value
    }
}

/// 生成脱敏后的键值副本，供日志或调试输出使用。
///
/// 原始映射不会被修改，调用方也不需要持有 `&mut`。
#[must_use]
pub fn redact_map(entries: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|(key, value)| (key.clone(), redact_value(key, value).to_string()))
        .collect()
}

/// 以键升序输出脱敏后的 `Debug` 映射。
///
/// 排序是为了让日志稳定可比，不依赖底层哈希顺序。
pub(crate) fn fmt_redacted<'a, I>(f: &mut fmt::Formatter<'_>, entries: I) -> fmt::Result
where
    I: IntoIterator<Item = (&'a String, &'a String)>,
{
    let mut items: Vec<(&String, &String)> = entries.into_iter().collect();
    items.sort_unstable_by(|left, right| left.0.cmp(right.0));
    let mut map = f.debug_map();
    for (key, value) in items {
        map.entry(key, &redact_value(key, value));
    }
    map.finish()
}

/// [`BTreeMap`] 脱敏视图的 `Debug` 包装。
pub(crate) struct RedactedEntries<'a>(pub(crate) &'a BTreeMap<String, String>);

impl fmt::Debug for RedactedEntries<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_redacted(f, self.0.iter())
    }
}

/// [`HashMap`] 脱敏视图的 `Debug` 包装。
pub(crate) struct RedactedHashMap<'a>(pub(crate) &'a HashMap<String, String>);

impl fmt::Debug for RedactedHashMap<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_redacted(f, self.0.iter())
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

    #[test]
    fn secret_prefix_is_the_only_signal_and_is_case_sensitive() {
        assert_eq!(SECRET_KEY_PREFIX, "secret:");
        assert!(is_secret_key("secret:"));
        assert!(is_secret_key("secret:token"));
        assert!(!is_secret_key("secret"));
        assert!(!is_secret_key("Secret:token"), "判定大小写敏感");
        assert!(!is_secret_key("password"), "不猜测词形，避免误判");
        assert!(!is_secret_key(""));
    }

    #[test]
    fn redact_value_only_replaces_secret_keys() {
        assert_eq!(redact_value("secret:token", "abc"), REDACTED_VALUE);
        assert_eq!(redact_value("plain", "abc"), "abc");

        // 不产生分配：非敏感值返回入参引用本身。
        let owned = String::from("value");
        let borrowed = redact_value("plain", &owned);
        assert!(
            std::ptr::eq(borrowed, owned.as_str()),
            "非敏感键必须原样返回入参引用"
        );

        // 敏感值返回 `REDACTED_VALUE` 常量：位置与入参不同，内容等于脱敏占位。
        let redacted = redact_value("secret:token", &owned);
        assert_eq!(redacted, REDACTED_VALUE);
        assert!(
            !std::ptr::eq(redacted.as_ptr(), owned.as_ptr()),
            "敏感键不得返回入参引用"
        );
    }

    #[test]
    fn redact_map_copies_and_keeps_original_intact() {
        let mut entries = BTreeMap::new();
        entries.insert("plain".to_string(), "visible".to_string());
        entries.insert("secret:token".to_string(), "hidden".to_string());

        let redacted = redact_map(&entries);
        assert_eq!(redacted.get("plain").map(String::as_str), Some("visible"));
        assert_eq!(
            redacted.get("secret:token").map(String::as_str),
            Some(REDACTED_VALUE)
        );
        assert_eq!(
            entries.get("secret:token").map(String::as_str),
            Some("hidden"),
            "原映射不得被修改"
        );
    }

    #[test]
    fn redacted_entries_debug_sorts_keys_and_masks_secrets() {
        let mut entries = BTreeMap::new();
        entries.insert("z".to_string(), "1".to_string());
        entries.insert("a".to_string(), "2".to_string());
        entries.insert("secret:token".to_string(), "hidden".to_string());

        let rendered = format!("{:?}", RedactedEntries(&entries));
        assert_eq!(
            rendered, r#"{"a": "2", "secret:token": "***", "z": "1"}"#,
            "Debug 必须按键升序且遮蔽敏感值"
        );
        assert!(
            !rendered.contains("hidden"),
            "不得泄漏原始敏感值：{rendered}"
        );
    }

    #[test]
    fn redacted_hashmap_debug_is_sorted_regardless_of_insert_order() {
        // HashMap 迭代顺序不确定，fmt_redacted 必须显式排序，否则日志不可比。
        let mut entries = HashMap::new();
        entries.insert("b".to_string(), "2".to_string());
        entries.insert("a".to_string(), "1".to_string());
        entries.insert("secret:k".to_string(), "v".to_string());

        let rendered = format!("{:?}", RedactedHashMap(&entries));
        assert_eq!(
            rendered, r#"{"a": "1", "b": "2", "secret:k": "***"}"#,
            "Debug 必须按键升序且遮蔽敏感值"
        );
    }
}
