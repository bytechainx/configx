//! 多层配置合并：按注册顺序，**后注册源覆盖先注册源**。

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use crate::error::ConfigxResult;
use crate::key::validate_key;
use crate::source::ConfigSource;

/// 多层配置合并器。
///
/// # 优先级
///
/// `sources` 按插入顺序排列：**越靠后优先级越高**。同一键出现多次时，
/// 后注册的值覆盖先注册的值；先注册独有的键保留。
///
/// 合并结果使用 [`BTreeMap`]，因此键序稳定、可直接比较与打快照。
#[derive(Default)]
pub struct LayeredConfig {
    sources: Vec<Arc<dyn ConfigSource>>,
}

impl LayeredConfig {
    /// 空层。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 追加一层；该层成为当前最高优先级。
    pub fn push(&mut self, source: Arc<dyn ConfigSource>) {
        self.sources.push(source);
    }

    /// 链式追加一层。
    #[must_use]
    pub fn with_source(mut self, source: Arc<dyn ConfigSource>) -> Self {
        self.push(source);
        self
    }

    /// 层数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.sources.len()
    }

    /// 是否没有任何层。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    /// 按层序加载并合并全部源。
    ///
    /// 每个键在写入合并结果前都会做键校验；任一层加载失败或键非法即整体失败，
    /// 不返回部分合并结果。
    ///
    /// # Errors
    ///
    /// 任一源加载失败，或任一键未通过键校验时返回错误。
    pub fn load_merged(&self) -> ConfigxResult<BTreeMap<String, String>> {
        let mut merged = BTreeMap::new();
        for source in &self.sources {
            let map = source.load()?;
            for (key, value) in map {
                validate_key(&key)?;
                merged.insert(key, value);
            }
        }
        Ok(merged)
    }
}

impl fmt::Debug for LayeredConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 只报告层数，不展开任何值。
        f.debug_struct("LayeredConfig")
            .field("layers", &self.sources.len())
            .finish()
    }
}
