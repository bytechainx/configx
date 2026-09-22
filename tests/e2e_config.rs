#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable
)]
//! E2E（configx）：在**真实文件 + 真实进程环境变量 + 真实分层存储行为**上端到端执行
//! **全部**公开接口。
//!
//! configx 没有外部服务，它的 E2E 面就是：真实临时文件（`KEY=VALUE` / TOML）、真实进程
//! 环境变量（`FOUNDATIONX_CONFIGX_*` 与自定义前缀）、以及真实的分层合并 / 重载 / 脱敏 /
//! 变更通知行为。
//!
//! 与 `layered_store_watch.rs`（分点单测）不同，本文件的对齐对象是
//! `cargo +nightly public-api --simplified` 导出的完整公开面：`fn` / `type` / `field` /
//! `const` / `variant` 五类逐条登记在 [`E2E_MANIFEST`]，运行期由 `cover` 登记表核对
//! 「声明 = 实际执行」（缺一即失败）。
//!
//! **独立核对**：`scripts/verify-e2e-coverage.mjs` 会重新派生公开面与清单双向 diff，并用
//! `-C instrument-coverage` + `llvm-cov report --show-functions` 断言每条公开函数执行次数
//! > 0；本文件内的登记表只是**声明**，不是唯一证据。
//!
//! 用例**不**需要任何凭据：临时目录名带 pid + 纳秒唯一化并在收尾删除且断言删除生效；
//! 进程环境变量逐条注入、逐条移除，收尾断言 `FOUNDATIONX_CONFIGX_*` 已清空。
//! configx 会改动**进程级**环境，故本文件只保留**一个**顺序驱动的 `#[test]`，
//! 以消除跨用例竞态。
//!
//! ```text
//! cd /home/workspace/bytechainx/configx
//! CARGO_TARGET_DIR=/home/workspace/bytechainx/.cargo/target cargo test --test e2e_config
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use configx::{
    diff_snapshots, is_secret_key, parse_key_value_file, redact_map, redact_value, snapshots_agree,
    subset_snapshot, try_subset_snapshot, ConfigChange, ConfigDiff, ConfigSource,
    ConfigWaitOutcome, ConfigWatch, ConfigxConfig, ConfigxConfigBuilder, ConfigxError,
    ConfigxHealth, ConfigxResult, ConfigxStore, EnvSource, ErrorKind, FileSource, LayeredConfig,
    MemorySource, ENV_ALLOW_EMPTY_SNAPSHOT, ENV_REDACT_SECRETS, REDACTED_VALUE, SECRET_KEY_PREFIX,
};

/// `FOUNDATIONX_CONFIGX_*` 前缀（本 crate 的环境变量命名空间）。
const CONFIGX_ENV_PREFIX: &str = "FOUNDATIONX_CONFIGX_";

