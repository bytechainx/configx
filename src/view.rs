//! 只读视图辅助：子集快照与快照一致性比较。

use std::collections::BTreeMap;

use crate::error::ConfigxResult;
use crate::key::validate_key;
use crate::store::ConfigxStore;

/// 从存储挑选子集键构建新快照。
///
/// 不存在的键被跳过（不视为错误）；任一请求键非法时折叠为空快照——
/// 需要区分失败原因时用 [`try_subset_snapshot`]。
#[must_use]
pub fn subset_snapshot(store: &ConfigxStore, keys: &[&str]) -> BTreeMap<String, String> {
    try_subset_snapshot(store, keys).unwrap_or_default()
}

/// 从存储挑选子集键构建新快照，并显式报告键非法。
///
/// # Errors
///
/// 任一请求键为空、含控制字符或超过 512 字节时返回 `ConfigxError::Invalid`；
/// 缺失的键仍然只是被跳过。
pub fn try_subset_snapshot(
    store: &ConfigxStore,
    keys: &[&str],
) -> ConfigxResult<BTreeMap<String, String>> {
    let full = store.snapshot();
    let mut subset = BTreeMap::new();
    for key in keys {
        validate_key(key)?;
        if let Some(value) = full.get(*key) {
            subset.insert((*key).to_string(), value.clone());
        }
    }
    Ok(subset)
}

/// 两个快照在给定键上是否完全一致（含「两侧都缺失」）。
#[must_use]
pub fn snapshots_agree(
    left: &BTreeMap<String, String>,
    right: &BTreeMap<String, String>,
    keys: &[&str],
) -> bool {
    keys.iter().all(|key| left.get(*key) == right.get(*key))
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
    use crate::{ConfigxError, ErrorKind, MemorySource};

    fn store_with(pairs: &[(&str, &str)]) -> ConfigxStore {
        let mut store = ConfigxStore::new();
        store.register_source(MemorySource::from_pairs(pairs.iter().copied()));
        store.reload().expect("reload 必须成功");
        store
    }

    #[test]
    fn subset_keeps_requested_keys_and_skips_missing_ones() {
        let store = store_with(&[("a", "1"), ("b", "2"), ("c", "3")]);
        let subset = try_subset_snapshot(&store, &["c", "absent", "a"]).expect("键均合法");
        assert_eq!(subset.len(), 2);
        assert_eq!(subset.get("a").map(String::as_str), Some("1"));
        assert_eq!(subset.get("c").map(String::as_str), Some("3"));
        assert!(!subset.contains_key("absent"), "缺失键只是被跳过");
    }

    #[test]
    fn subset_of_empty_key_list_is_empty_map() {
        let store = store_with(&[("a", "1")]);
        assert!(try_subset_snapshot(&store, &[])
            .expect("空请求合法")
            .is_empty());
        assert!(subset_snapshot(&store, &[]).is_empty());
    }

    #[test]
    fn try_subset_reports_invalid_key_but_subset_folds_to_empty() {
        let store = store_with(&[("a", "1")]);

        let empty_key = try_subset_snapshot(&store, &[""]).expect_err("空键必须报错");
        assert_eq!(empty_key.kind(), ErrorKind::Invalid);

        let control_key = try_subset_snapshot(&store, &["a", "b\n"]).expect_err("控制字符必须报错");
        assert_eq!(control_key.kind(), ErrorKind::Invalid);
        assert!(matches!(control_key, ConfigxError::Invalid(_)));

        // 折叠版本对同一请求返回空快照，而不是部分结果。
        assert!(subset_snapshot(&store, &["a", "b\n"]).is_empty());
    }

    #[test]
    fn agreement_treats_both_missing_as_equal() {
        let left: BTreeMap<String, String> =
            [("a".to_string(), "1".to_string())].into_iter().collect();
        let right: BTreeMap<String, String> =
            [("a".to_string(), "1".to_string())].into_iter().collect();

        // 两侧都不存在的键视为一致。
        assert!(snapshots_agree(&left, &right, &["a", "missing"]));
        // 只比较给定键：未列入的差异不影响结论。
        assert!(snapshots_agree(&left, &right, &["a"]));
    }

    #[test]
    fn agreement_detects_value_and_presence_differences() {
        let left: BTreeMap<String, String> =
            [("a".to_string(), "1".to_string())].into_iter().collect();
        let right: BTreeMap<String, String> =
            [("a".to_string(), "2".to_string())].into_iter().collect();
        assert!(!snapshots_agree(&left, &right, &["a"]));

        let empty = BTreeMap::new();
        assert!(
            !snapshots_agree(&left, &empty, &["a"]),
            "一侧缺失另一侧存在时必须判为不一致"
        );
        assert!(snapshots_agree(&left, &empty, &[]), "空键列表恒为真");
    }
}
