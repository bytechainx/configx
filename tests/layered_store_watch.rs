#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable
)]
//! 分层合并、原子重载、配置源解析与 watch 行为。

use std::sync::Arc;
use std::time::Duration;

use configx::{
    diff_snapshots, parse_key_value_file, snapshots_agree, subset_snapshot, try_subset_snapshot,
    ConfigChange, ConfigSource, ConfigWaitOutcome, ConfigxConfig, ConfigxStore, EnvSource,
    ErrorKind, FileSource, GlobalFileSource, LayeredConfig, MemorySource,
};

#[test]
fn layered_merge_later_source_wins() {
    let low = Arc::new(MemorySource::from_pairs([("k", "low"), ("only_low", "1")]));
    let high = Arc::new(MemorySource::from_pairs([("k", "high")]));
    let layered = LayeredConfig::new().with_source(low).with_source(high);
    assert_eq!(layered.len(), 2);
    assert!(!layered.is_empty());
    let merged = layered.load_merged().unwrap();
    assert_eq!(merged.get("k").map(String::as_str), Some("high"));
    assert_eq!(merged.get("only_low").map(String::as_str), Some("1"));

    // store 门面上的同一语义：后注册的源优先级更高。
    let mut store = ConfigxStore::new();
    store.register_source(MemorySource::from_pairs([("k", "low"), ("only_low", "1")]));
    store.register_source(MemorySource::from_pairs([("k", "high")]));
    store.reload().unwrap();
    assert_eq!(store.get("k"), Some("high"));
    assert_eq!(store.get("only_low"), Some("1"));
    assert_eq!(store.len(), 2);
    assert_eq!(store.source_count(), 2);

    let empty = LayeredConfig::default();
    assert!(empty.is_empty());
    assert!(empty.load_merged().unwrap().is_empty());
}

#[test]
fn reload_rereads_file_source_and_replaces_snapshot() {
    let dir = std::env::temp_dir().join(format!("configx-reload-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("app.conf");
    std::fs::write(&path, "A=1\n").unwrap();

    let mut store = ConfigxStore::new();
    store.register_source(FileSource::new(&path));
    store.reload().unwrap();
    assert_eq!(store.get("A"), Some("1"));

    // 源每次 load 都重读文件；整体替换意味着旧键消失。
    std::fs::write(&path, "B=2\n").unwrap();
    store.reload().unwrap();
    assert_eq!(store.get("B"), Some("2"));
    assert_eq!(store.get("A"), None);
    assert_eq!(store.len(), 1);
    assert_eq!(store.generation(), 2);
    assert_eq!(store.health_check().unwrap().keys, 1);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn failed_reload_keeps_previous_snapshot_and_generation() {
    let mut store = ConfigxStore::new();
    store.register_source(MemorySource::from_pairs([("keep", "alive")]));
    store.reload().unwrap();

    store.register_source(FileSource::new("/no/such/configx-missing.conf"));
    let error = store.reload().unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Invalid);
    assert!(error
        .to_string()
        .contains("读取配置文件失败：路径=/no/such/configx-missing.conf"));
    assert_eq!(store.get("keep"), Some("alive"));
    assert_eq!(store.generation(), 1, "失败的 reload 不得推进 generation");
    assert!(store.ping().is_ok());
}

#[test]
fn invalid_key_is_rejected_without_touching_snapshot() {
    let mut store = ConfigxStore::new();
    store.register_source(MemorySource::from_pairs([("keep", "alive")]));
    store.reload().unwrap();

    store.register_source(MemorySource::from_pairs([("bad\nkey", "value")]));
    let error = store.reload().unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Invalid);
    assert!(error.to_string().contains("配置键不能包含控制字符"));
    assert_eq!(store.get("keep"), Some("alive"));
    assert_eq!(store.generation(), 1);
}

#[test]
fn empty_snapshot_can_be_refused_by_config() {
    let config = ConfigxConfig::builder()
        .allow_empty_snapshot(false)
        .build()
        .unwrap();
    let mut store = ConfigxStore::from_config(config).unwrap();
    store.register_source(MemorySource::new());

    let error = store.reload().unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Conflict);
    assert!(store.is_empty());
    assert!(!store.health_check().unwrap().healthy);
    assert_eq!(store.generation(), 0);

    // 有内容时同一策略不再触发。
    store.register_source(MemorySource::from_pairs([("a", "1")]));
    store.reload().unwrap();
    assert_eq!(store.get("a"), Some("1"));
    assert_eq!(store.generation(), 1);
}

#[test]
fn env_and_key_value_sources_keep_documented_semantics() {
    let env = EnvSource::new("APP_");
    assert_eq!(env.prefix(), "APP_");
    let map = env
        .load_from_iter([
            ("APP_HOST", "h"),
            ("APP_PORT", "80"),
            ("OTHER", "x"),
            ("APP_", "skip"),
        ])
        .unwrap();
    assert_eq!(map.get("HOST").map(String::as_str), Some("h"));
    assert_eq!(map.get("PORT").map(String::as_str), Some("80"));
    assert!(!map.contains_key("OTHER"));
    assert!(!map.contains_key(""));
    assert!(EnvSource::new("").load().unwrap().is_empty());

    let parsed = parse_key_value_file("# 注释\nHOST=h\nPORT = \"8080\"\nEMPTY=\n").unwrap();
    assert_eq!(parsed.get("HOST").map(String::as_str), Some("h"));
    assert_eq!(parsed.get("PORT").map(String::as_str), Some("8080"));
    assert_eq!(parsed.get("EMPTY").map(String::as_str), Some(""));

    let missing_separator = parse_key_value_file("secret-looking-value").unwrap_err();
    assert_eq!(missing_separator.kind(), ErrorKind::Invalid);
    assert!(missing_separator
        .to_string()
        .contains("第 1 行：应为 KEY=VALUE"));
    assert!(
        !missing_separator
            .to_string()
            .contains("secret-looking-value"),
        "不得回显原始行"
    );
    assert!(parse_key_value_file("=v")
        .unwrap_err()
        .to_string()
        .contains("键为空"));

    // 前缀为空的源合法但永远加载出空映射；空映射仍算「成功加载」。
    let mut store = ConfigxStore::new();
    store.register_source(EnvSource::new("UNLIKELY_PREFIX_CONFIGX_XYZ_"));
    store.reload().unwrap();
    assert!(store.is_empty());
    store.ping().expect("空映射也是成功加载");
}