/// 公开面清单：`(条目类别, 入口 id)`，由 `cargo +nightly public-api --simplified` 派生并冻结。
///
/// 类别取值域：`fn` / `type` / `field` / `const` / `variant`。
/// 该清单是运行时登记的**唯一事实源**——`cover::hit` 拒绝清单外的 id，收尾断言拒绝
/// 「声明了却没执行」的条目。清单本身的时效性由外部核对器与公开面 diff 保证。
const E2E_MANIFEST: &[(&str, &str)] = &[
    ("type", "ConfigWaitOutcome"),
    ("variant", "ConfigWaitOutcome::Changed"),
    ("variant", "ConfigWaitOutcome::Closed"),
    ("variant", "ConfigWaitOutcome::TimedOut"),
    ("type", "ConfigxError"),
    ("variant", "ConfigxError::Conflict"),
    ("variant", "ConfigxError::Invalid"),
    ("variant", "ConfigxError::Io"),
    ("variant", "ConfigxError::Missing"),
    ("variant", "ConfigxError::Parse"),
    ("variant", "ConfigxError::TypeMismatch"),
    ("variant", "ConfigxError::Unavailable"),
    ("variant", "ConfigxError::Unsupported"),
    ("fn", "ConfigxError::conflict"),
    ("fn", "ConfigxError::invalid"),
    ("fn", "ConfigxError::io"),
    ("fn", "ConfigxError::is_retryable"),
    ("fn", "ConfigxError::kind"),
    ("fn", "ConfigxError::missing"),
    ("fn", "ConfigxError::parse"),
    ("fn", "ConfigxError::type_mismatch"),
    ("fn", "ConfigxError::unavailable"),
    ("fn", "ConfigxError::unsupported"),
    ("type", "ErrorKind"),
    ("variant", "ErrorKind::Conflict"),
    ("variant", "ErrorKind::Invalid"),
    ("variant", "ErrorKind::Missing"),
    ("variant", "ErrorKind::Parse"),
    ("variant", "ErrorKind::TypeMismatch"),
    ("variant", "ErrorKind::Unavailable"),
    ("variant", "ErrorKind::Unsupported"),
    ("type", "ConfigChange"),
    ("field", "ConfigChange::generation"),
    ("type", "ConfigDiff"),
    ("field", "ConfigDiff::changed"),
    ("field", "ConfigDiff::only_left"),
    ("field", "ConfigDiff::only_right"),
    ("fn", "ConfigDiff::is_empty"),
    ("fn", "ConfigDiff::total_changes"),
    ("type", "ConfigSubscription"),
    ("fn", "ConfigSubscription::seen"),
    ("fn", "ConfigSubscription::wait"),
    ("fn", "ConfigSubscription::wait_outcome"),
    ("fn", "ConfigSubscription::wait_timeout"),
    ("fn", "ConfigSubscription::wait_timeout_outcome"),
    ("type", "ConfigWatch"),
    ("fn", "ConfigWatch::close"),
    ("fn", "ConfigWatch::generation"),
    ("fn", "ConfigWatch::new"),
    ("fn", "ConfigWatch::notify"),
    ("fn", "ConfigWatch::subscribe"),
    ("type", "ConfigxConfig"),
    ("field", "ConfigxConfig::allow_empty_snapshot"),
    ("field", "ConfigxConfig::redact_secrets"),
    ("fn", "ConfigxConfig::builder"),
    ("fn", "ConfigxConfig::from_env"),
    ("fn", "ConfigxConfig::from_toml"),
    ("fn", "ConfigxConfig::validate"),
    ("type", "ConfigxConfigBuilder"),
    ("fn", "ConfigxConfigBuilder::allow_empty_snapshot"),
    ("fn", "ConfigxConfigBuilder::build"),
    ("fn", "ConfigxConfigBuilder::new"),
    ("fn", "ConfigxConfigBuilder::redact_secrets"),
    ("type", "ConfigxHealth"),
    ("field", "ConfigxHealth::healthy"),
    ("field", "ConfigxHealth::keys"),
    ("field", "ConfigxHealth::sources"),
    ("type", "ConfigxStore"),
    ("fn", "ConfigxStore::config"),
    ("fn", "ConfigxStore::contains_key"),
    ("fn", "ConfigxStore::from_config"),
    ("fn", "ConfigxStore::generation"),
    ("fn", "ConfigxStore::get"),
    ("fn", "ConfigxStore::get_typed"),
    ("fn", "ConfigxStore::health_check"),
    ("fn", "ConfigxStore::is_empty"),
    ("fn", "ConfigxStore::len"),
    ("fn", "ConfigxStore::new"),
    ("fn", "ConfigxStore::notifier"),
    ("fn", "ConfigxStore::ping"),
    ("fn", "ConfigxStore::register_shared_source"),
    ("fn", "ConfigxStore::register_source"),
    ("fn", "ConfigxStore::reload"),
    ("fn", "ConfigxStore::snapshot"),
    ("fn", "ConfigxStore::source_count"),
    ("fn", "ConfigxStore::subscribe"),
    ("fn", "ConfigxStore::watch"),
    ("fn", "ConfigxStore::with_source"),
    ("type", "EnvSource"),
    ("fn", "EnvSource::load_from_iter"),
    ("fn", "EnvSource::new"),
    ("fn", "EnvSource::prefix"),
    ("type", "FileSource"),
    ("fn", "FileSource::new"),
    ("fn", "FileSource::path"),
    ("type", "LayeredConfig"),
    ("fn", "LayeredConfig::is_empty"),
    ("fn", "LayeredConfig::len"),
    ("fn", "LayeredConfig::load_merged"),
    ("fn", "LayeredConfig::new"),
    ("fn", "LayeredConfig::push"),
    ("fn", "LayeredConfig::with_source"),
    ("type", "MemorySource"),
    ("fn", "MemorySource::from_pairs"),
    ("fn", "MemorySource::new"),
    ("const", "ENV_ALLOW_EMPTY_SNAPSHOT"),
    ("const", "ENV_REDACT_SECRETS"),
    ("const", "REDACTED_VALUE"),
    ("const", "SECRET_KEY_PREFIX"),
    ("type", "ConfigSource"),
    ("fn", "ConfigSource::load"),
    ("fn", "diff_snapshots"),
    ("fn", "is_secret_key"),
    ("fn", "parse_key_value_file"),
    ("fn", "redact_map"),
    ("fn", "redact_value"),
    ("fn", "snapshots_agree"),
    ("fn", "subset_snapshot"),
    ("fn", "try_subset_snapshot"),
    ("type", "ConfigxResult"),
];

/// 覆盖登记表：只登记**真实发生**的调用/读取，不登记「计划要调用」。
mod cover {
    use std::collections::BTreeSet;
    use std::sync::{Mutex, OnceLock};

    static EXECUTED: OnceLock<Mutex<BTreeSet<(&'static str, &'static str)>>> = OnceLock::new();

    fn log() -> &'static Mutex<BTreeSet<(&'static str, &'static str)>> {
        EXECUTED.get_or_init(|| Mutex::new(BTreeSet::new()))
    }

    /// 登记一次真实执行。清单外的 `(类别, id)` 立即 panic，防止调用点与清单漂移。
    pub fn hit(kind: &'static str, id: &'static str) {
        assert!(
            super::E2E_MANIFEST
                .iter()
                .any(|(declared_kind, declared_id)| *declared_kind == kind && *declared_id == id),
            "登记了清单外的公开条目：{kind} {id}"
        );
        log().lock().expect("覆盖登记表锁中毒").insert((kind, id));
    }

    pub fn executed() -> BTreeSet<(&'static str, &'static str)> {
        log().lock().expect("覆盖登记表锁中毒").clone()
    }
}

/// 覆盖登记的简写入口（保持调用点可读）。
fn hit(kind: &'static str, id: &'static str) {
    cover::hit(kind, id);
}

/// 清单自身良构：类别取值域合法、`(类别, id)` 不重复。
fn assert_manifest_wellformed() {
    let mut seen: BTreeSet<(&str, &str)> = BTreeSet::new();
    for (kind, id) in E2E_MANIFEST {
        assert!(
            matches!(*kind, "fn" | "type" | "field" | "const" | "variant"),
            "未知条目类别 {kind}（id={id}）"
        );
        assert!(seen.insert((kind, id)), "清单重复条目：{kind} {id}");
    }
    assert!(!E2E_MANIFEST.is_empty(), "清单不得为空");
}

