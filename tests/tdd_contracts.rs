#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable
)]
//! TDD 行为契约（特性 002）。
//!
//! 下表逐条登记 `specs/features/002-*/contracts/public-api-contract.md` 中 configx 的全部入口。
//! 公开 API 已存在，先写断言只能得到假断言，因此每条入口都在 `/tmp` 的变异副本上
//! 观测过红、再在本树观测绿；变异描述与复现命令见 PR 描述。
//!
//! // TDD-PROBE: ConfigxConfig::from_env | 变异：FOUNDATIONX_CONFIGX_REDACT_SECRETS=off 被忽略 | 红=from_env_reads_prefixed_switches | 绿=from_env_reads_prefixed_switches
//! // TDD-PROBE: ConfigxConfig::from_toml | 变异：TOML 语法错误被当作默认配置吞掉 | 红=from_toml_partial_and_fail_fast | 绿=from_toml_partial_and_fail_fast
//! // TDD-PROBE: ConfigxConfig::validate | 变异：validate 恒返回 Conflict | 红=validate_is_total_over_boolean_fields | 绿=validate_is_total_over_boolean_fields
//! // TDD-PROBE: ConfigxStore::new | 变异：new 预置一个已加载源 | 红=store_new_is_empty_and_unhealthy | 绿=store_new_is_empty_and_unhealthy
//! // TDD-PROBE: ConfigxStore::register_source | 变异：后注册源优先级反转 | 红=register_source_later_wins | 绿=register_source_later_wins
//! // TDD-PROBE: ConfigxStore::reload | 变异：失败路径仍替换快照并推进 generation | 红=reload_is_atomic_and_failure_preserving | 绿=reload_is_atomic_and_failure_preserving
//! // TDD-PROBE: ConfigxStore::ping | 变异：无已加载源时 ping 恒 Ok | 红=ping_requires_a_loaded_source | 绿=ping_requires_a_loaded_source
//! // TDD-PROBE: ConfigWatch::wait_timeout_outcome | 变异：等待恒定返回 Changed | 红=config_watch_wait_reports_change_timeout_and_closed | 绿=config_watch_wait_reports_change_timeout_and_closed
//! // TDD-PROBE: redact_map | 变异：redact_map 返回原始映射 | 红=redact_map_masks_secret_prefixed_values | 绿=redact_map_masks_secret_prefixed_values
//! // TDD-PROBE: is_secret_key | 变异：is_secret_key 取反 | 红=is_secret_key_is_prefix_based_and_case_sensitive | 绿=is_secret_key_is_prefix_based_and_case_sensitive

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use configx::{
    is_secret_key, redact_map, ConfigSubscription, ConfigWaitOutcome, ConfigWatch, ConfigxConfig,
    ConfigxStore, EnvSource, ErrorKind, FileSource, MemorySource, ENV_ALLOW_EMPTY_SNAPSHOT,
    ENV_REDACT_SECRETS,
};

/// 环境变量是进程级共享状态：本文件的 env 用例必须串行。
static ENV_LOCK: Mutex<()> = Mutex::new(());

const MANAGED_ENV: [&str; 2] = [ENV_REDACT_SECRETS, ENV_ALLOW_EMPTY_SNAPSHOT];

fn env_guard() -> MutexGuard<'static, ()> {
    ENV_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn clear_env() {
    for name in MANAGED_ENV {
        std::env::remove_var(name);
    }
}

/// `ConfigxConfig::from_env`：只识别 `FOUNDATIONX_CONFIGX_` 前缀，接受多形态布尔字面量。
#[test]
fn from_env_reads_prefixed_switches() {
    let _guard = env_guard();
    clear_env();

    assert_eq!(ConfigxConfig::from_env().unwrap(), ConfigxConfig::default());

    std::env::set_var(ENV_REDACT_SECRETS, "off");
    std::env::set_var(ENV_ALLOW_EMPTY_SNAPSHOT, "YES");
    let config = ConfigxConfig::from_env().unwrap();
    assert!(!config.redact_secrets, "off 必须映射为 false");
    assert!(
        config.allow_empty_snapshot,
        "YES 必须映射为 true（忽略大小写）"
    );

    // 只含空白视为未设置。
    std::env::set_var(ENV_REDACT_SECRETS, "   ");
    assert!(ConfigxConfig::from_env().unwrap().redact_secrets);

    // 非法布尔值：报错且不回显原始值。
    std::env::set_var(ENV_REDACT_SECRETS, "maybe");
    let error = ConfigxConfig::from_env().unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Invalid);
    assert!(!error.to_string().contains("maybe"), "不得回显原始值");

    clear_env();
    assert_eq!(ConfigxConfig::from_env().unwrap(), ConfigxConfig::default());
}

