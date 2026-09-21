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