/// 收尾断言：声明集合与执行集合必须**双向相等**。
fn assert_coverage_complete() {
    let declared: BTreeSet<(&str, &str)> = E2E_MANIFEST.iter().copied().collect();
    let executed = cover::executed();

    let missing: Vec<&(&str, &str)> = declared.difference(&executed).collect();
    let ghost: Vec<&(&str, &str)> = executed.difference(&declared).collect();

    assert!(
        missing.is_empty(),
        "以下 {} 条公开条目被声明却未执行：{missing:?}",
        missing.len()
    );
    assert!(
        ghost.is_empty(),
        "以下 {} 条执行未登记在清单：{ghost:?}",
        ghost.len()
    );
    eprintln!(
        "E2E 覆盖：{}/{} 条公开条目全部执行（configx）",
        executed.len(),
        declared.len()
    );
}

/// 进程内唯一的资源名：`<前缀>_<pid>_<纳秒>`。
fn unique_name(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("系统时钟应晚于 UNIX_EPOCH")
        .as_nanos();
    format!("{prefix}_{}_{}", std::process::id(), nanos)
}

/// 临时目录（收尾删除并断言删除生效）。
fn unique_temp_dir(prefix: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(unique_name(prefix));
    std::fs::create_dir_all(&dir).expect("创建临时目录必须成功");
    dir
}

/// 进程内唯一的应用环境变量前缀（用于 `EnvSource` 的真实环境变量路径）。
fn unique_env_prefix() -> String {
    format!("CONFIGX_E2E_{}_", unique_name("APP").to_uppercase())
}

/// 清空并断言不存在 `FOUNDATIONX_CONFIGX_*`：用例不得依赖环境中的残留变量。
fn clear_configx_env() {
    let names: Vec<String> = std::env::vars_os()
        .filter_map(|(key, _)| key.into_string().ok())
        .collect();
    for name in &names {
        if name.starts_with(CONFIGX_ENV_PREFIX) {
            std::env::remove_var(name);
        }
    }
    assert_no_configx_env_left();
}

/// 断言 `FOUNDATIONX_CONFIGX_*` 一个不剩。
fn assert_no_configx_env_left() {
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy().into_owned();
        assert!(
            !name.starts_with(CONFIGX_ENV_PREFIX),
            "用例收尾后不得残留 {CONFIGX_ENV_PREFIX}* 环境变量：{name}"
        );
    }
}

/// 阶段 1：4 个公开常量逐条取值断言。
fn phase_constants() {
    hit("const", "ENV_REDACT_SECRETS");
    assert!(
        ENV_REDACT_SECRETS.starts_with(CONFIGX_ENV_PREFIX),
        "ENV_REDACT_SECRETS 必须带前缀 {CONFIGX_ENV_PREFIX}，实际 {ENV_REDACT_SECRETS}"
    );
    assert_eq!(ENV_REDACT_SECRETS, "FOUNDATIONX_CONFIGX_REDACT_SECRETS");

    hit("const", "ENV_ALLOW_EMPTY_SNAPSHOT");
    assert!(
        ENV_ALLOW_EMPTY_SNAPSHOT.starts_with(CONFIGX_ENV_PREFIX),
        "ENV_ALLOW_EMPTY_SNAPSHOT 必须带前缀 {CONFIGX_ENV_PREFIX}，实际 {ENV_ALLOW_EMPTY_SNAPSHOT}"
    );
    assert_eq!(
        ENV_ALLOW_EMPTY_SNAPSHOT,
        "FOUNDATIONX_CONFIGX_ALLOW_EMPTY_SNAPSHOT"
    );

    assert_ne!(
        ENV_REDACT_SECRETS, ENV_ALLOW_EMPTY_SNAPSHOT,
        "两个常量不得重复"
    );

    hit("const", "REDACTED_VALUE");
    assert_eq!(REDACTED_VALUE, "***");
    hit("const", "SECRET_KEY_PREFIX");
    assert_eq!(SECRET_KEY_PREFIX, "secret:");
}

