#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable
)]
//! SDD 规格对照（特性 002）：把 `docs/标准.md` 的章节条款转成可执行断言。
//!
//! // SPEC-MAP: S-1 | 1. 定位 | assert_positioning
//! // SPEC-MAP: S-2 | 2. 数据与键治理 | assert_data_and_key_governance
//! // SPEC-MAP: S-3 | 3. 并发与订阅 | assert_concurrency_and_subscription
//! // SPEC-MAP: S-4 | 4. 配置治理 | assert_config_governance
//! // SPEC-MAP: S-5 | 5. 验收 | assert_acceptance

use std::sync::Arc;
use std::time::Duration;

use configx::{
    ConfigSource, ConfigWaitOutcome, ConfigxConfig, ConfigxStore, ErrorKind, MemorySource,
    ENV_ALLOW_EMPTY_SNAPSHOT, ENV_REDACT_SECRETS,
};

/// S-1：纯同步 crate——不引入异步运行时、不启动后台线程或文件 watcher；
/// 读取路径（`get` / `get_typed`）在同步上下文中即可完成。
#[test]
fn assert_positioning() {
    let mut store = ConfigxStore::new();
    store.register_source(MemorySource::from_pairs([("app.port", "5432")]));
    store.reload().unwrap();

    // 同步读取：没有 `.await`、没有运行时初始化。
    assert_eq!(store.get("app.port"), Some("5432"));
    assert_eq!(store.get_typed::<u16>("app.port").unwrap(), 5432);

    // `ConfigSource` 是同步 trait 且对象安全：可直接放进 `Arc<dyn>`。
    let source: Arc<dyn ConfigSource> = Arc::new(MemorySource::from_pairs([("k", "v")]));
    let mut via_dyn = ConfigxStore::new();
    via_dyn.register_shared_source(source);
    via_dyn.reload().unwrap();
    assert_eq!(via_dyn.get("k"), Some("v"));

    // 纯同步 crate 的主类型必须可跨线程共享（无隐藏的非 Send 状态）。
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ConfigxStore>();
    assert_send_sync::<ConfigxConfig>();
}

/// S-2：键按原样存储（大小写敏感）、只拒绝空键 / 控制字符 / 超 512 字节；
/// 快照为按键有序的 `Arc<BTreeMap>`；后注册源覆盖先注册源；失败不改状态。
#[test]
fn assert_data_and_key_governance() {
    // 键不做规范化：大小写敏感、按原样查询。
    let mut store = ConfigxStore::new();
    store.register_source(MemorySource::from_pairs([
        ("Key", "upper"),
        ("key", "lower"),
    ]));
    store.reload().unwrap();
    assert_eq!(store.get("Key"), Some("upper"));
    assert_eq!(store.get("key"), Some("lower"));

    // 拒绝空键与含控制字符的键，且失败不改状态。
    for bad in ["", "bad\nkey", "bad\u{1f}key"] {
        let mut candidate = ConfigxStore::new();
        candidate.register_source(MemorySource::from_pairs([("keep", "alive")]));
        candidate.reload().unwrap();
        candidate.register_source(MemorySource::from_pairs([(bad, "v")]));
        let error = candidate.reload().unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Invalid, "键 {bad:?} 必须被拒绝");
        assert_eq!(candidate.get("keep"), Some("alive"), "失败不改状态");
    }

    // 512 字节是闭区间上界。
    let mut boundary = ConfigxStore::new();
    boundary.register_source(MemorySource::from_pairs([("k".repeat(512), "ok")]));
    boundary.reload().expect("512 字节键合法");
    let mut too_long = ConfigxStore::new();
    too_long.register_source(MemorySource::from_pairs([("k".repeat(513), "no")]));
    assert_eq!(
        too_long.reload().unwrap_err().kind(),
        ErrorKind::Invalid,
        "513 字节键必须被拒绝"
    );

    // 快照按键升序，且是不可变的 `Arc` 视图。
    let mut ordered = ConfigxStore::new();
    ordered.register_source(MemorySource::from_pairs([
        ("c", "3"),
        ("a", "1"),
        ("b", "2"),
    ]));
    ordered.reload().unwrap();
    let snapshot = ordered.snapshot();
    assert_eq!(
        snapshot.keys().cloned().collect::<Vec<_>>(),
        vec!["a".to_string(), "b".to_string(), "c".to_string()]
    );
    ordered.register_source(MemorySource::from_pairs([("d", "4")]));
    ordered.reload().unwrap();
    assert_eq!(snapshot.len(), 3, "已取得的快照视图不受后续 reload 影响");

    // 层序：后注册者覆盖先注册者，先注册者的独有键保留。
    let mut layered = ConfigxStore::new();
    layered.register_source(MemorySource::from_pairs([("k", "low"), ("only_low", "1")]));
    layered.register_source(MemorySource::from_pairs([("k", "high")]));
    layered.reload().unwrap();
    assert_eq!(layered.get("k"), Some("high"));
    assert_eq!(layered.get("only_low"), Some("1"));

    // 脱敏是展示层职责：读取永远返回原始值。
    let mut secret = ConfigxStore::new();
    secret.register_source(MemorySource::from_pairs([("secret:token", "top-secret")]));
    secret.reload().unwrap();
    assert_eq!(secret.get("secret:token"), Some("top-secret"));
    assert!(
        !format!("{secret:?}").contains("top-secret"),
        "Debug 必须脱敏"
    );
}