#[test]
fn watch_reports_changed_timeout_and_closed() {
    let mut store = ConfigxStore::new();
    store.register_source(MemorySource::from_pairs([("k", "v")]));
    let mut subscription = store.subscribe();
    assert_eq!(subscription.seen(), 0);

    // 未发生变更时，限时等待必须超时而不是阻塞。
    assert_eq!(
        subscription
            .wait_timeout_outcome(Duration::from_millis(10))
            .unwrap(),
        ConfigWaitOutcome::TimedOut
    );
    assert!(subscription
        .wait_timeout(Duration::from_millis(1))
        .unwrap()
        .is_none());

    store.reload().unwrap();
    assert_eq!(store.generation(), 1);
    assert_eq!(
        subscription.wait_outcome().unwrap(),
        ConfigWaitOutcome::Changed(ConfigChange { generation: 1 })
    );
    assert_eq!(subscription.seen(), 1);

    // 兼容接口在关闭时折叠为 None，显式接口区分 Closed。
    store.notifier().close().unwrap();
    assert_eq!(
        subscription.wait_outcome().unwrap(),
        ConfigWaitOutcome::Closed
    );
    assert!(subscription.wait().unwrap().is_none());

    // 已关闭的监听会让后续 reload 报错，且不改变快照。
    let before = store.snapshot();
    let error = store.reload().unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Conflict);
    assert_eq!(store.snapshot(), before);
}

#[test]
fn snapshots_are_stable_and_views_agree() {
    let mut store = ConfigxStore::new();
    store.register_source(MemorySource::from_pairs([
        ("a", "1"),
        ("b", "2"),
        ("c", "3"),
    ]));
    store.reload().unwrap();

    let before = store.snapshot();
    assert_eq!(before.len(), 3);

    let subset = subset_snapshot(&store, &["a", "c", "missing"]);
    assert_eq!(subset.len(), 2);
    assert_eq!(subset.get("a").map(String::as_str), Some("1"));
    assert!(snapshots_agree(&before, &subset, &["a", "c"]));
    assert!(!snapshots_agree(&before, &subset, &["b"]));
    assert!(snapshots_agree(&before, &subset, &[]));
    assert_eq!(
        try_subset_snapshot(&store, &["b"])
            .unwrap()
            .get("b")
            .map(String::as_str),
        Some("2")
    );
    assert_eq!(
        try_subset_snapshot(&store, &[""]).unwrap_err().kind(),
        ErrorKind::Invalid
    );
    assert!(
        subset_snapshot(&store, &[""]).is_empty(),
        "非法键折叠为空快照"
    );

    // 追加更高优先级的层后再 reload：新值生效，旧 Arc 视图保持不变。
    store.register_source(MemorySource::from_pairs([("a", "9")]));
    store.reload().unwrap();
    assert_eq!(store.get("a"), Some("9"));
    assert_eq!(store.get("b"), Some("2"), "低优先级层的独有键保留");
    assert_eq!(before.get("a").map(String::as_str), Some("1"));
    assert_eq!(before.len(), 3);

    let mut other = ConfigxStore::new();
    other.register_source(MemorySource::from_pairs([("a", "9"), ("d", "4")]));
    other.reload().unwrap();
    let after = other.snapshot();

    let diff = diff_snapshots(after.as_ref(), before.as_ref());
    assert!(!diff.is_empty());
    assert_eq!(diff.only_left, vec!["d".to_string()]);
    assert_eq!(diff.only_right, vec!["b".to_string(), "c".to_string()]);
    assert_eq!(diff.changed, vec!["a".to_string()]);
    assert_eq!(diff.total_changes(), 4);
}

#[test]
fn concurrent_reads_from_shared_store_are_safe() {
    let mut store = ConfigxStore::new();
    let pairs: Vec<(String, String)> = (0..64)
        .map(|index| (format!("k{index}"), format!("v{index}")))
        .collect();
    store.register_source(MemorySource::from_pairs(pairs));
    store.reload().unwrap();

    let store = Arc::new(store);
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let shared = Arc::clone(&store);
            std::thread::spawn(move || {
                for index in 0..64 {
                    let key = format!("k{index}");
                    let expected = format!("v{index}");
                    assert_eq!(shared.get(&key), Some(expected.as_str()));
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
}

#[test]
fn global_file_then_memory_overrides() {
    let path = std::env::temp_dir().join(format!(
        "configx-layered-global-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::write(&path, "k=file\nonly_file=1\n").unwrap();
    let mut store = ConfigxStore::new();
    store.register_source(GlobalFileSource::new(&path));
    store.register_source(MemorySource::from_pairs([("k", "mem")]));
    store.reload().unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(store.get("k"), Some("mem"));
    assert_eq!(store.get("only_file"), Some("1"));
}
