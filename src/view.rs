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
