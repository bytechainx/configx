//! 配置源抽象：内存 / 环境变量 / `KEY=VALUE` 文件。

use std::collections::HashMap;
use std::env;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::ENV_GLOBAL_FILE;
use crate::error::{ConfigxError, ConfigxResult};
use crate::secret::{is_secret_key, RedactedHashMap};

/// 可加载配置条目的源。
///
/// 实现必须返回**完整快照**：调用方负责合并与覆盖策略，源本身不感知层序。
/// 实现需要 `Send + Sync`，因为源会被存进可共享的存储中。
///
/// # 阻塞调用
///
/// 本 trait 为纯同步接口，不依赖任何异步运行时。`load()` 可能执行阻塞 I/O
/// （如 [`FileSource`] 的 `std::fs::read_to_string`、或自定义实现的网络请求等）。
/// **异步调用方（tokio 上下文）须用 `tokio::task::spawn_blocking` 隔离，否则会
/// 冻结运行时工作线程。**
pub trait ConfigSource: Send + Sync {
    /// 加载当前键值映射。
    ///
    /// # 阻塞调用
    ///
    /// 此方法可能执行阻塞 I/O（文件读取、网络请求等），仅限**同步上下文**调用。
    /// 若必须在异步上下文中调用，请用 `tokio::task::spawn_blocking` 包装：
    ///
    /// ```ignore
    /// # use configx::ConfigSource;
    /// # fn example(source: impl ConfigSource) {
    /// let handle = tokio::task::spawn_blocking(move || source.load());
    /// let entries = handle.await??;
    /// # }
    /// ```
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
///
/// # 阻塞调用
///
/// `load()` 内部使用 `std::fs::read_to_string` 执行**阻塞文件 I/O**。
/// 异步调用方须用 `tokio::task::spawn_blocking` 隔离，避免冻结 tokio worker。
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

const MAX_APP_ID_BYTES: usize = 64;

fn validate_app_id(app_id: &str) -> ConfigxResult<()> {
    if app_id.is_empty() {
        return Err(ConfigxError::invalid("app_id 不能为空"));
    }
    if app_id.len() > MAX_APP_ID_BYTES {
        return Err(ConfigxError::invalid("app_id 长度超过 64 字节"));
    }
    if app_id.chars().any(char::is_control) {
        return Err(ConfigxError::invalid("app_id 不能包含控制字符"));
    }
    if app_id.contains('/') || app_id.contains('\\') {
        return Err(ConfigxError::invalid("app_id 不能包含路径分隔符"));
    }
    if app_id.contains("..") {
        return Err(ConfigxError::invalid("app_id 不能包含 .."));
    }
    Ok(())
}

/// 按进程环境解析全局配置文件路径。
///
/// 规则见 [`resolve_global_file_path_from_env`]。
pub fn resolve_global_file_path(app_id: &str) -> ConfigxResult<PathBuf> {
    resolve_global_file_path_from_env(app_id, env::vars_os())
}

/// 从给定环境映射解析全局配置文件路径（供测试注入）。
///
/// 优先 [`ENV_GLOBAL_FILE`] 的非空 trim 值；否则
/// `{XDG_CONFIG_HOME}/{app_id}/config` 或 `{HOME}/.config/{app_id}/config`。
///
/// # Errors
///
/// `app_id` 非法，或没有覆盖变量且缺少 `XDG_CONFIG_HOME` 与 `HOME`。
pub fn resolve_global_file_path_from_env(
    app_id: &str,
    vars: impl IntoIterator<Item = (OsString, OsString)>,
) -> ConfigxResult<PathBuf> {
    let mut map = HashMap::<String, String>::new();
    for (key, value) in vars {
        let Ok(key) = key.into_string() else {
            continue;
        };
        let Ok(value) = value.into_string() else {
            continue;
        };
        map.insert(key, value);
    }

    if let Some(raw) = map.get(ENV_GLOBAL_FILE) {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed));
        }
    }

    validate_app_id(app_id)?;

    if let Some(xdg) = map
        .get("XDG_CONFIG_HOME")
        .map(|item| item.trim())
        .filter(|item| !item.is_empty())
    {
        return Ok(PathBuf::from(xdg).join(app_id).join("config"));
    }
    if let Some(home) = map
        .get("HOME")
        .map(|item| item.trim())
        .filter(|item| !item.is_empty())
    {
        return Ok(PathBuf::from(home)
            .join(".config")
            .join(app_id)
            .join("config"));
    }

    Err(ConfigxError::invalid(
        "无法解析全局配置文件路径：未设置覆盖变量且缺少 XDG_CONFIG_HOME 与 HOME",
    ))
}

