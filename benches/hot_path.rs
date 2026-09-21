#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable
)]
//! configx 热路径基准测试。
//!
//! 覆盖「注册源 → reload → 快照读取 → 类型化读取 → 差异比较 → 配置校验」的同步热路径，
//! 全程离线、无文件与网络 I/O。运行：`cargo bench`（加 `-- --quick` 缩减迭代数）。
use std::hint::black_box;
use std::time::Instant;

use configx::{diff_snapshots, ConfigxConfig, ConfigxStore, MemorySource};

fn iters() -> u32 {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--test") {
        // cargo test --all-targets 会以测试模式运行本二进制；只做冒烟。
        10
    } else if args.iter().any(|a| a == "--quick") {
        1_000
    } else {
        50_000
    }
}

/// 构建带内存源的存储并完成首次 reload。
fn build_store() -> ConfigxStore {
    let mut store = ConfigxStore::new();
    store.register_source(MemorySource::from_pairs([
        ("app.host", "db.local"),
        ("app.port", "5432"),
        ("app.name", "configx-bench"),
        ("db.password", "s3cret"),
    ]));
    store.reload().expect("reload 应成功");
    store
}

fn main() {
    let n = iters();

    // 预热：触发分配器与分支预测稳定
    for _ in 0..3 {
        let store = build_store();
        black_box(store.snapshot());
    }

    let config = ConfigxConfig::builder().build().expect("build 应成功");
    let mut store = build_store();
    let baseline = store.snapshot();

    let start = Instant::now();
    for i in 0..n {
        // 配置校验
        config.validate().expect("validate 应成功");
        // 重新合并全部源并生成新快照
        store.reload().expect("reload 应成功");
        let snap = store.snapshot();
        // 原始读取 + 类型化读取
        black_box(snap.get("app.host"));
        black_box(
            store
                .get_typed::<u16>("app.port")
                .expect("类型化读取应成功"),
        );
        // 差异比较
        black_box(diff_snapshots(&baseline, &snap));
        black_box(i);
    }
    let elapsed = start.elapsed();
    println!(
        "bench_configx_hot_path: iters={n} total={elapsed:?} per_iter={:?}",
        elapsed / n
    );
}