/// S-3：单写多读（读取 `&self` 可并发）；变更通知基于 `Condvar` 的 generation
/// 计数器，单调递增且由订阅方判断是否需重读。
#[test]
fn assert_concurrency_and_subscription() {
    let mut store = ConfigxStore::new();
    let pairs: Vec<(String, String)> = (0..32)
        .map(|index| (format!("k{index}"), format!("v{index}")))
        .collect();
    store.register_source(MemorySource::from_pairs(pairs));
    store.reload().unwrap();

    // 读取并发安全：8 个线程同时 `get` 同一份 `Arc<ConfigxStore>`。
    let shared = Arc::new(store);
    let readers: Vec<_> = (0..8)
        .map(|_| {
            let shared = Arc::clone(&shared);
            std::thread::spawn(move || {
                for index in 0..32 {
                    let key = format!("k{index}");
                    let expected = format!("v{index}");
                    assert_eq!(shared.get(&key), Some(expected.as_str()));
                }
            })
        })
        .collect();
    for reader in readers {
        reader.join().unwrap();
    }

    // generation 单调递增：订阅句柄以变更序号判断是否需重读。
    let mut writer = ConfigxStore::new();
    writer.register_source(MemorySource::from_pairs([("k", "v")]));
    let mut subscription = writer.subscribe();
    assert_eq!(subscription.seen(), 0);
    assert_eq!(writer.generation(), 0);
    writer.reload().unwrap();
    assert_eq!(writer.generation(), 1);
    assert_eq!(
        subscription.wait_outcome().unwrap(),
        ConfigWaitOutcome::Changed(configx::ConfigChange { generation: 1 })
    );
    assert_eq!(subscription.seen(), 1);

    // 无变更时按调用方给定的 deadline 超时返回，不做无界阻塞。
    assert_eq!(
        subscription
            .wait_timeout_outcome(Duration::from_millis(10))
            .unwrap(),
        ConfigWaitOutcome::TimedOut
    );
}

/// S-4：`ConfigxConfig` 范式（结构体 + builder/from_env/from_toml/validate，fail-fast）；
/// 环境变量开关以常量登记；秘密值不进入配置仓库明文。
#[test]
fn assert_config_governance() {
    assert_eq!(ENV_REDACT_SECRETS, "FOUNDATIONX_CONFIGX_REDACT_SECRETS");
    assert_eq!(
        ENV_ALLOW_EMPTY_SNAPSHOT,
        "FOUNDATIONX_CONFIGX_ALLOW_EMPTY_SNAPSHOT"
    );

    let config = ConfigxConfig::builder()
        .redact_secrets(true)
        .allow_empty_snapshot(false)
        .build()
        .expect("build 必须校验");
    config.validate().expect("validate fail-fast 通过");

    // 空快照策略生效：合并结果为空时 reload 返回 Conflict，快照保持原样。
    let mut store = ConfigxStore::from_config(config).unwrap();
    store.register_source(MemorySource::new());
    assert_eq!(store.reload().unwrap_err().kind(), ErrorKind::Conflict);
    assert!(store.is_empty());
    assert_eq!(store.generation(), 0);
    // 有内容时同一策略不再触发。
    store.register_source(MemorySource::from_pairs([("a", "1")]));
    store.reload().unwrap();
    assert_eq!(store.get("a"), Some("1"));

    // 秘密只经环境变量或内存源注入；配置面本身不含凭据字段。
    let debug = format!("{:?}", ConfigxConfig::default());
    assert!(
        !debug.contains("access_key") && !debug.contains("password") && !debug.contains("token"),
        "配置 Debug 不含凭据字段：{debug}"
    );
    assert!(debug.contains("redact_secrets"));
    assert!(debug.contains("allow_empty_snapshot"));
}

/// S-5：验收面——标准文档登记的 fmt / test / clippy 三条命令可复现，
/// 且本文件本身即是「一次执行跑完 SDD 断言」的载体。
#[test]
fn assert_acceptance() {
    let standard = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/标准.md"))
        .expect("docs/标准.md 必须存在（D3）");
    for command in [
        "cargo fmt --all --check",
        "cargo test --all-targets",
        "cargo clippy --all-targets -- -D warnings",
    ] {
        assert!(
            standard.contains(command),
            "验收命令 {command:?} 必须登记在 docs/标准.md"
        );
    }
    // 测试面覆盖清单同样登记在文档中，避免文档与测试脱节。
    assert!(standard.contains("tests/public_api.rs"));
    let version = package_version();
    assert!(
        standard.contains(&format!("v{version}")),
        "docs/标准.md 必须声明与 Cargo.toml [package].version 一致的 v{{version}}"
    );
    let api = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/API.md"))
        .expect("docs/API.md 必须存在");
    assert!(
        api.contains(&format!("configx {version}")),
        "docs/API.md 必须声明与 Cargo.toml [package].version 一致的 configx {{version}}"
    );
    let _ = std::env::current_dir().expect("可取得当前目录（验收命令可执行）");
}

fn package_version() -> String {
    let cargo = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .expect("Cargo.toml 必须存在");
    let mut in_package = false;
    for line in cargo.lines() {
        let trimmed = line.trim();
        if trimmed == "[package]" {
            in_package = true;
            continue;
        }
        if trimmed.starts_with('[') {
            in_package = false;
            continue;
        }
        if in_package {
            if let Some(rest) = trimmed.strip_prefix("version") {
                let rest = rest.trim_start();
                if let Some(rest) = rest.strip_prefix('=') {
                    let value = rest.trim().trim_matches('"');
                    assert!(!value.is_empty(), "Cargo.toml [package].version 不得为空");
                    return value.to_string();
                }
            }
        }
    }
    panic!("Cargo.toml 缺少 [package].version");
}