/// `ConfigxConfig::from_toml`：缺省字段取默认值，语法/类型错误 fail-fast。
#[test]
fn from_toml_partial_and_fail_fast() {
    let partial = ConfigxConfig::from_toml("redact_secrets = false\n").unwrap();
    assert!(!partial.redact_secrets);
    assert!(partial.allow_empty_snapshot, "缺省字段取默认值而不是零值");

    assert_eq!(
        ConfigxConfig::from_toml("").unwrap(),
        ConfigxConfig::default()
    );

    let syntax = ConfigxConfig::from_toml("redact_secrets = ").unwrap_err();
    assert_eq!(syntax.kind(), ErrorKind::Parse);
    let wrong_type = ConfigxConfig::from_toml("allow_empty_snapshot = 1").unwrap_err();
    assert_eq!(wrong_type.kind(), ErrorKind::Parse);
}

/// `ConfigxConfig::validate`：当前字段集合全为布尔量，故恒通过且不改变取值。
#[test]
fn validate_is_total_over_boolean_fields() {
    ConfigxConfig::default().validate().expect("默认配置合法");
    let toggled = ConfigxConfig::builder()
        .redact_secrets(false)
        .allow_empty_snapshot(false)
        .build()
        .unwrap();
    assert!(!toggled.redact_secrets);
    assert!(!toggled.allow_empty_snapshot);
    toggled.validate().expect("布尔字段无取值约束");
}

/// `ConfigxStore::new`：空存储——无源、空快照、不健康。
#[test]
fn store_new_is_empty_and_unhealthy() {
    let store = ConfigxStore::new();
    assert_eq!(store.source_count(), 0);
    assert!(store.is_empty());
    assert_eq!(store.len(), 0);
    assert_eq!(store.generation(), 0);
    assert_eq!(store.get("any"), None);

    let health = store.health_check().unwrap();
    assert_eq!(health.sources, 0);
    assert!(!health.healthy);
}

/// `ConfigxStore::register_source`：追加顺序即优先级，后注册者覆盖先注册者。
#[test]
fn register_source_later_wins() {
    let mut store = ConfigxStore::new();
    store.register_source(MemorySource::from_pairs([("k", "low"), ("only_low", "1")]));
    store.register_source(MemorySource::from_pairs([("k", "high")]));
    assert_eq!(store.source_count(), 2);
    store.reload().unwrap();

    assert_eq!(store.get("k"), Some("high"));
    assert_eq!(store.get("only_low"), Some("1"), "低优先级独有键保留");
}

/// `ConfigxStore::reload`：整体替换快照；任一失败路径都不改快照、不推进 generation。
#[test]
fn reload_is_atomic_and_failure_preserving() {
    let mut store = ConfigxStore::new();
    store.register_source(MemorySource::from_pairs([("a", "1")]));
    store.reload().unwrap();
    assert_eq!(store.generation(), 1);

    // 新一层整体替换：旧的独有键消失。
    store.register_source(MemorySource::from_pairs([("b", "2")]));
    store.reload().unwrap();
    assert_eq!(store.get("b"), Some("2"));
    assert_eq!(store.get("a"), Some("1"));
    assert_eq!(store.generation(), 2);

    // 源加载失败：快照与 generation 保持原样。
    let before = store.snapshot();
    store.register_source(FileSource::new("/no/such/configx-tdd.conf"));
    let error = store.reload().unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Invalid);
    assert_eq!(store.snapshot(), before);
    assert_eq!(store.generation(), 2);

    // 空快照被策略拒绝时同样保持原样。
    let policy = ConfigxConfig::builder()
        .allow_empty_snapshot(false)
        .build()
        .unwrap();
    let mut strict = ConfigxStore::from_config(policy).unwrap();
    strict.register_source(MemorySource::new());
    let error = strict.reload().unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Conflict);
    assert!(strict.is_empty());
    assert_eq!(strict.generation(), 0);
}