/// 阶段 2：值类型（枚举变体逐个构造、错误分类逐条断言）。
fn phase_value_types() {
    // —— ConfigChange：类型 + 1 字段 ——
    hit("type", "ConfigChange");
    let change = ConfigChange { generation: 7 };
    let ConfigChange { generation } = change;
    hit("field", "ConfigChange::generation");
    assert_eq!(generation, 7);

    // —— ConfigWaitOutcome：类型 + 3 个变体显式构造 ——
    hit("type", "ConfigWaitOutcome");
    hit("variant", "ConfigWaitOutcome::Changed");
    let changed = ConfigWaitOutcome::Changed(ConfigChange { generation: 3 });
    hit("variant", "ConfigWaitOutcome::TimedOut");
    let timed_out = ConfigWaitOutcome::TimedOut;
    hit("variant", "ConfigWaitOutcome::Closed");
    let closed = ConfigWaitOutcome::Closed;
    assert_eq!(
        changed,
        ConfigWaitOutcome::Changed(ConfigChange { generation: 3 })
    );
    assert_ne!(timed_out, closed);
    assert!(format!("{changed:?}").contains("Changed"));

    // —— ConfigxError：类型 + 8 个变体 + 8 个构造器 + kind + is_retryable ——
    hit("type", "ConfigxError");
    // Io 变体由**真实失败的读取**构造，不使用人造错误。
    let absent = std::env::temp_dir().join(unique_name("configx_e2e_absent"));
    let io_source =
        std::fs::read_to_string(&absent).expect_err("读取不存在的文件必须返回 I/O 错误");
    hit("variant", "ConfigxError::Io");
    let explicit_io = ConfigxError::Io {
        message: "显式构造".to_string(),
        source: std::io::Error::other("e2e"),
    };
    assert_eq!(explicit_io.kind(), ErrorKind::Invalid);

    let constructors: [&str; 8] = [
        "ConfigxError::invalid",
        "ConfigxError::missing",
        "ConfigxError::type_mismatch",
        "ConfigxError::conflict",
        "ConfigxError::parse",
        "ConfigxError::unsupported",
        "ConfigxError::unavailable",
        "ConfigxError::io",
    ];
    let errors: [(&str, ConfigxError, ErrorKind, bool); 8] = [
        (
            "ConfigxError::Invalid",
            ConfigxError::invalid("e2e"),
            ErrorKind::Invalid,
            false,
        ),
        (
            "ConfigxError::Missing",
            ConfigxError::missing("e2e"),
            ErrorKind::Missing,
            false,
        ),
        (
            "ConfigxError::TypeMismatch",
            ConfigxError::type_mismatch("e2e"),
            ErrorKind::TypeMismatch,
            false,
        ),
        (
            "ConfigxError::Conflict",
            ConfigxError::conflict("e2e"),
            ErrorKind::Conflict,
            false,
        ),
        (
            "ConfigxError::Parse",
            ConfigxError::parse("e2e"),
            ErrorKind::Parse,
            false,
        ),
        (
            "ConfigxError::Unsupported",
            ConfigxError::unsupported("e2e"),
            ErrorKind::Unsupported,
            false,
        ),
        (
            "ConfigxError::Unavailable",
            ConfigxError::unavailable("e2e"),
            ErrorKind::Unavailable,
            true,
        ),
        (
            "ConfigxError::Io",
            ConfigxError::io("e2e", io_source),
            ErrorKind::Invalid,
            false,
        ),
    ];
    for (index, (id, error, kind, retryable)) in errors.into_iter().enumerate() {
        hit("variant", id);
        hit("fn", constructors[index]);
        hit("fn", "ConfigxError::kind");
        assert_eq!(error.kind(), kind, "{id} 的分类不符合契约");
        hit("fn", "ConfigxError::is_retryable");
        assert_eq!(
            error.is_retryable(),
            retryable,
            "{id} 的可重试分类不符合契约"
        );
        assert!(!error.to_string().is_empty(), "{id} 的 Display 不得为空");
    }

    // —— ErrorKind：类型 + 7 个变体逐个构造并去重 ——
    hit("type", "ErrorKind");
    let kinds = [
        ErrorKind::Invalid,
        ErrorKind::Missing,
        ErrorKind::TypeMismatch,
        ErrorKind::Conflict,
        ErrorKind::Parse,
        ErrorKind::Unsupported,
        ErrorKind::Unavailable,
    ];
    let kind_ids = [
        "ErrorKind::Invalid",
        "ErrorKind::Missing",
        "ErrorKind::TypeMismatch",
        "ErrorKind::Conflict",
        "ErrorKind::Parse",
        "ErrorKind::Unsupported",
        "ErrorKind::Unavailable",
    ];
    let mut seen_kinds = BTreeSet::new();
    for (kind, id) in kinds.into_iter().zip(kind_ids) {
        hit("variant", id);
        assert!(
            seen_kinds.insert(format!("{kind:?}")),
            "{id} 与其它变体重复"
        );
    }
    assert_eq!(seen_kinds.len(), 7);

    // —— ConfigxResult：成功 / 失败两条路径 ——
    hit("type", "ConfigxResult");
    let ok: ConfigxResult<u8> = Ok(7);
    match ok {
        Ok(value) => assert_eq!(value, 7),
        Err(error) => panic!("必须是 Ok 分支，实际 {error}"),
    }
    let err: ConfigxResult<u8> = Err(ConfigxError::invalid("e2e"));
    assert!(err.is_err(), "Err 分支必须保持错误");

    // —— ConfigSource：以 trait 对象使用，并真实执行 trait 方法 load ——
    hit("type", "ConfigSource");
    let shared: Arc<dyn ConfigSource> = Arc::new(MemorySource::from_pairs([("trait.key", "v")]));
    hit("fn", "ConfigSource::load");
    let map = shared.load().expect("共享源必须可加载");
    assert_eq!(map.get("trait.key").map(String::as_str), Some("v"));
}

