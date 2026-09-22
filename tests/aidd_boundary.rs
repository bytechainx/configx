#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable
)]
//! AIDD 对抗 / 边界用例（特性 002）。
//!
//! 候选由 AI 生成，逐条人工复核后仅保留「结论=保留」项；丢弃项登记于 PR 描述。
//!
//! // AIDD: 512/513 字节键边界 | 来源=AI | 复核=ZoneCNH/2026-09-22 | 依据=标准.md §2 键长度上界 | 结论=保留
//! // AIDD: 键含控制字符与换行 | 来源=AI | 复核=ZoneCNH/2026-09-22 | 依据=标准.md §2 控制字符拒绝 | 结论=保留
//! // AIDD: Unicode 与全角点键 | 来源=AI | 复核=ZoneCNH/2026-09-22 | 依据=标准.md §2 不做键规范化 | 结论=保留
//! // AIDD: 无变更时的限时等待 | 来源=AI | 复核=ZoneCNH/2026-09-22 | 依据=标准.md §3 不无界阻塞 | 结论=保留
//! // AIDD: 关闭监听后的 reload | 来源=AI | 复核=ZoneCNH/2026-09-22 | 依据=标准.md §2 失败不改状态 | 结论=保留
//! // AIDD: 错误消息不回显配置值 | 来源=AI | 复核=ZoneCNH/2026-09-22 | 依据=标准.md §4 秘密不入日志 | 结论=保留
//! // AIDD: 源暂时返回空内容 | 来源=AI | 复核=ZoneCNH/2026-09-22 | 依据=标准.md §4 空快照策略 | 结论=保留
//! // AIDD: 8 线程并发读取 | 来源=AI | 复核=ZoneCNH/2026-09-22 | 依据=标准.md §3 单写多读 | 结论=保留

use std::sync::Arc;
use std::time::Duration;

use configx::{
    parse_key_value_file, ConfigWaitOutcome, ConfigxConfig, ConfigxStore, ErrorKind, MemorySource,
};

fn store_with(pairs: impl IntoIterator<Item = (String, String)>) -> ConfigxStore {
    let mut store = ConfigxStore::new();
    store.register_source(MemorySource::from_pairs(pairs));
    store
}

/// 边界：512 字节是闭区间上界，513 字节必须被拒绝且失败不改状态。
#[test]
fn key_length_boundary() {
    let ok_key = "k".repeat(512);
    let mut ok = store_with([(ok_key.clone(), "v".to_string())]);
    ok.reload().expect("512 字节键合法");
    assert_eq!(ok.get(&ok_key), Some("v"));

    let mut too_long = ConfigxStore::new();
    too_long.register_source(MemorySource::from_pairs([(
        "keep".to_string(),
        "1".to_string(),
    )]));
    too_long.reload().unwrap();
    too_long.register_source(MemorySource::from_pairs([(
        "k".repeat(513),
        "v".to_string(),
    )]));
    assert_eq!(too_long.reload().unwrap_err().kind(), ErrorKind::Invalid);
    assert_eq!(too_long.get("keep"), Some("1"), "失败不改状态");
}

/// 边界：键含控制字符（含换行、NUL、ESC）——必须拒绝，防止日志注入与歧义。
#[test]
fn key_with_control_characters() {
    for bad in [
        "line\nbreak",
        "carriage\rreturn",
        "nul\u{0}byte",
        "ansi\u{1b}[31m",
        "tab\tkey",
    ] {
        let mut store = ConfigxStore::new();
        store.register_source(MemorySource::from_pairs([(
            bad.to_string(),
            "v".to_string(),
        )]));
        let error = store.reload().unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Invalid, "键 {bad:?} 必须被拒绝");
        assert!(store.is_empty(), "被拒绝的键不得进入快照");
    }
}

/// 边界：Unicode / 组合字符 / 全角点键——键不做规范化，照原样存取且不 panic。
#[test]
fn unicode_keys_are_stored_verbatim() {
    let mut store = ConfigxStore::new();
    store.register_source(MemorySource::from_pairs([
        ("数据库．查询".to_string(), "1".to_string()),
        ("cafe\u{0301}".to_string(), "2".to_string()),
        ("大小写敏感".to_string(), "3".to_string()),
    ]));
    store.reload().expect("非 ASCII 键合法");
    assert_eq!(store.get("数据库．查询"), Some("1"));
    assert_eq!(store.get("cafe\u{0301}"), Some("2"));
    assert_eq!(store.get("大小写敏感"), Some("3"));
    assert_eq!(store.len(), 3);
}

/// 对抗：无变更时的限时等待必须按 deadline 返回，不得死等（Condvar 不被误用为无界阻塞）。
#[test]
fn timed_wait_without_change_returns() {
    let mut store = ConfigxStore::new();
    store.register_source(MemorySource::from_pairs([("k", "v")]));
    let mut subscription = store.subscribe();

    let started = std::time::Instant::now();
    let outcome = subscription
        .wait_timeout_outcome(Duration::from_millis(30))
        .unwrap();
    assert_eq!(outcome, ConfigWaitOutcome::TimedOut);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "限时等待必须受 deadline 约束"
    );
    // 兼容接口在超时时折叠为 None。
    assert!(subscription
        .wait_timeout(Duration::from_millis(5))
        .unwrap()
        .is_none());
}

