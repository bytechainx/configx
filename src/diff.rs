//! 配置差异视图（纯函数，输入为不可变快照）。

use std::collections::{BTreeMap, BTreeSet};

/// 两个快照之间的键级差异。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConfigDiff {
    /// 仅左侧存在的键。
    pub only_left: Vec<String>,
    /// 仅右侧存在的键。
    pub only_right: Vec<String>,
    /// 两侧都存在但值不同的键。
    pub changed: Vec<String>,
}

impl ConfigDiff {
    /// 是否无差异。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.only_left.is_empty() && self.only_right.is_empty() && self.changed.is_empty()
    }

    /// 变更键的总数。
    #[must_use]
    pub fn total_changes(&self) -> usize {
        self.only_left.len() + self.only_right.len() + self.changed.len()
    }
}

/// 计算 `left` 相对 `right` 的差异。
///
/// 三个列表都按键升序排列（输入是 [`BTreeMap`]，集合运算保持有序），便于日志比对。
#[must_use]
pub fn diff_snapshots(
    left: &BTreeMap<String, String>,
    right: &BTreeMap<String, String>,
) -> ConfigDiff {
    let left_keys: BTreeSet<&String> = left.keys().collect();
    let right_keys: BTreeSet<&String> = right.keys().collect();
    let mut diff = ConfigDiff::default();
    for key in left_keys.difference(&right_keys) {
        diff.only_left.push((*key).clone());
    }
    for key in right_keys.difference(&left_keys) {
        diff.only_right.push((*key).clone());
    }
    for key in left_keys.intersection(&right_keys) {
        if left.get(*key) != right.get(*key) {
            diff.changed.push((*key).clone());
        }
    }
    diff
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

    fn snapshot(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }

    #[test]
    fn identical_snapshots_produce_empty_diff() {
        let left = snapshot(&[("b", "2"), ("a", "1")]);
        let diff = diff_snapshots(&left, &left.clone());
        assert!(diff.is_empty());
        assert_eq!(diff.total_changes(), 0);
        assert_eq!(diff, ConfigDiff::default());
    }

    #[test]
    fn both_empty_snapshots_agree() {
        let empty = BTreeMap::new();
        let diff = diff_snapshots(&empty, &empty);
        assert!(diff.is_empty());
        assert_eq!(diff.total_changes(), 0);
    }

    #[test]
    fn classifies_left_only_right_only_and_changed() {
        let left = snapshot(&[("keep", "1"), ("changed", "old"), ("only.left", "x")]);
        let right = snapshot(&[("keep", "1"), ("changed", "new"), ("only.right", "y")]);
        let diff = diff_snapshots(&left, &right);
        assert_eq!(diff.only_left, vec!["only.left".to_string()]);
        assert_eq!(diff.only_right, vec!["only.right".to_string()]);
        assert_eq!(diff.changed, vec!["changed".to_string()]);
        assert_eq!(diff.total_changes(), 3);
        assert!(!diff.is_empty());
    }

    #[test]
    fn all_change_kinds_are_sorted_ascending() {
        // 输入是 BTreeMap，集合运算保持有序：日志比对因此稳定。
        let left = snapshot(&[("z", "1"), ("m", "1"), ("a", "1")]);
        let right = snapshot(&[("z", "2"), ("m", "2"), ("a", "2")]);
        let diff = diff_snapshots(&left, &right);
        assert_eq!(
            diff.changed,
            vec!["a".to_string(), "m".to_string(), "z".to_string()]
        );

        let left_only = snapshot(&[("z", "1"), ("a", "1")]);
        let diff = diff_snapshots(&left_only, &BTreeMap::new());
        assert_eq!(
            diff.only_left,
            vec!["a".to_string(), "z".to_string()],
            "仅左侧的键同样按键升序"
        );
    }

    #[test]
    fn value_equality_ignores_nothing_by_type() {
        // 值比较是字符串精确比较：`01` 与 `1` 视为不同。
        let diff = diff_snapshots(&snapshot(&[("k", "01")]), &snapshot(&[("k", "1")]));
        assert_eq!(diff.changed, vec!["k".to_string()]);
    }
}