/// 阶段 3：配置面 —— `from_env` 读真实 `FOUNDATIONX_CONFIGX_*`，`from_toml` / 构建器。
fn phase_config_plane() {
    hit("type", "ConfigxConfig");

    // 未设置变量 → 两个默认值都是 true。
    hit("fn", "ConfigxConfig::from_env");
    let defaults = ConfigxConfig::from_env().expect("未设置变量时必须回落到默认值");
    assert!(defaults.redact_secrets && defaults.allow_empty_snapshot);

    // 注入真实进程环境变量。
    std::env::set_var(ENV_REDACT_SECRETS, "false");
    std::env::set_var(ENV_ALLOW_EMPTY_SNAPSHOT, "off");
    let from_env = ConfigxConfig::from_env().expect("合法布尔环境变量必须被解析");
    assert!(!from_env.redact_secrets, "false 必须被解析为 false");
    assert!(!from_env.allow_empty_snapshot, "off 必须被解析为 false");
    std::env::remove_var(ENV_REDACT_SECRETS);
    std::env::remove_var(ENV_ALLOW_EMPTY_SNAPSHOT);
    assert!(
        std::env::var(ENV_REDACT_SECRETS).is_err(),
        "收尾必须移除注入的环境变量"
    );
    assert!(std::env::var(ENV_ALLOW_EMPTY_SNAPSHOT).is_err());

    // 非法布尔值 → Invalid，且错误消息不得回显原始值。
    std::env::set_var(ENV_REDACT_SECRETS, "definitely-not-a-bool");
    let bad_bool = ConfigxConfig::from_env().expect_err("非法布尔值必须被拒绝");
    assert_eq!(bad_bool.kind(), ErrorKind::Invalid);
    assert!(
        !bad_bool.to_string().contains("definitely-not-a-bool"),
        "错误消息不得回显原始值"
    );
    std::env::remove_var(ENV_REDACT_SECRETS);

    // validate 恒通过（当前字段集合无约束）。
    hit("fn", "ConfigxConfig::validate");
    from_env.validate().expect("合法配置必须通过校验");

    // 默认值穷尽解构（不写 `..`）：新增公开字段会在此处编译失败。
    let ConfigxConfig {
        redact_secrets,
        allow_empty_snapshot,
    } = ConfigxConfig::default();
    hit("field", "ConfigxConfig::redact_secrets");
    assert!(redact_secrets);
    hit("field", "ConfigxConfig::allow_empty_snapshot");
    assert!(allow_empty_snapshot);

    // from_toml：成功路径 + 解析失败路径。
    hit("fn", "ConfigxConfig::from_toml");
    let toml = ConfigxConfig::from_toml("redact_secrets = false\nallow_empty_snapshot = false\n")
        .expect("合法 TOML 必须可解析");
    assert!(!toml.redact_secrets && !toml.allow_empty_snapshot);
    let parse_error =
        ConfigxConfig::from_toml("redact_secrets = \n").expect_err("非法 TOML 必须被拒绝");
    assert_eq!(parse_error.kind(), ErrorKind::Parse);

    // 构建器：4 个公开方法 + ConfigxConfig::builder。
    hit("type", "ConfigxConfigBuilder");
    hit("fn", "ConfigxConfigBuilder::new");
    let built = ConfigxConfigBuilder::new()
        .redact_secrets(true)
        .allow_empty_snapshot(false)
        .build()
        .expect("构建器产出的配置必须合法");
    hit("fn", "ConfigxConfigBuilder::redact_secrets");
    hit("fn", "ConfigxConfigBuilder::allow_empty_snapshot");
    hit("fn", "ConfigxConfigBuilder::build");
    assert!(built.redact_secrets);
    assert!(!built.allow_empty_snapshot);

    hit("fn", "ConfigxConfig::builder");
    let via_builder = ConfigxConfig::builder()
        .allow_empty_snapshot(true)
        .build()
        .expect("构建器产出的配置必须合法");
    assert!(via_builder.allow_empty_snapshot);
}

/// 阶段 4：真实文件面（`KEY=VALUE` 文件 + TOML 文件 + 真实 I/O 失败）。
///
/// 返回临时目录与 `KEY=VALUE` 文件路径，供阶段 5 的分层合并继续使用；目录在阶段 8 删除。
fn phase_file_plane() -> (PathBuf, PathBuf) {
    let dir = unique_temp_dir("configx_e2e");
    let kv_path = dir.join("app.conf");
    let kv_text = "# 注释行\nHOST=db.local\nPORT=\"5432\"\nsecret:token=s3cr3t-raw\nEMPTY=\n";
    std::fs::write(&kv_path, kv_text).expect("写 KEY=VALUE 文件必须成功");

    // —— FileSource ——
    hit("type", "FileSource");
    let source = FileSource::new(&kv_path);
    hit("fn", "FileSource::new");
    hit("fn", "FileSource::path");
    assert_eq!(source.path(), kv_path.as_path());
    let loaded = source.load().expect("真实文件必须可加载");
    assert_eq!(loaded.get("HOST").map(String::as_str), Some("db.local"));
    assert_eq!(
        loaded.get("PORT").map(String::as_str),
        Some("5432"),
        "引号应去掉一层"
    );

    // —— parse_key_value_file（同一份真实文本）——
    hit("fn", "parse_key_value_file");
    let parsed = parse_key_value_file(kv_text).expect("合法 KEY=VALUE 文本必须可解析");
    assert_eq!(parsed.get("HOST").map(String::as_str), Some("db.local"));
    assert_eq!(parsed.get("PORT").map(String::as_str), Some("5432"));
    assert_eq!(parsed.get("EMPTY").map(String::as_str), Some(""));
    assert_eq!(
        parsed.get("secret:token").map(String::as_str),
        Some("s3cr3t-raw")
    );

    // 拒绝路径：非注释行缺 `=` / 键为空；错误消息只带行号，不回显原始行。
    let missing_separator = parse_key_value_file("looks-secret\n").expect_err("缺分隔符必须被拒绝");
    assert_eq!(missing_separator.kind(), ErrorKind::Invalid);
    assert!(missing_separator.to_string().contains("第 1 行"));
    assert!(
        !missing_separator.to_string().contains("looks-secret"),
        "不得回显原始行"
    );
    let empty_key = parse_key_value_file("=v\n").expect_err("空键必须被拒绝");
    assert_eq!(empty_key.kind(), ErrorKind::Invalid);

    // —— 真实 TOML 文件 → ConfigxConfig::from_toml ——
    let toml_path = dir.join("config.toml");
    std::fs::write(
        &toml_path,
        "redact_secrets = false\nallow_empty_snapshot = true\n",
    )
    .expect("写 TOML 文件必须成功");
    let toml_text = std::fs::read_to_string(&toml_path).expect("读 TOML 文件必须成功");
    let from_file = ConfigxConfig::from_toml(&toml_text).expect("TOML 文件内容必须可解析");
    assert!(!from_file.redact_secrets);

    // —— 真实 I/O 失败 → ConfigxError::Io，并保留底层 source ——
    let absent_path = dir.join("does-not-exist.conf");
    let io_error = FileSource::new(&absent_path)
        .load()
        .expect_err("读取不存在的文件必须失败");
    assert_eq!(io_error.kind(), ErrorKind::Invalid);
    assert!(
        std::error::Error::source(&io_error).is_some(),
        "I/O 错误必须保留底层 source"
    );

    (dir, kv_path)
}

