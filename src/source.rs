//! 配置源抽象：内存 / 环境变量 / `KEY=VALUE` 文件。

use std::collections::HashMap;
use std::env;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{ConfigxError, ConfigxResult};
use crate::secret::RedactedHashMap;

/// 可加载配置条目的源。
///
/// 实现必须返回**完整快照**：调用方负责合并与覆盖策略，源本身不感知层序。
/// 实现需要 `Send + Sync`，因为源会被存进可共享的存储中。
pub trait ConfigSource: Send + Sync {
    /// 加载当前键值映射。
    ///
    /// # Errors
    ///
    /// 源读取或解析失败时返回错误。返回的键会由调用方逐个校验。
    fn load(&self) -> ConfigxResult<HashMap<String, String>>;
}

/// 内存配置源：值直接来自构造时给定的键值对。
#[derive(Clone, Default)]
pub struct MemorySource {
    entries: HashMap<String, String>,
}

impl fmt::Debug for MemorySource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 内存源不持有脱敏配置，因此 Debug 恒定脱敏：宁可少输出也不泄漏。
        f.debug_struct("MemorySource")
            .field("entries", &RedactedHashMap(&self.entries))
            .finish()
    }
}

impl MemorySource {
    /// 空源。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 由键值对构造。
    #[must_use]
    pub fn from_pairs<I, K, V>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let mut entries = HashMap::new();
        for (key, value) in pairs {
            entries.insert(key.into(), value.into());
        }
        Self { entries }
    }
}

impl ConfigSource for MemorySource {
    fn load(&self) -> ConfigxResult<HashMap<String, String>> {
        Ok(self.entries.clone())
    }
}

/// 环境变量配置源。
///
/// 只收集以 `prefix` 开头的变量，写入映射时**剥离前缀**：
/// `prefix = "APP_"` 时 `APP_HOST=h` → 键 `HOST`。
#[derive(Debug, Clone)]
pub struct EnvSource {
    prefix: String,
}

impl EnvSource {
    /// 构造源；`prefix` 为空时 [`load`](ConfigSource::load) 不返回任何键
    /// （避免误吞整张环境变量表）。
    #[must_use]
    pub fn new(prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
        }
    }

    /// 前缀。
    #[must_use]
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// 从任意键值迭代器加载（测试与依赖注入用；生产路径走 [`ConfigSource::load`]）。
    ///
    /// 前缀剥离规则与生产路径一致：前缀匹配但剩余部分为空（如 `APP_`）的变量被跳过。
    ///
    /// # Errors
    ///
    /// 当前实现不产生错误；返回 `Result` 以保持与 [`ConfigSource::load`] 形状一致。
    pub fn load_from_iter<I, K, V>(&self, vars: I) -> ConfigxResult<HashMap<String, String>>
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: Into<String>,
    {
        if self.prefix.is_empty() {
            return Ok(HashMap::new());
        }
        let mut out = HashMap::new();
        for (key, value) in vars {
            let key = key.as_ref();
            let Some(stripped) = key.strip_prefix(&self.prefix) else {
                continue;
            };
            if stripped.is_empty() {
                continue;
            }
            out.insert(stripped.to_string(), value.into());
        }
        Ok(out)
    }

    /// 从原始环境变量迭代器加载，把非 Unicode 键/值转成显式错误。
    fn load_from_os_iter<I>(&self, vars: I) -> ConfigxResult<HashMap<String, String>>
    where
        I: IntoIterator<Item = (OsString, OsString)>,
    {
        let vars = vars
            .into_iter()
            .map(|(key, value)| {
                let key = key
                    .into_string()
                    .map_err(|_| ConfigxError::invalid("环境变量键不是有效 Unicode"))?;
                let value = value
                    .into_string()
                    .map_err(|_| ConfigxError::invalid("环境变量值不是有效 Unicode"))?;
                Ok((key, value))
            })
            .collect::<ConfigxResult<Vec<_>>>()?;
        self.load_from_iter(vars)
    }
}

impl ConfigSource for EnvSource {
    fn load(&self) -> ConfigxResult<HashMap<String, String>> {
        if self.prefix.is_empty() {
            return Ok(HashMap::new());
        }
        self.load_from_os_iter(env::vars_os())
    }
}

/// 简单文件配置源：读取 `KEY=VALUE` 文本文件。
///
/// 解析规则见 [`parse_key_value_file`]。
#[derive(Debug, Clone)]
pub struct FileSource {
    path: PathBuf,
}

impl FileSource {
    /// 指定文件路径。
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// 路径。
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl ConfigSource for FileSource {
    fn load(&self) -> ConfigxResult<HashMap<String, String>> {
        let text = fs::read_to_string(&self.path).map_err(|error| {
            ConfigxError::io(
                format!("读取配置文件失败：路径={}", self.path.display()),
                error,
            )
        })?;
        parse_key_value_file(&text)
    }
}

/// 解析 `KEY=VALUE` 文本。
///
/// 规则（与既有实现保持一致）：
/// - 逐行处理，行首尾空白忽略；
/// - 空行与 `#` 开头的注释行跳过；
/// - 非注释行必须包含 `=`，否则报错；
/// - 键做两侧 trim，允许包含 `.`、`-`、大小写混合等任意非控制字符；
/// - 值做两侧 trim，若被单引号或双引号成对包裹则各去掉一层引号（只去一层）。
///
/// # Errors
///
/// 非注释行缺少 `=` 或键为空时返回 [`ConfigxError::Invalid`]。
/// 错误只携带行号，**不回显原始行内容**，避免把敏感值写进日志。
pub fn parse_key_value_file(text: &str) -> ConfigxResult<HashMap<String, String>> {
    let mut out = HashMap::new();
    for (lineno, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(ConfigxError::invalid(format!(
                "配置文件第 {} 行：应为 KEY=VALUE",
                lineno + 1
            )));
        };
        let key = key.trim();
        if key.is_empty() {
            return Err(ConfigxError::invalid(format!(
                "配置文件第 {} 行：键为空",
                lineno + 1
            )));
        }
        let mut value = value.trim().to_string();
        if (value.starts_with('"') && value.ends_with('"') && value.len() >= 2)
            || (value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2)
        {
            value = value[1..value.len() - 1].to_string();
        }
        out.insert(key.to_string(), value);
    }
    Ok(out)
}
