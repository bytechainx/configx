#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable
)]
//! 配置校验与环境变量解析。
//!
//! 关于覆盖范围变更的说明：`ConfigxConfig` 原有第三个字段 `watch_channel_capacity: usize`
//! （环境变量 `FOUNDATIONX_CONFIGX_WATCH_CHANNEL_CAPACITY`，校验区间 `1..=65536`）。
//! 该字段已删除——`src/watch.rs` 的通知机制是 `Mutex` + `Condvar` 的 generation 计数器，
//! 根本不存在通道，容量值从未被任何实现读取，属死配置。
//!
//! 随之消失的是「数值越界 → 校验失败」这一类用例：剩余的 `redact_secrets` 与
//! `allow_empty_snapshot` 都是布尔量，`validate()` 已不存在可拒绝的取值，无法承接该覆盖。
//! 但仍被保留的失败路径是：环境变量非法布尔值（`ErrorKind::Invalid`，且不回显原始值）
//! 与 TOML 语法/类型错误（`ErrorKind::Parse`），二者在本文件中均有对应用例。

use configx::{
    ConfigxConfig, ConfigxConfigBuilder, ConfigxError, ErrorKind, ENV_ALLOW_EMPTY_SNAPSHOT,
    ENV_REDACT_SECRETS,
};

#[test]
fn defaults_are_valid_and_documented() {
    let config = ConfigxConfig::default();
    assert!(config.redact_secrets);
    assert!(config.allow_empty_snapshot);
    // 剩余字段均无取值约束，validate() 必须通过；该方法保留为将来新增约束的入口。
    config.validate().expect("默认配置必须合法");

    let built = ConfigxConfigBuilder::new().build().unwrap();
    assert_eq!(built, config);
    assert_eq!(ConfigxConfig::builder().build().unwrap(), config);
}

#[test]
fn builder_overrides_fields() {
    let config = ConfigxConfig::builder()
        .redact_secrets(false)
        .allow_empty_snapshot(false)
        .build()
        .unwrap();
    assert!(!config.redact_secrets);
    assert!(!config.allow_empty_snapshot);
    config.validate().expect("布尔字段无取值约束，校验必须通过");
}

#[test]
fn from_toml_accepts_partial_and_rejects_bad_input() {
    let full = ConfigxConfig::from_toml(
        r#"
redact_secrets = false
allow_empty_snapshot = false
"#,
    )
    .unwrap();
    assert!(!full.redact_secrets);
    assert!(!full.allow_empty_snapshot);

    // 未出现的字段使用默认值，而不是 `Default::default()` 的零值。
    let partial = ConfigxConfig::from_toml("redact_secrets = false\n").unwrap();
    assert!(!partial.redact_secrets);
    assert!(partial.allow_empty_snapshot);

    let empty = ConfigxConfig::from_toml("").unwrap();
    assert_eq!(empty, ConfigxConfig::default());

    let syntax_error = ConfigxConfig::from_toml("redact_secrets = ").unwrap_err();
    assert_eq!(syntax_error.kind(), ErrorKind::Parse);

    // 类型错误也必须被分类为解析失败。
    let wrong_type = ConfigxConfig::from_toml("redact_secrets = \"yes\"").unwrap_err();
    assert_eq!(wrong_type.kind(), ErrorKind::Parse);

    let wrong_type = ConfigxConfig::from_toml("allow_empty_snapshot = 1").unwrap_err();
    assert_eq!(wrong_type.kind(), ErrorKind::Parse);
}

#[test]
fn config_round_trips_through_toml() {
    let original = ConfigxConfig::builder()
        .redact_secrets(false)
        .allow_empty_snapshot(false)
        .build()
        .unwrap();
    let text = toml::to_string(&original).unwrap();
    let restored = ConfigxConfig::from_toml(&text).unwrap();
    assert_eq!(restored, original);
}

/// 本测试独占进程环境变量：其他测试不得读写 `FOUNDATIONX_CONFIGX_*`。
#[test]
fn from_env_reads_prefixed_variables_and_rejects_bad_values() {
    // 清理可能存在的残留，保证起点为默认值。
    clear_env();

    assert_eq!(ConfigxConfig::from_env().unwrap(), ConfigxConfig::default());

    std::env::set_var(ENV_REDACT_SECRETS, "off");
    std::env::set_var(ENV_ALLOW_EMPTY_SNAPSHOT, "YES");
    let config = ConfigxConfig::from_env().unwrap();
    assert!(!config.redact_secrets);
    assert!(config.allow_empty_snapshot);

    // 只含空白视为未设置。
    std::env::set_var(ENV_REDACT_SECRETS, "   ");
    assert!(ConfigxConfig::from_env().unwrap().redact_secrets);

    // 非法布尔值：报错但不得回显原始值。
    std::env::set_var(ENV_REDACT_SECRETS, "maybe");
    let error = ConfigxConfig::from_env().unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Invalid);
    assert!(!error.to_string().contains("maybe"));

    std::env::remove_var(ENV_REDACT_SECRETS);

    // 另一个布尔字段同样走非法值报错路径，且同样不回显原始值。
    std::env::set_var(ENV_ALLOW_EMPTY_SNAPSHOT, "1.5");
    let error = ConfigxConfig::from_env().unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Invalid);
    assert!(!error.to_string().contains("1.5"));

    clear_env();
    assert_eq!(ConfigxConfig::from_env().unwrap(), ConfigxConfig::default());
}

#[test]
fn config_error_messages_use_expected_prefixes() {
    let error: ConfigxError = ConfigxConfig::from_toml("=").unwrap_err();
    assert!(
        error.to_string().starts_with("解析失败: "),
        "实际消息：{error}"
    );
}

fn clear_env() {
    std::env::remove_var(ENV_REDACT_SECRETS);
    std::env::remove_var(ENV_ALLOW_EMPTY_SNAPSHOT);
}