/// 阶段 5：分层存储面（真实分层覆盖 / 重载 generation / 失败不改状态 / 脱敏 / 视图）。
fn phase_store_plane(kv_path: &Path) {
    // 低优先级：真实文件；高优先级：真实环境变量（后注册者覆盖先注册者）。
    let prefix = unique_env_prefix();
    let host_var = format!("{prefix}HOST");
    let port_var = format!("{prefix}PORT");
    std::env::set_var(&host_var, "env-host");
    std::env::set_var(&port_var, "9999");

    hit("type", "ConfigxStore");
    let mut store = ConfigxStore::new();
    hit("fn", "ConfigxStore::new");
    store.register_source(FileSource::new(kv_path));
    hit("fn", "ConfigxStore::register_source");
    store.register_source(EnvSource::new(&prefix));
    hit("fn", "ConfigxStore::register_source");
    hit("type", "MemorySource");
    let shared: Arc<dyn ConfigSource> =
        Arc::new(MemorySource::from_pairs([("shared.only", "yes")]));
    hit("fn", "ConfigxStore::register_shared_source");
    store.register_shared_source(shared);
    hit("fn", "ConfigxStore::source_count");
    assert_eq!(store.source_count(), 3);

    store.reload().expect("三个源都必须加载成功");
    hit("fn", "ConfigxStore::reload");

    // 后注册覆盖先注册；低优先级层独有键保留。
    hit("fn", "ConfigxStore::get");
    assert_eq!(store.get("HOST"), Some("env-host"), "环境源必须覆盖文件源");
    assert_eq!(store.get("PORT"), Some("9999"));
    assert_eq!(
        store.get("secret:token"),
        Some("s3cr3t-raw"),
        "低优先级层独有键必须保留"
    );

    hit("fn", "ConfigxStore::contains_key");
    assert!(store.contains_key("HOST"));
    assert!(!store.contains_key("nope"));
    hit("fn", "ConfigxStore::len");
    assert_eq!(store.len(), 5);
    hit("fn", "ConfigxStore::is_empty");
    assert!(!store.is_empty());
    hit("fn", "ConfigxStore::generation");
    assert_eq!(store.generation(), 1, "成功的 reload 必须推进 generation");
    hit("fn", "ConfigxStore::config");
    assert!(store.config().redact_secrets);
    hit("fn", "ConfigxStore::snapshot");
    let snapshot = store.snapshot();
    assert_eq!(snapshot.len(), 5);

    hit("fn", "ConfigxStore::ping");
    store.ping().expect("至少一个源已加载 → ping 必须通过");
    hit("fn", "ConfigxStore::health_check");
    let health = store.health_check().expect("health_check 当前不产生错误");
    hit("type", "ConfigxHealth");
    let ConfigxHealth {
        sources,
        keys,
        healthy,
    } = health;
    hit("field", "ConfigxHealth::sources");
    assert_eq!(sources, 3);
    hit("field", "ConfigxHealth::keys");
    assert_eq!(keys, 5);
    hit("field", "ConfigxHealth::healthy");
    assert!(healthy);

    // —— 类型化读取：成功 / 缺失 / 类型不匹配 ——
    hit("fn", "ConfigxStore::get_typed");
    let port = store
        .get_typed::<u16>("PORT")
        .expect("数值字面量必须能转成 u16");
    assert_eq!(port, 9999);
    let token = store
        .get_typed::<String>("secret:token")
        .expect("裸字符串必须能转成 String");
    assert_eq!(token, "s3cr3t-raw");
    let missing = store.get_typed::<u16>("nope").expect_err("缺失键必须报错");
    assert_eq!(missing.kind(), ErrorKind::Missing);
    let mismatch = store
        .get_typed::<u16>("secret:token")
        .expect_err("非数值必须报类型不匹配");
    assert_eq!(mismatch.kind(), ErrorKind::TypeMismatch);
    assert!(
        !mismatch.to_string().contains("s3cr3t-raw"),
        "类型错误不得回显配置值"
    );

    // —— 脱敏：`get` 返回原始值，展示路径才脱敏 ——
    let debug_text = format!("{store:?}");
    assert!(debug_text.contains(REDACTED_VALUE), "Debug 必须含脱敏占位");
    assert!(
        !debug_text.contains("s3cr3t-raw"),
        "Debug 不得泄漏原始敏感值"
    );
    hit("fn", "is_secret_key");
    assert!(is_secret_key("secret:token"));
    assert!(!is_secret_key("HOST"));
    // 说明：`redact_value<'a>` 是带显式生命周期的自由函数，仍属公开面；真实调用后登记。
    hit("fn", "redact_value");
    assert_eq!(redact_value("secret:token", "s3cr3t-raw"), REDACTED_VALUE);
    assert_eq!(redact_value("HOST", "env-host"), "env-host");
    hit("fn", "redact_map");
    let redacted = redact_map(&snapshot);
    assert_eq!(
        redacted.get("secret:token").map(String::as_str),
        Some(REDACTED_VALUE)
    );
    assert_eq!(
        redacted.get("HOST").map(String::as_str),
        Some("env-host"),
        "非敏感键必须原样保留"
    );
    assert_eq!(
        store.get("secret:token"),
        Some("s3cr3t-raw"),
        "读取永远返回原始值"
    );

    // —— 视图：子集快照与一致性比较 ——
    hit("fn", "subset_snapshot");
    let subset = subset_snapshot(&store, &["HOST", "secret:token", "missing"]);
    assert_eq!(subset.len(), 2, "缺失键只是被跳过");
    hit("fn", "try_subset_snapshot");
    let tried = try_subset_snapshot(&store, &["HOST"]).expect("合法键必须成功");
    assert_eq!(tried.get("HOST").map(String::as_str), Some("env-host"));
    let bad_key = try_subset_snapshot(&store, &[""]).expect_err("空键必须被拒绝");
    assert_eq!(bad_key.kind(), ErrorKind::Invalid);
    assert!(
        subset_snapshot(&store, &[""]).is_empty(),
        "非法键折叠为空快照"
    );
    hit("fn", "snapshots_agree");
    assert!(snapshots_agree(
        &snapshot,
        &subset,
        &["HOST", "secret:token"]
    ));
    assert!(!snapshots_agree(&snapshot, &subset, &["shared.only"]));
    assert!(
        snapshots_agree(&snapshot, &subset, &["absent"]),
        "两侧都缺失也算一致"
    );

    // —— 差异视图 ——
    hit("fn", "diff_snapshots");
    let mut left = BTreeMap::new();
    left.insert("a".to_string(), "1".to_string());
    left.insert("b".to_string(), "2".to_string());
    let mut right = BTreeMap::new();
    right.insert("a".to_string(), "9".to_string());
    right.insert("c".to_string(), "3".to_string());
    let diff = diff_snapshots(&left, &right);
    hit("type", "ConfigDiff");
    hit("fn", "ConfigDiff::is_empty");
    assert!(!diff.is_empty());
    hit("fn", "ConfigDiff::total_changes");
    assert_eq!(diff.total_changes(), 3);
    let ConfigDiff {
        only_left,
        only_right,
        changed,
    } = diff;
    hit("field", "ConfigDiff::only_left");
    assert_eq!(only_left, vec!["b".to_string()]);
    hit("field", "ConfigDiff::only_right");
    assert_eq!(only_right, vec!["c".to_string()]);
    hit("field", "ConfigDiff::changed");
    assert_eq!(changed, vec!["a".to_string()]);

    // —— 失败不改状态：新增一个缺失文件源后 reload 必须失败且快照/序号不变 ——
    store.register_source(FileSource::new("/no/such/configx_e2e_missing.conf"));
    let failed_reload = store.reload().expect_err("缺失文件必须让 reload 失败");
    assert_eq!(failed_reload.kind(), ErrorKind::Invalid);
    assert_eq!(store.generation(), 1, "失败的 reload 不得推进 generation");
    assert_eq!(store.get("HOST"), Some("env-host"), "旧快照必须完整保留");
    assert_eq!(store.snapshot(), snapshot, "旧 Arc 视图不得被替换");

    // —— 空快照策略：allow_empty_snapshot=false 时拒绝空合并结果 ——
    let strict_config = ConfigxConfig::builder()
        .allow_empty_snapshot(false)
        .build()
        .expect("合法配置必须可构建");
    hit("fn", "ConfigxStore::from_config");
    let mut strict_store =
        ConfigxStore::from_config(strict_config).expect("合法配置必须可构造存储");
    hit("fn", "MemorySource::new");
    strict_store.register_source(MemorySource::new());
    let conflict = strict_store.reload().expect_err("空快照必须被拒绝");
    assert_eq!(conflict.kind(), ErrorKind::Conflict);
    assert!(strict_store.is_empty());
    let ping_error = strict_store.ping().expect_err("无源加载 → ping 必须失败");
    assert_eq!(ping_error.kind(), ErrorKind::Unavailable);
    assert!(ping_error.is_retryable(), "Unavailable 是唯一可重试分类");

    // —— 链式注册 ——
    hit("fn", "ConfigxStore::with_source");
    let chained = ConfigxStore::new().with_source(MemorySource::from_pairs([("chain", "1")]));
    assert_eq!(chained.source_count(), 1);
    hit("fn", "MemorySource::from_pairs");

    // —— LayeredConfig 独立使用：push / with_source / load_merged ——
    hit("type", "LayeredConfig");
    hit("fn", "LayeredConfig::new");
    let mut layered = LayeredConfig::new();
    hit("fn", "LayeredConfig::with_source");
    layered = layered.with_source(Arc::new(MemorySource::from_pairs([
        ("k", "low"),
        ("only_low", "1"),
    ])));
    hit("fn", "LayeredConfig::push");
    layered.push(Arc::new(MemorySource::from_pairs([("k", "high")])));
    hit("fn", "LayeredConfig::len");
    assert_eq!(layered.len(), 2);
    hit("fn", "LayeredConfig::is_empty");
    assert!(!layered.is_empty());
    hit("fn", "LayeredConfig::load_merged");
    let merged = layered.load_merged().expect("两层必须合并成功");
    assert_eq!(merged.get("k").map(String::as_str), Some("high"));
    assert_eq!(merged.get("only_low").map(String::as_str), Some("1"));
    let empty_layered = LayeredConfig::new();
    assert!(empty_layered.is_empty());
    assert_eq!(empty_layered.len(), 0);
    assert!(empty_layered
        .load_merged()
        .expect("空层合并必须成功")
        .is_empty());

    // —— store 门面上的订阅句柄：watch / subscribe / notifier ——
    let mut watch_store = ConfigxStore::new();
    watch_store.register_source(MemorySource::from_pairs([("wk", "wv")]));
    hit("fn", "ConfigxStore::watch");
    let mut subscription = watch_store.watch();
    hit("fn", "ConfigxStore::subscribe");
    let _also = watch_store.subscribe();
    hit("fn", "ConfigxStore::notifier");
    let notifier = watch_store.notifier();
    assert_eq!(
        subscription
            .wait_timeout_outcome(Duration::from_millis(10))
            .expect("等待不得报错"),
        ConfigWaitOutcome::TimedOut,
        "未变更时必须是 TimedOut"
    );
    watch_store.reload().expect("重载必须成功");
    assert_eq!(
        subscription.wait_outcome().expect("等待不得报错"),
        ConfigWaitOutcome::Changed(ConfigChange { generation: 1 })
    );
    assert_eq!(watch_store.generation(), 1);
    assert_eq!(notifier.generation(), 1);
    notifier.close().expect("关闭变更总线必须成功");
    assert_eq!(
        subscription.wait_outcome().expect("等待不得报错"),
        ConfigWaitOutcome::Closed
    );
    assert!(
        watch_store.reload().is_err(),
        "总线关闭后 reload 必须失败（失败不改状态）"
    );

    // 收尾移除本阶段注入的环境变量。
    std::env::remove_var(&host_var);
    std::env::remove_var(&port_var);
    assert!(std::env::var(&host_var).is_err(), "收尾必须移除环境变量");
    assert!(std::env::var(&port_var).is_err());
}

