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
