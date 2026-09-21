# configx

分层配置存储（layered configuration store）：把内存、环境变量、`KEY=VALUE` 文件等配置源
按注册顺序合并成一份不可变快照，并提供读取、类型化读取、变更订阅、快照差异与密钥脱敏。

- **纯同步**：变更通知基于 `Condvar`，不引入异步运行时，也不启动自动文件 watcher；
- **零内部耦合**：仅依赖 `serde` / `serde_json` / `toml` / `thiserror`；
- **单写多读**：读取用 `&self` 可并发，替换快照用 `&mut self`；
- **失败不改状态**：源加载或校验失败时，旧快照与变更序号保持原样；
- **读取永不脱敏**：脱敏只作用于日志与 `Debug` 展示路径。

## 安装

```bash
cargo add configx
```

## 最小可运行示例

```rust
use configx::{ConfigxStore, EnvSource, MemorySource};

fn main() -> Result<(), configx::ConfigxError> {
    let mut store = ConfigxStore::new();

    // 低优先级：内置默认值；高优先级：环境变量（后注册者覆盖先注册者）
    store.register_source(MemorySource::from_pairs([
        ("app.host", "db.local"),
        ("app.port", "5432"),
    ]));
    store.register_source(EnvSource::new("APP_"));
    store.reload()?;

    println!("host = {:?}", store.get("app.host"));
    println!("port = {}", store.get_typed::<u16>("app.port")?);

    // 订阅变更，并在 reload 后观察新的 generation
    let mut subscription = store.watch();
    store.reload()?;
    let change = subscription.wait()?;
    println!("observed change: {change:?}");

    Ok(())
}
```

## 配置项

`ConfigxConfig` 支持 `from_env()`（前缀 `FOUNDATIONX_CONFIGX_`）、`from_toml(&str)`
与链式构建器 `ConfigxConfig::builder()`；所有字段都有默认值。

| 字段 / 环境变量 | 类型 | 默认值 | 说明 |
| --------------- | ---- | ------ | ---- |
| `redact_secrets`<br>`FOUNDATIONX_CONFIGX_REDACT_SECRETS` | `bool` | `true` | 是否在 `Debug`/日志路径上把 `secret:` 前缀键的值显示为 `***`；不影响 `get` 的返回值 |
| `watch_channel_capacity`<br>`FOUNDATIONX_CONFIGX_WATCH_CHANNEL_CAPACITY` | `usize` | `64` | 变更通道容量，合法区间 `1..=65536`，超出即校验失败 |
| `allow_empty_snapshot`<br>`FOUNDATIONX_CONFIGX_ALLOW_EMPTY_SNAPSHOT` | `bool` | `true` | 为 `false` 时，合并结果为空的 `reload` 返回 `ConfigxError::Conflict` 并保留旧快照 |

布尔值接受 `1/0`、`true/false`、`yes/no`、`on/off`（忽略大小写）；环境变量只含空白时视为未设置。

```rust
use configx::ConfigxConfig;

let config = ConfigxConfig::builder()
    .redact_secrets(true)
    .watch_channel_capacity(16)
    .allow_empty_snapshot(false)
    .build()?;

let from_toml = ConfigxConfig::from_toml(
    r#"
    watch_channel_capacity = 16
    allow_empty_snapshot = false
    "#,
)?;
assert_eq!(config, from_toml);
# Ok::<(), configx::ConfigxError>(())
```

## 关键语义

- **层序**：源按注册顺序合并，后注册者优先级更高；先注册源独有的键保留。
- **原子替换**：`reload()` 先完整加载并校验全部源，再整体替换快照，读取方只会看到旧快照或新快照。
- **键不规范化**：键大小写敏感、按原样存储与查询；只拒绝空键、控制字符与超过 512 字节的键。
- **配置源**：`MemorySource`（内存）、`EnvSource`（按前缀剥离环境变量）、`FileSource`（`KEY=VALUE` 文件），
  也可自行实现 `ConfigSource`。
- **健康检查**：`health_check()` 返回 `ConfigxHealth { sources, keys, healthy }`；
  `ping()` 在「至少一个源已成功加载」时返回 `Ok(())`，否则返回可重试的 `ConfigxError::Unavailable`。
- **变更订阅**：`ConfigWatch` / `ConfigSubscription` 提供阻塞等待、限时等待与关闭语义，
  返回 `ConfigWaitOutcome::{Changed, TimedOut, Closed}`。
- **快照视图**：`snapshot()` 返回不可变 `Arc<BTreeMap<String, String>>`，配合
  `subset_snapshot` / `try_subset_snapshot` / `snapshots_agree` / `diff_snapshots` 使用。
- **密钥脱敏**：`is_secret_key` / `redact_value` / `redact_map` 为纯函数，`ConfigxStore::get`
  永远返回原始值。

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
