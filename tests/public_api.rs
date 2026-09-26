#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable
)]
//! 公共 API 表面：类型存在性、`Send + Sync`、错误分类、脱敏纯函数、类型化读取与健康检查。

use std::collections::BTreeMap;

use configx::{
    is_secret_key, redact_map, redact_value, ConfigChange, ConfigDiff, ConfigSource,
    ConfigSubscription, ConfigWaitOutcome, ConfigWatch, ConfigxConfig, ConfigxConfigBuilder,
    ConfigxError, ConfigxHealth, ConfigxResult, ConfigxStore, EnvSource, ErrorKind, FileSource,
    GlobalFileSource, LayeredConfig, MemorySource,
};

fn assert_send_sync<T: Send + Sync>() {}

fn sample_store() -> ConfigxStore {
    let mut store = ConfigxStore::new();
    store.register_source(MemorySource::from_pairs([
        ("app.host", "db.local"),
        ("app.port", "5432"),
        ("app.tls", "true"),
        ("app.hosts", r#"["a","b"]"#),
        ("secret:token", "top-secret"),
    ]));
    store.reload().expect("reload must succeed");
    store
}

#[test]
fn core_types_are_send_and_sync() {
    assert_send_sync::<ConfigxStore>();
    assert_send_sync::<ConfigxConfig>();
    assert_send_sync::<ConfigxConfigBuilder>();
    assert_send_sync::<ConfigxError>();
    assert_send_sync::<ConfigxHealth>();
    assert_send_sync::<ConfigDiff>();
    assert_send_sync::<LayeredConfig>();
    assert_send_sync::<ConfigWatch>();
    assert_send_sync::<ConfigSubscription>();
    assert_send_sync::<ConfigChange>();
    assert_send_sync::<ConfigWaitOutcome>();
    assert_send_sync::<MemorySource>();
    assert_send_sync::<EnvSource>();
    assert_send_sync::<FileSource>();
    assert_send_sync::<GlobalFileSource>();
}

#[test]
fn error_kind_and_retryability() {
    let invalid = ConfigxError::invalid("配置键不能为空");
    assert_eq!(invalid.kind(), ErrorKind::Invalid);
    assert!(!invalid.is_retryable());
    assert_eq!(ConfigxError::missing("k").kind(), ErrorKind::Missing);
    assert_eq!(
        ConfigxError::type_mismatch("k").kind(),
        ErrorKind::TypeMismatch
    );
    assert_eq!(ConfigxError::conflict("k").kind(), ErrorKind::Conflict);
    assert_eq!(ConfigxError::parse("k").kind(), ErrorKind::Parse);
    assert_eq!(
        ConfigxError::unsupported("k").kind(),
        ErrorKind::Unsupported
    );

    // 只有「源暂不可用」可重试。
    let unavailable = ConfigxError::unavailable("没有源");
    assert_eq!(unavailable.kind(), ErrorKind::Unavailable);
    assert!(unavailable.is_retryable());

    // I/O 失败（文件缺失等）需要人工修复，不属于瞬时故障。
    let io = ConfigxError::io(
        "读取失败",
        std::io::Error::new(std::io::ErrorKind::NotFound, "gone"),
    );
    assert_eq!(io.kind(), ErrorKind::Invalid);
    assert!(!io.is_retryable());
    assert!(
        std::error::Error::source(&io).is_some(),
        "必须保留底层 I/O 错误"
    );
}

#[test]
fn redaction_helpers_are_pure_and_prefix_based() {
    assert_eq!(configx::SECRET_KEY_PREFIX, "secret:");
    assert_eq!(configx::REDACTED_VALUE, "***");
    assert!(is_secret_key("secret:token"));
    assert!(!is_secret_key("token"));
    assert!(!is_secret_key("Secret:token"), "判定大小写敏感");

    assert_eq!(redact_value("secret:token", "abc"), "***");
    assert_eq!(redact_value("plain", "abc"), "abc");

    let mut entries = BTreeMap::new();
    entries.insert("plain".to_string(), "visible".to_string());
    entries.insert("secret:token".to_string(), "hidden".to_string());
    let redacted = redact_map(&entries);
    assert_eq!(redacted.get("plain").map(String::as_str), Some("visible"));
    assert_eq!(
        redacted.get("secret:token").map(String::as_str),
        Some("***")
    );
}

#[test]
fn typed_read_and_health_check() {
    let store = sample_store();

    // 读取路径永远返回原始值，脱敏不介入。
    assert_eq!(store.get("secret:token"), Some("top-secret"));
    assert_eq!(store.get_typed::<u16>("app.port").unwrap(), 5432);
    assert_eq!(store.get_typed::<String>("app.host").unwrap(), "db.local");
    assert!(store.get_typed::<bool>("app.tls").unwrap());
    assert_eq!(
        store.get_typed::<Vec<String>>("app.hosts").unwrap(),
        vec!["a", "b"]
    );

    let health = store.health_check().unwrap();
    assert_eq!(health.sources, 1);
    assert_eq!(health.keys, 5);
    assert!(health.healthy);
    store.ping().expect("加载成功的存储必须 ping 通");
}

#[test]
fn missing_and_type_mismatch_errors_do_not_leak_values() {
    let store = sample_store();

    let missing = store.get_typed::<u16>("app.absent").unwrap_err();
    assert_eq!(missing.kind(), ErrorKind::Missing);

    let mismatch = store.get_typed::<u16>("app.host").unwrap_err();
    assert_eq!(mismatch.kind(), ErrorKind::TypeMismatch);
    assert!(
        !mismatch.to_string().contains("db.local"),
        "错误消息不得回显配置值"
    );

    let secret_mismatch = store.get_typed::<u16>("secret:token").unwrap_err();
    assert_eq!(secret_mismatch.kind(), ErrorKind::TypeMismatch);
    assert!(
        !secret_mismatch.to_string().contains("top-secret"),
        "错误消息不得回显敏感值"
    );
}

#[test]
fn ping_fails_when_no_source_is_loaded() {
    let mut store = ConfigxStore::new();
    assert!(!store.health_check().unwrap().healthy);
    let error = store.ping().unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Unavailable);
    assert!(error.is_retryable());

    // 注册源但未 reload 时仍视为「未成功加载」。
    store.register_source(MemorySource::new());
    assert_eq!(store.source_count(), 1);
    assert!(store.ping().is_err());

    store.reload().unwrap();
    store.ping().expect("reload 之后必须恢复健康");
    assert_eq!(store.health_check().unwrap().sources, 1);
}