/// 阶段 6：变更通知面（独立 `ConfigWatch` + 5 个订阅方法 + 三种结果形态）。
fn phase_watch_plane() {
    hit("type", "ConfigWatch");
    hit("fn", "ConfigWatch::new");
    let watch = Arc::new(ConfigWatch::new());
    hit("fn", "ConfigWatch::generation");
    assert_eq!(watch.generation(), 0);

    hit("fn", "ConfigWatch::subscribe");
    let mut subscription = watch.subscribe();
    hit("type", "ConfigSubscription");
    hit("fn", "ConfigSubscription::seen");
    assert_eq!(subscription.seen(), 0);

    // 无变更 → TimedOut（确定性判据，不依赖休眠后的墙钟猜测）。
    hit("fn", "ConfigSubscription::wait_timeout_outcome");
    assert_eq!(
        subscription
            .wait_timeout_outcome(Duration::from_millis(10))
            .expect("等待不得报错"),
        ConfigWaitOutcome::TimedOut
    );
    hit("fn", "ConfigSubscription::wait_timeout");
    assert!(subscription
        .wait_timeout(Duration::from_millis(1))
        .expect("等待不得报错")
        .is_none());

    // 真实广播 → Changed。
    hit("fn", "ConfigWatch::notify");
    let change = watch.notify().expect("广播必须成功");
    assert_eq!(change.generation, 1);
    hit("fn", "ConfigSubscription::wait_outcome");
    assert_eq!(
        subscription.wait_outcome().expect("等待不得报错"),
        ConfigWaitOutcome::Changed(ConfigChange { generation: 1 })
    );
    assert_eq!(subscription.seen(), 1);

    // 兼容接口 wait 折叠出 Some(变更)。
    hit("fn", "ConfigSubscription::wait");
    watch.notify().expect("二次广播必须成功");
    assert_eq!(
        subscription.wait().expect("等待不得报错"),
        Some(ConfigChange { generation: 2 })
    );

    // 关闭 → Closed；兼容接口折叠为 None。
    hit("fn", "ConfigWatch::close");
    watch.close().expect("关闭必须成功");
    assert_eq!(
        subscription.wait_outcome().expect("等待不得报错"),
        ConfigWaitOutcome::Closed
    );
    assert!(subscription.wait().expect("等待不得报错").is_none());
    assert!(subscription
        .wait_timeout(Duration::from_millis(1))
        .expect("等待不得报错")
        .is_none());
    assert!(watch.notify().is_err(), "关闭后广播必须失败");
}

