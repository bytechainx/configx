#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable
)]
//! 配置校验与环境变量解析。

use configx::{
    ConfigxConfig, ConfigxConfigBuilder, ConfigxError, ErrorKind, ENV_ALLOW_EMPTY_SNAPSHOT,
    ENV_REDACT_SECRETS, ENV_WATCH_CHANNEL_CAPACITY, MAX_WATCH_CHANNEL_CAPACITY,
};

#[test]
fn defaults_are_valid_and_documented() {
    let config = ConfigxConfig::default();
    assert!(config.redact_secrets);
    assert!(config.allow_empty_snapshot);
    assert_eq!(
        config.watch_channel_capacity,
        configx::DEFAULT_WATCH_CHANNEL_CAPACITY
    );
    config.validate().expect("默认配置必须合法");

    let built = ConfigxConfigBuilder::new().build().unwrap();
    assert_eq!(built, config);
    assert_eq!(ConfigxConfig::builder().build().unwrap(), config);
}

#[test]
fn builder_overrides_and_validates() {
    let config = ConfigxConfig::builder()
        .redact_secrets(false)
        .watch_channel_capacity(8)
        .allow_empty_snapshot(false)
        .build()
        .unwrap();
    assert!(!config.redact_secrets);
    assert_eq!(config.watch_channel_capacity, 8);
    assert!(!config.allow_empty_snapshot);

    let zero = ConfigxConfig::builder()
        .watch_channel_capacity(0)
        .build()
        .unwrap_err();
    assert_eq!(zero.kind(), ErrorKind::Invalid);
    assert!(zero.to_string().contains("容量必须大于 0"));

    let too_big = ConfigxConfig::builder()
        .watch_channel_capacity(MAX_WATCH_CHANNEL_CAPACITY + 1)
        .build()
        .unwrap_err();
    assert_eq!(too_big.kind(), ErrorKind::Invalid);
}

#[test]
fn from_toml_accepts_partial_and_rejects_bad_input() {
    let full = ConfigxConfig::from_toml(
        r#"
redact_secrets = false
watch_channel_capacity = 16
allow_empty_snapshot = false
"#,
    )
    .unwrap();
    assert!(!full.redact_secrets);
    assert_eq!(full.watch_channel_capacity, 16);
    assert!(!full.allow_empty_snapshot);

    // 未出现的字段使用默认值，而不是 `Default::default()` 的零值。
    let partial = ConfigxConfig::from_toml("redact_secrets = false\n").unwrap();
    assert!(!partial.redact_secrets);
    assert_eq!(
        partial.watch_channel_capacity,
        configx::DEFAULT_WATCH_CHANNEL_CAPACITY
    );
    assert!(partial.allow_empty_snapshot);

    let empty = ConfigxConfig::from_toml("").unwrap();
    assert_eq!(empty, ConfigxConfig::default());

    let syntax_error = ConfigxConfig::from_toml("watch_channel_capacity = ").unwrap_err();
    assert_eq!(syntax_error.kind(), ErrorKind::Parse);

    // 解析成功但校验失败：容量 0。
    let invalid = ConfigxConfig::from_toml("watch_channel_capacity = 0").unwrap_err();
    assert_eq!(invalid.kind(), ErrorKind::Invalid);

    // 类型错误也必须被分类为解析失败。
    let wrong_type = ConfigxConfig::from_toml("redact_secrets = \"yes\"").unwrap_err();
    assert_eq!(wrong_type.kind(), ErrorKind::Parse);
}

#[test]
fn config_round_trips_through_toml() {
    let original = ConfigxConfig::builder()
        .redact_secrets(false)
        .watch_channel_capacity(7)
        .allow_empty_snapshot(false)
        .build()
        .unwrap();
    let text = toml::to_string(&original).unwrap();
    let restored = ConfigxConfig::from_toml(&text).unwrap();
    assert_eq!(restored, original);
}

/// 本测试独占进程环境变量：其他测试不得读写 `FOUNDATIONX_CONFIGX_*`。
#[test]
fn from_env_reads_prefixed_variables_and_validates() {
    // 清理可能存在的残留，保证起点为默认值。
    clear_env();

    assert_eq!(ConfigxConfig::from_env().unwrap(), ConfigxConfig::default());

    std::env::set_var(ENV_REDACT_SECRETS, "off");
    std::env::set_var(ENV_WATCH_CHANNEL_CAPACITY, " 32 ");
    std::env::set_var(ENV_ALLOW_EMPTY_SNAPSHOT, "YES");
    let config = ConfigxConfig::from_env().unwrap();
    assert!(!config.redact_secrets);
    assert_eq!(config.watch_channel_capacity, 32);
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

    // 非法整数。
    std::env::set_var(ENV_WATCH_CHANNEL_CAPACITY, "-1");
    assert_eq!(
        ConfigxConfig::from_env().unwrap_err().kind(),
        ErrorKind::Invalid
    );

    // 合法整数但越界 → 校验失败。
    std::env::set_var(ENV_WATCH_CHANNEL_CAPACITY, "0");
    assert_eq!(
        ConfigxConfig::from_env().unwrap_err().kind(),
        ErrorKind::Invalid
    );

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
    std::env::remove_var(ENV_WATCH_CHANNEL_CAPACITY);
    std::env::remove_var(ENV_ALLOW_EMPTY_SNAPSHOT);
}