#[test]
fn store_debug_redacts_by_default_and_can_be_disabled() {
    let store = sample_store();
    let debug = format!("{store:?}");
    assert!(debug.contains("***"), "默认必须在 Debug 中脱敏：{debug}");
    assert!(!debug.contains("top-secret"));
    assert!(debug.contains("db.local"), "非敏感值应保持可读");

    let config = ConfigxConfig::builder()
        .redact_secrets(false)
        .build()
        .unwrap();
    let mut open = ConfigxStore::from_config(config).unwrap();
    open.register_source(MemorySource::from_pairs([("secret:token", "top-secret")]));
    open.reload().unwrap();
    assert!(
        format!("{open:?}").contains("top-secret"),
        "关闭脱敏后应输出原始值"
    );
}

#[test]
fn source_trait_is_object_safe_and_usable_as_dyn() {
    fn reload_via_alias(store: &mut ConfigxStore) -> ConfigxResult<()> {
        store.reload()
    }

    let source: std::sync::Arc<dyn ConfigSource> =
        std::sync::Arc::new(MemorySource::from_pairs([("k", "v")]));
    let mut store = ConfigxStore::new();
    store.register_shared_source(source);
    reload_via_alias(&mut store).expect("reload 必须成功");
    assert_eq!(store.get("k"), Some("v"));
}