/// 阶段 7：真实进程环境变量面（`EnvSource`，前缀剥离 + 收尾移除）。
fn phase_env_plane() {
    hit("type", "EnvSource");
    hit("fn", "EnvSource::new");
    hit("fn", "EnvSource::prefix");
    let empty_prefix = EnvSource::new("");
    assert_eq!(empty_prefix.prefix(), "");
    assert!(
        empty_prefix.load().expect("空前缀必须合法").is_empty(),
        "空前缀不得吞整张环境变量表"
    );

    let prefix = unique_env_prefix();
    let source = EnvSource::new(&prefix);
    assert_eq!(source.prefix(), prefix);

    let host_var = format!("{prefix}HOST");
    let port_var = format!("{prefix}PORT");
    std::env::set_var(&host_var, "env-host");
    std::env::set_var(&port_var, "9999");
    let loaded = source.load().expect("真实环境变量必须可加载");
    assert_eq!(loaded.get("HOST").map(String::as_str), Some("env-host"));
    assert_eq!(loaded.get("PORT").map(String::as_str), Some("9999"));
    std::env::remove_var(&host_var);
    std::env::remove_var(&port_var);
    assert!(std::env::var(&host_var).is_err(), "收尾必须移除环境变量");
    assert!(std::env::var(&port_var).is_err());

    // 依赖注入路径：前缀剥离规则与生产路径一致。
    hit("fn", "EnvSource::load_from_iter");
    let injected = source
        .load_from_iter([
            (format!("{prefix}X"), "1"),
            ("OTHER_PREFIX_X".to_string(), "2"),
            (prefix.clone(), "skip"),
        ])
        .expect("注入路径必须成功");
    assert_eq!(injected.get("X").map(String::as_str), Some("1"));
    assert_eq!(injected.len(), 1, "异前缀与前缀本身都必须被跳过");
}

/// 阶段 8：删除临时目录并断言删除生效。
fn cleanup_files(dir: &Path) {
    std::fs::remove_dir_all(dir).expect("清理临时目录必须成功");
    assert!(!dir.exists(), "清理后临时目录不得残留");
}

/// 单一驱动用例：configx 会改动**进程级**环境变量，顺序执行以消除跨用例竞态。
#[test]
fn e2e_config_all_public_api() {
    assert_manifest_wellformed();
    clear_configx_env();
    phase_constants();
    phase_value_types();
    phase_config_plane();
    let (dir, kv_path) = phase_file_plane();
    phase_store_plane(&kv_path);
    phase_watch_plane();
    phase_env_plane();
    cleanup_files(&dir);
    assert_no_configx_env_left();
    assert_coverage_complete();
}