/// 对抗：监听被关闭后 reload 必须失败，且旧快照与 generation 完整保留。
#[test]
fn reload_after_watch_closed_keeps_snapshot() {
    let mut store = ConfigxStore::new();
    store.register_source(MemorySource::from_pairs([("keep", "alive")]));
    store.reload().unwrap();
    let before = store.snapshot();

    store.notifier().close().unwrap();
    let error = store.reload().unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Conflict);
    assert_eq!(store.snapshot(), before);
    assert_eq!(store.generation(), 1);
    assert_eq!(store.get("keep"), Some("alive"));
}

/// 对抗：所有失败路径的错误消息都不得回显配置值 / 敏感值。
#[test]
fn error_messages_never_echo_values() {
    let mut store = ConfigxStore::new();
    store.register_source(MemorySource::from_pairs([
        ("secret:token".to_string(), "top-secret-value".to_string()),
        ("app.host".to_string(), "db.internal".to_string()),
    ]));
    store.reload().unwrap();

    let mismatch = store.get_typed::<u16>("secret:token").unwrap_err();
    assert_eq!(mismatch.kind(), ErrorKind::TypeMismatch);
    assert!(!mismatch.to_string().contains("top-secret-value"));
    assert!(!mismatch.to_string().contains("db.internal"));

    let parse = parse_key_value_file("secret-looking-value").unwrap_err();
    assert_eq!(parse.kind(), ErrorKind::Invalid);
    assert!(
        !parse.to_string().contains("secret-looking-value"),
        "解析错误只报行号：{}",
        parse
    );

    // TOML 路径同样不得回显值。此前这里只断言 kind()，把「不回显」本身漏掉了——
    // 该路径恰恰会把非法值内联进 serde 错误文本（覆盖缺口，见本文件头 AIDD 表最后一行）。
    let toml = ConfigxConfig::from_toml("redact_secrets = \"top-secret-value\"").unwrap_err();
    assert_eq!(toml.kind(), ErrorKind::Parse);
    let rendered = toml.to_string();
    assert!(
        !rendered.contains("top-secret-value"),
        "TOML 解析失败不得回显配置值：{rendered}"
    );
    assert!(
        !rendered.contains("redact_secrets ="),
        "TOML 解析失败不得回显源码行：{rendered}"
    );

    // Debug 展示路径同样脱敏。
    assert!(!format!("{store:?}").contains("top-secret-value"));
}

/// 对抗：源「暂时返回空内容」不得把线上配置清空（`allow_empty_snapshot=false` 的初衷）。
#[test]
fn empty_source_cannot_wipe_snapshot() {
    let policy = ConfigxConfig::builder()
        .allow_empty_snapshot(false)
        .build()
        .unwrap();
    let mut store = ConfigxStore::from_config(policy).unwrap();
    store.register_source(MemorySource::from_pairs([("a", "1"), ("b", "2")]));
    store.reload().unwrap();
    assert_eq!(store.len(), 2);

    // 追加一个临时为空的源：合并结果仍非空，reload 正常。
    store.register_source(MemorySource::new());
    store.reload().unwrap();
    assert_eq!(store.len(), 2);

    // 整体替换为空：被策略拒绝，旧快照保留。
    let before = store.snapshot();
    let mut empty = ConfigxStore::from_config(
        ConfigxConfig::builder()
            .allow_empty_snapshot(false)
            .build()
            .unwrap(),
    )
    .unwrap();
    empty.register_source(MemorySource::new());
    assert_eq!(empty.reload().unwrap_err().kind(), ErrorKind::Conflict);
    assert!(empty.is_empty());
    assert_eq!(store.snapshot(), before);
}

/// 对抗：8 个线程并发读同一份快照——不得出现数据竞争或读到半提交状态。
#[test]
fn concurrent_readers_see_consistent_snapshot() {
    let mut store = ConfigxStore::new();
    let pairs: Vec<(String, String)> = (0..64)
        .map(|index| (format!("k{index}"), format!("v{index}")))
        .collect();
    store.register_source(MemorySource::from_pairs(pairs));
    store.reload().unwrap();

    let shared = Arc::new(store);
    let readers: Vec<_> = (0..8)
        .map(|_| {
            let shared = Arc::clone(&shared);
            std::thread::spawn(move || {
                for index in 0..64 {
                    let key = format!("k{index}");
                    let expected = format!("v{index}");
                    assert_eq!(shared.get(&key), Some(expected.as_str()));
                }
                assert_eq!(shared.len(), 64);
                assert!(shared.ping().is_ok());
            })
        })
        .collect();
    for reader in readers {
        reader.join().expect("读线程不得 panic");
    }
}
