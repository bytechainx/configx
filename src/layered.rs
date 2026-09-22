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

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable
    )]

    use super::*;
    use crate::error::ConfigxError;
    use crate::{ErrorKind, MemorySource};
    use std::collections::HashMap;

    fn memory(pairs: &[(&str, &str)]) -> Arc<dyn ConfigSource> {
        Arc::new(MemorySource::from_pairs(pairs.iter().copied()))
    }

    /// 返回固定映射的源，用于构造「键非法」这类真实源难以表达的场景。
    struct RawSource(HashMap<String, String>);

    impl ConfigSource for RawSource {
        fn load(&self) -> ConfigxResult<HashMap<String, String>> {
            Ok(self.0.clone())
        }
    }

    /// 恒定失败的源，用于验证「任一层失败即整体失败」。
    struct FailingSource;

    impl ConfigSource for FailingSource {
        fn load(&self) -> ConfigxResult<HashMap<String, String>> {
            Err(ConfigxError::unavailable("测试源暂不可用"))
        }
    }

    #[test]
    fn empty_config_has_no_layers_and_merges_to_empty() {
        let layered = LayeredConfig::new();
        assert!(layered.is_empty());
        assert_eq!(layered.len(), 0);
        assert!(layered.load_merged().expect("空层合并必须成功").is_empty());
        assert_eq!(format!("{layered:?}"), "LayeredConfig { layers: 0 }");
    }

    #[test]
    fn later_sources_override_earlier_ones() {
        let layered = LayeredConfig::new()
            .with_source(memory(&[("a", "1"), ("b", "1")]))
            .with_source(memory(&[("b", "2"), ("c", "2")]));
        assert_eq!(layered.len(), 2);
        assert!(!layered.is_empty());

        let merged = layered.load_merged().expect("合并必须成功");
        assert_eq!(
            merged.get("a").map(String::as_str),
            Some("1"),
            "先注册独有的键保留"
        );
        assert_eq!(
            merged.get("b").map(String::as_str),
            Some("2"),
            "同键后注册者覆盖"
        );
        assert_eq!(merged.get("c").map(String::as_str), Some("2"));
    }

    #[test]
    fn push_and_with_source_build_the_same_layers() {
        let mut pushed = LayeredConfig::new();
        pushed.push(memory(&[("k", "v")]));
        let chained = LayeredConfig::new().with_source(memory(&[("k", "v")]));

        assert_eq!(pushed.len(), chained.len());
        assert_eq!(
            pushed.load_merged().expect("合并且成功"),
            chained.load_merged().expect("合并且成功")
        );
    }

    #[test]
    fn invalid_key_from_any_layer_fails_the_whole_merge() {
        let layered = LayeredConfig::new()
            .with_source(memory(&[("good", "1")]))
            .with_source(Arc::new(RawSource(HashMap::from([(
                "bad\nkey".to_string(),
                "2".to_string(),
            )]))));
        let error = layered.load_merged().expect_err("非法键必须整体失败");
        assert_eq!(error.kind(), ErrorKind::Invalid);
    }

    #[test]
    fn a_failing_source_aborts_without_partial_result() {
        let layered = LayeredConfig::new()
            .with_source(memory(&[("first", "1")]))
            .with_source(Arc::new(FailingSource));
        let error = layered.load_merged().expect_err("源失败必须整体失败");
        assert_eq!(error.kind(), ErrorKind::Unavailable);
        assert!(error.is_retryable());
    }

    #[test]
    fn debug_reports_only_the_layer_count() {
        let layered = LayeredConfig::new()
            .with_source(memory(&[("secret:token", "top-secret")]))
            .with_source(memory(&[("b", "2")]));
        let rendered = format!("{layered:?}");
        assert_eq!(rendered, "LayeredConfig { layers: 2 }");
        assert!(
            !rendered.contains("top-secret"),
            "不得展开任何值：{rendered}"
        );
    }
}