/// 可选全局 `KEY=VALUE` 文件源：文件不存在视为空映射；含 `secret:` 键则失败。
///
/// 不创建、不改写、不监视文件。调用方须显式 [`crate::ConfigxStore::register_source`]。
///
/// # 阻塞调用
///
/// `load()` 在文件存在时使用阻塞 `std::fs::read_to_string`。
#[derive(Debug, Clone)]
pub struct GlobalFileSource {
    path: PathBuf,
}

impl GlobalFileSource {
    /// 指定已解析的全局文件路径。
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

impl ConfigSource for GlobalFileSource {
    fn load(&self) -> ConfigxResult<HashMap<String, String>> {
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(HashMap::new());
            }
            Err(error) => {
                return Err(ConfigxError::io(
                    format!("读取全局配置文件失败：路径={}", self.path.display()),
                    error,
                ));
            }
        };
        let loaded = parse_key_value_file(&text)?;
        if loaded.keys().any(|key| is_secret_key(key)) {
            return Err(ConfigxError::invalid("全局配置文件不得包含 secret: 前缀键"));
        }
        Ok(loaded)
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

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable
    )]

    use super::*;
    use crate::ErrorKind;

    /// 唯一化临时文件路径（pid + 纳秒 + tag），避免并发用例互相覆盖。
    fn unique_temp_path(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "configx-src-test-{}-{}-{nanos}.kv",
            std::process::id(),
            tag
        ))
    }

    #[test]
    fn memory_source_round_trips_pairs() {
        assert!(MemorySource::new()
            .load()
            .expect("空源必须可加载")
            .is_empty());

        let source = MemorySource::from_pairs([("a", "1"), ("b", "2")]);
        let loaded = source.load().expect("加载成功");
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.get("a").map(String::as_str), Some("1"));
        assert_eq!(loaded.get("b").map(String::as_str), Some("2"));
    }

    #[test]
    fn memory_source_load_returns_an_independent_copy() {
        let source = MemorySource::from_pairs([("a", "1")]);
        let mut first = source.load().expect("加载成功");
        first.insert("a".to_string(), "mutated".to_string());
        first.insert("extra".to_string(), "x".to_string());

        let second = source.load().expect("再次加载");
        assert_eq!(
            second.get("a").map(String::as_str),
            Some("1"),
            "源内部状态不得被调用方改动"
        );
        assert!(!second.contains_key("extra"));
    }

    #[test]
    fn memory_source_debug_is_always_redacted() {
        let source =
            MemorySource::from_pairs([("secret:token", "top-secret"), ("plain", "visible")]);
        let rendered = format!("{source:?}");
        assert!(rendered.starts_with("MemorySource"), "{rendered}");
        assert!(
            rendered.contains("***"),
            "内存源 Debug 恒定脱敏：{rendered}"
        );
        assert!(!rendered.contains("top-secret"), "{rendered}");
        assert!(rendered.contains("visible"), "非敏感值保持可读：{rendered}");
    }

    #[test]
    fn env_source_exposes_prefix() {
        assert_eq!(EnvSource::new("APP_").prefix(), "APP_");
    }

    #[test]
    fn env_source_strips_prefix_and_skips_non_matching_keys() {
        let loaded = EnvSource::new("APP_")
            .load_from_iter([("APP_HOST", "h"), ("APP_PORT", "1"), ("OTHER", "x")])
            .expect("加载成功");
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.get("HOST").map(String::as_str), Some("h"));
        assert_eq!(loaded.get("PORT").map(String::as_str), Some("1"));
        assert!(!loaded.contains_key("OTHER"));
    }

    #[test]
    fn env_source_is_case_sensitive_and_skips_prefix_only_key() {
        let loaded = EnvSource::new("APP_")
            .load_from_iter([("app_host", "h"), ("APP_", "empty")])
            .expect("加载成功");
        assert!(
            loaded.is_empty(),
            "大小写不匹配与前缀后为空者都必须跳过：{loaded:?}"
        );
    }

    #[test]
    fn env_source_with_empty_prefix_returns_nothing() {
        let source = EnvSource::new("");
        assert!(source
            .load_from_iter([("ANY", "1")])
            .expect("加载成功")
            .is_empty());
        assert!(
            source.load().expect("生产路径加载成功").is_empty(),
            "空前缀不得误吞整张环境变量表"
        );
    }

    #[test]
    fn env_source_load_reads_real_environment() {
        // 注入真实进程环境变量，覆盖生产路径（含前缀剥离）。
        let name = "CONFIGX_SOURCE_TEST_HOST";
        std::env::set_var(name, "db.local");
        let loaded = EnvSource::new("CONFIGX_SOURCE_TEST_")
            .load()
            .expect("加载成功");
        std::env::remove_var(name);
        assert_eq!(loaded.get("HOST").map(String::as_str), Some("db.local"));
    }

    #[test]
    fn env_source_os_iter_accepts_unicode_entries() {
        let loaded = EnvSource::new("APP_")
            .load_from_os_iter([
                (OsString::from("APP_HOST"), OsString::from("h")),
                (OsString::from("OTHER"), OsString::from("x")),
            ])
            .expect("合法 Unicode 必须成功");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded.get("HOST").map(String::as_str), Some("h"));
    }

    #[cfg(unix)]
    #[test]
    fn env_source_reports_non_unicode_key_and_value() {
        use std::os::unix::ffi::OsStringExt;

        let bad_key = EnvSource::new("APP_")
            .load_from_os_iter([(
                OsString::from_vec(vec![0xff]),
                OsString::from_vec(b"v".to_vec()),
            )])
            .expect_err("非 Unicode 键必须报错");
        assert_eq!(bad_key.kind(), ErrorKind::Invalid);
        assert!(bad_key.to_string().contains("键"), "实际消息：{bad_key}");

        let bad_value = EnvSource::new("APP_")
            .load_from_os_iter([(OsString::from("APP_K"), OsString::from_vec(vec![0xff]))])
            .expect_err("非 Unicode 值必须报错");
        assert_eq!(bad_value.kind(), ErrorKind::Invalid);
        assert!(
            bad_value.to_string().contains("值"),
            "实际消息：{bad_value}"
        );
    }

    #[test]
    fn file_source_exposes_path_and_reads_file() {
        let path = unique_temp_path("ok");
        std::fs::write(&path, "# comment\nHOST=db.local\nPORT = 5432\n").expect("写夹具");
        let source = FileSource::new(&path);
        assert_eq!(source.path(), path.as_path());
        let loaded = source.load().expect("读取成功");
        let _ = std::fs::remove_file(&path);

        assert_eq!(loaded.get("HOST").map(String::as_str), Some("db.local"));
        assert_eq!(loaded.get("PORT").map(String::as_str), Some("5432"));
    }

    #[test]
    fn file_source_missing_file_reports_io_error_with_path() {
        let path = unique_temp_path("missing");
        let error = FileSource::new(&path).load().expect_err("缺失文件必须报错");
        assert_eq!(
            error.kind(),
            ErrorKind::Invalid,
            "文件缺失需人工修复，不可重试"
        );
        let rendered = error.to_string();
        assert!(
            rendered.contains(&path.display().to_string()),
            "应报告路径：{rendered}"
        );
        assert!(
            std::error::Error::source(&error).is_some(),
            "必须保留底层 I/O 错误"
        );
    }

    #[test]
    fn file_source_propagates_parse_error() {
        let path = unique_temp_path("bad");
        std::fs::write(&path, "NOT_A_PAIR\n").expect("写夹具");
        let error = FileSource::new(&path).load().expect_err("解析失败必须报错");
        let _ = std::fs::remove_file(&path);
        assert_eq!(error.kind(), ErrorKind::Invalid);
        assert!(error.to_string().contains("第 1 行"), "实际消息：{error}");
    }

    #[test]
    fn parse_skips_blank_and_comment_lines() {
        let parsed = parse_key_value_file("\n  \n# comment\n#\nA=1\n").expect("解析成功");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed.get("A").map(String::as_str), Some("1"));
        assert!(parse_key_value_file("").expect("空文本可解析").is_empty());
    }

    #[test]
    fn parse_trims_key_and_value() {
        let parsed = parse_key_value_file("  KEY  =  value with spaces  \n").expect("解析成功");
        assert_eq!(
            parsed.get("KEY").map(String::as_str),
            Some("value with spaces")
        );
    }

    #[test]
    fn parse_strips_exactly_one_layer_of_matching_quotes() {
        let parsed =
            parse_key_value_file("D=\"quoted\"\nS='single'\nN=\"'inner'\"\n").expect("解析成功");
        assert_eq!(parsed.get("D").map(String::as_str), Some("quoted"));
        assert_eq!(parsed.get("S").map(String::as_str), Some("single"));
        assert_eq!(
            parsed.get("N").map(String::as_str),
            Some("'inner'"),
            "只去一层引号"
        );
    }

    #[test]
    fn parse_keeps_unmatched_or_single_quotes_verbatim() {
        let parsed = parse_key_value_file("A=\"x\nB=x\"\nC=\"\n").expect("解析成功");
        assert_eq!(parsed.get("A").map(String::as_str), Some("\"x"));
        assert_eq!(parsed.get("B").map(String::as_str), Some("x\""));
        assert_eq!(parsed.get("C").map(String::as_str), Some("\""));
    }

    #[test]
    fn parse_allows_arbitrary_key_characters_and_keeps_inner_equals() {
        let parsed = parse_key_value_file("a.b-c/MIXED=1\nurl=http://x/?a=b\n").expect("解析成功");
        assert_eq!(parsed.get("a.b-c/MIXED").map(String::as_str), Some("1"));
        assert_eq!(
            parsed.get("url").map(String::as_str),
            Some("http://x/?a=b"),
            "值中的 = 必须保留"
        );
    }

    #[test]
    fn parse_last_duplicate_key_wins() {
        let parsed = parse_key_value_file("A=1\nA=2\n").expect("解析成功");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed.get("A").map(String::as_str), Some("2"));
    }

    #[test]
    fn parse_reports_line_number_without_echoing_content() {
        let error =
            parse_key_value_file("A=1\nSECRET_VALUE_WITHOUT_EQUALS\n").expect_err("缺 = 必须报错");
        assert_eq!(error.kind(), ErrorKind::Invalid);
        let rendered = error.to_string();
        assert!(rendered.contains("第 2 行"), "应报告行号：{rendered}");
        assert!(
            !rendered.contains("SECRET_VALUE_WITHOUT_EQUALS"),
            "不得回显原始行：{rendered}"
        );
    }

    #[test]
    fn parse_rejects_empty_key_with_line_number() {
        let error = parse_key_value_file("=value\n").expect_err("空键必须报错");
        assert_eq!(error.kind(), ErrorKind::Invalid);
        let rendered = error.to_string();
        assert!(rendered.contains("第 1 行"), "实际消息：{rendered}");
        assert!(rendered.contains("键为空"), "实际消息：{rendered}");
        assert!(!rendered.contains("value"), "不得回显值：{rendered}");
    }

    #[test]
    fn resolve_global_prefers_override_and_ignores_home() {
        let path = resolve_global_file_path_from_env(
            "app",
            [
                (
                    OsString::from(ENV_GLOBAL_FILE),
                    OsString::from("/tmp/g.conf"),
                ),
                (OsString::from("HOME"), OsString::from("/home/x")),
                (OsString::from("XDG_CONFIG_HOME"), OsString::from("/xdg")),
            ],
        )
        .expect("覆盖变量必须赢");
        assert_eq!(path, PathBuf::from("/tmp/g.conf"));
    }

    #[test]
    fn resolve_global_blank_override_falls_through_to_xdg() {
        let path = resolve_global_file_path_from_env(
            "myapp",
            [
                (OsString::from(ENV_GLOBAL_FILE), OsString::from("  ")),
                (OsString::from("XDG_CONFIG_HOME"), OsString::from("/xdg")),
            ],
        )
        .expect("空白覆盖视为未设");
        assert_eq!(path, PathBuf::from("/xdg/myapp/config"));
    }

    #[test]
    fn resolve_global_uses_home_dot_config() {
        let path = resolve_global_file_path_from_env(
            "myapp",
            [(OsString::from("HOME"), OsString::from("/home/x"))],
        )
        .expect("HOME 回退");
        assert_eq!(path, PathBuf::from("/home/x/.config/myapp/config"));
    }

    #[test]
    fn resolve_global_rejects_bad_app_id_and_missing_roots() {
        let slash = resolve_global_file_path_from_env("a/b", []).expect_err("分隔符");
        assert_eq!(slash.kind(), ErrorKind::Invalid);
        let missing = resolve_global_file_path_from_env("ok", []).expect_err("无根");
        assert_eq!(missing.kind(), ErrorKind::Invalid);
        assert!(!missing.to_string().contains("ok"));
    }

    #[test]
    fn global_file_source_missing_is_empty() {
        let path = unique_temp_path("global-missing");
        let loaded = GlobalFileSource::new(&path).load().expect("缺文件必须成功");
        assert!(loaded.is_empty());
    }

    #[test]
    fn global_file_source_rejects_secret_prefix_keys() {
        let path = unique_temp_path("global-secret");
        std::fs::write(&path, "host=local\nsecret:token=nope\n").expect("写夹具");
        let error = GlobalFileSource::new(&path)
            .load()
            .expect_err("secret: 必须失败");
        let _ = std::fs::remove_file(&path);
        assert_eq!(error.kind(), ErrorKind::Invalid);
        assert!(!error.to_string().contains("nope"));
    }
}