/// `ConfigxStore::ping`：只有「至少一个源已成功加载」才就绪，且可重试分类为 Unavailable。
#[test]
fn ping_requires_a_loaded_source() {
    let mut store = ConfigxStore::new();
    let error = store.ping().unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Unavailable);
    assert!(error.is_retryable(), "源暂不可用是唯一可安全重试的分类");

    // 注册但未 reload 仍视为未加载。
    store.register_source(MemorySource::new());
    assert!(store.ping().is_err());

    store.reload().unwrap();
    store.ping().expect("reload 之后必须恢复健康");
    assert_eq!(store.health_check().unwrap().sources, 1);
    assert!(store.health_check().unwrap().healthy);
}

/// `ConfigWatch` 的变更等待：显式区分「变更 / 超时 / 已关闭」，且无变更时不阻塞。
///
/// 契约登记的入口为 `ConfigWatch::wait_timeout_outcome`；本 crate 的实际形态是
/// `ConfigWatch::subscribe` 返回 `ConfigSubscription`，由订阅句柄承担阻塞与限时等待
/// （`wait_outcome` / `wait_timeout_outcome`），故此处按该形态断言等待语义。
#[test]
fn config_watch_wait_reports_change_timeout_and_closed() {
    let watch = Arc::new(ConfigWatch::new());
    let mut subscription: ConfigSubscription = watch.subscribe();
    assert_eq!(subscription.seen(), 0);

    // 无变更：限时等待必须超时返回，而不是永久阻塞。
    assert_eq!(
        subscription
            .wait_timeout_outcome(Duration::from_millis(20))
            .unwrap(),
        ConfigWaitOutcome::TimedOut
    );

    // 通知之后：观察到新的 generation（从 1 起）。
    let change = watch.notify().unwrap();
    assert_eq!(change.generation, 1);
    let outcome = subscription
        .wait_timeout_outcome(Duration::from_millis(200))
        .unwrap();
    assert_eq!(
        outcome,
        ConfigWaitOutcome::Changed(configx::ConfigChange { generation: 1 })
    );
    assert_eq!(subscription.seen(), 1);

    // 关闭之后：等待立即返回 Closed（关闭优先于「有新变更」）。
    watch.close().unwrap();
    assert_eq!(
        subscription.wait_outcome().unwrap(),
        ConfigWaitOutcome::Closed
    );
}

/// `redact_map`：只脱敏 `secret:` 前缀键，其余原样，且不修改入参。
#[test]
fn redact_map_masks_secret_prefixed_values() {
    let mut entries = BTreeMap::new();
    entries.insert("app.host".to_string(), "db.local".to_string());
    entries.insert("secret:token".to_string(), "top-secret".to_string());

    let redacted: BTreeMap<String, String> = redact_map(&entries);
    assert_eq!(
        redacted.get("app.host").map(String::as_str),
        Some("db.local")
    );
    assert_eq!(
        redacted.get("secret:token").map(String::as_str),
        Some("***")
    );
    assert_eq!(
        entries.get("secret:token").map(String::as_str),
        Some("top-secret"),
        "纯函数不得修改入参"
    );
    // 展示层职责：存储读取路径始终返回原始值。
    let mut store = ConfigxStore::new();
    store.register_source(MemorySource::from_pairs([("secret:token", "top-secret")]));
    store.reload().unwrap();
    assert_eq!(store.get("secret:token"), Some("top-secret"));
}

/// `is_secret_key`：判定只看 `secret:` 前缀，大小写敏感，不猜测词形。
#[test]
fn is_secret_key_is_prefix_based_and_case_sensitive() {
    assert!(is_secret_key("secret:token"));
    assert!(is_secret_key("secret:"));
    assert!(!is_secret_key("token"));
    assert!(!is_secret_key("password"));
    assert!(!is_secret_key("Secret:token"), "判定必须大小写敏感");
    assert!(!is_secret_key(""));

    // 环境变量源与前缀剥离语义（与脱敏判定互不影响）。
    let env = EnvSource::new("APP_");
    let loaded = env
        .load_from_iter([("APP_HOST", "h"), ("OTHER", "x"), ("APP_", "skip")])
        .unwrap();
    assert_eq!(loaded.get("HOST").map(String::as_str), Some("h"));
    assert!(!loaded.contains_key("OTHER"));
}
