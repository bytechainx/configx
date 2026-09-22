//! 配置变更通知：进程内的同步广播总线。
//!
//! 本模块**不启动任何自动 watcher**、不读文件、不依赖异步运行时。
//! 变更只能由调用方显式触发（[`ConfigxStore::reload`](crate::ConfigxStore::reload)
//! 或 [`ConfigWatch::notify`]），订阅端阻塞等待 generation 增长。

use std::sync::{Arc, Condvar, Mutex, MutexGuard, TryLockError};
use std::thread;
use std::time::{Duration, Instant};

use crate::error::{ConfigxError, ConfigxResult};

/// 限时等待的轮询间隔上限。
const TIMED_WAIT_POLL_INTERVAL: Duration = Duration::from_millis(1);
/// 变更锁中毒上下文。
const WATCH_MUTATION_LOCK_POISONED_CONTEXT: &str = "配置监听变更锁已中毒";
/// 状态锁中毒上下文。
const WATCH_STATE_LOCK_POISONED_CONTEXT: &str = "配置监听状态锁已中毒";
/// 监听已关闭上下文。
const WATCH_CLOSED_CONTEXT: &str = "配置监听已关闭";
/// generation 溢出上下文。
const WATCH_GENERATION_OVERFLOW_CONTEXT: &str = "配置监听 generation 溢出";

/// 一次变更通知；`generation` 单调递增且从 1 起。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigChange {
    /// 从 1 起的变更序号。
    pub generation: u64,
}

/// 一次订阅等待的显式结果。
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigWaitOutcome {
    /// 观察到新的 generation。
    Changed(ConfigChange),
    /// 总 deadline 已到，且未观察到新的 generation。
    TimedOut,
    /// 监听已关闭。
    Closed,
}

impl ConfigWaitOutcome {
    /// 兼容 API 折叠：只有 [`Changed`](Self::Changed) 有值。
    fn into_change(self) -> Option<ConfigChange> {
        match self {
            Self::Changed(change) => Some(change),
            Self::TimedOut | Self::Closed => None,
        }
    }
}

/// 监听状态：generation 与关闭标记。
#[derive(Default)]
struct WatchState {
    generation: u64,
    closed: bool,
}

/// 进程内配置变更总线。
///
/// - [`subscribe`](Self::subscribe) 返回订阅句柄；
/// - [`notify`](Self::notify) 广播一次变更（generation + 1）；
/// - [`close`](Self::close) 之后所有等待立即返回 [`ConfigWaitOutcome::Closed`]。
///
/// 变更由独立的 mutation 锁串行化；状态锁只覆盖短暂的检查与提交，
/// 不跨任何外部等待——这保证 [`ConfigSubscription::wait_timeout_outcome`] 的
/// 返回时刻由调用方 deadline 决定，而不是由锁竞争决定。
#[derive(Default)]
pub struct ConfigWatch {
    mutation: Mutex<()>,
    state: Mutex<WatchState>,
    cvar: Condvar,
}

impl ConfigWatch {
    /// 构造监听总线。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 当前 generation；未通知过或锁中毒时为 0。
    #[must_use]
    pub fn generation(&self) -> u64 {
        let Ok(_mutation) = self.mutation.lock() else {
            return 0;
        };
        self.state.lock().map(|state| state.generation).unwrap_or(0)
    }

    /// 订阅；返回从当前 generation 之后开始等待的句柄。
    #[must_use]
    pub fn subscribe(self: &Arc<Self>) -> ConfigSubscription {
        let seen = self.generation();
        ConfigSubscription {
            watch: Arc::clone(self),
            seen,
        }
    }

    /// 广播一次变更。
    ///
    /// # Errors
    ///
    /// mutation/state 锁中毒、监听已关闭或 generation 溢出时返回错误。
    pub fn notify(&self) -> ConfigxResult<ConfigChange> {
        let _mutation = self.lock_mutation()?;
        let mut state = self.lock_state()?;
        let generation = next_generation(&state)?;
        state.generation = generation;
        let change = ConfigChange { generation };
        self.cvar.notify_all();
        Ok(change)
    }

    /// 关闭监听；后续等待与通知都会失败或返回关闭结果。
    ///
    /// # Errors
    ///
    /// mutation/state 锁中毒时返回错误。
    pub fn close(&self) -> ConfigxResult<()> {
        let _mutation = self.lock_mutation()?;
        let mut state = self.lock_state()?;
        state.closed = true;
        self.cvar.notify_all();
        Ok(())
    }

    fn lock_mutation(&self) -> ConfigxResult<MutexGuard<'_, ()>> {
        self.mutation
            .lock()
            .map_err(|_| ConfigxError::conflict(WATCH_MUTATION_LOCK_POISONED_CONTEXT))
    }

    fn lock_state(&self) -> ConfigxResult<MutexGuard<'_, WatchState>> {
        self.state
            .lock()
            .map_err(|_| ConfigxError::conflict(WATCH_STATE_LOCK_POISONED_CONTEXT))
    }
}

impl std::fmt::Debug for ConfigWatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfigWatch")
            .field("generation", &self.generation())
            .finish()
    }
}

/// 计算下一个 generation；监听关闭或溢出时报错。
fn next_generation(state: &WatchState) -> ConfigxResult<u64> {
    if state.closed {
        return Err(ConfigxError::conflict(WATCH_CLOSED_CONTEXT));
    }
    state
        .generation
        .checked_add(1)
        .ok_or_else(|| ConfigxError::conflict(WATCH_GENERATION_OVERFLOW_CONTEXT))
}

/// 订阅句柄：记录已观察到的 generation，并可阻塞等待更新。
///
/// # 阻塞调用
///
/// 本类型的所有等待方法均为**同步阻塞**实现（基于 `std::sync::Condvar` 与
/// `std::thread::sleep`），不依赖异步运行时，因此**不应在 tokio 异步上下文中
/// 直接调用**。若必须从异步上下文中等待，请用 `tokio::task::spawn_blocking` 隔离：
///
/// ```ignore
/// # use configx::{ConfigWatch, ConfigSubscription};
/// # use std::sync::Arc;
/// # use std::time::Duration;
/// # fn example(watch: Arc<ConfigWatch>) {
/// let mut sub = watch.subscribe();
/// let outcome = tokio::task::spawn_blocking(move || {
///     sub.wait_timeout_outcome(Duration::from_secs(30))
/// }).await??;
/// # }
/// ```
pub struct ConfigSubscription {
    watch: Arc<ConfigWatch>,
    seen: u64,
}

impl ConfigSubscription {
    /// 已经观察到的 generation。
    #[must_use]
    pub fn seen(&self) -> u64 {
        self.seen
    }

    /// 阻塞直到 generation 增长或监听关闭，并显式区分两种结果。
    ///
    /// # Errors
    ///
    /// 状态锁中毒时返回错误。
    pub fn wait_outcome(&mut self) -> ConfigxResult<ConfigWaitOutcome> {
        let watch = Arc::clone(&self.watch);
        let mut state = watch.lock_state()?;
        loop {
            if let Some(outcome) = observe(&mut self.seen, &state) {
                return Ok(outcome);
            }
            state = watch
                .cvar
                .wait(state)
                .map_err(|_| ConfigxError::conflict(WATCH_STATE_LOCK_POISONED_CONTEXT))?;
        }
    }

    /// 兼容等待接口：超时之外的关闭结果折叠为 `None`。
    ///
    /// # Errors
    ///
    /// 状态锁中毒时返回错误。
    pub fn wait(&mut self) -> ConfigxResult<Option<ConfigChange>> {
        Ok(self.wait_outcome()?.into_change())
    }

    /// 在总 deadline 内等待，并显式区分变更、超时与关闭。
    ///
    /// 状态锁只用 `try_lock` 读取，竞争不会造成无界阻塞；每次轮询 sleep 至多 1ms，
    /// 且始终受「调用开始时刻 + timeout」约束。
    ///
    /// # 阻塞调用
    ///
    /// 内部使用 `std::thread::sleep` 实现轮询等待。在 tokio 异步上下文中直接调用会
    /// **冻结当前 OS 线程**，阻止运行时调度其他任务。在单线程运行时
    /// （`current_thread`）下尤为致命：所有并发任务被阻塞，直至本方法返回。
    /// 若必须从异步上下文等待，请用 `tokio::task::spawn_blocking` 隔离。
    ///
    /// # Errors
    ///
    /// 状态锁中毒时返回错误。
    pub fn wait_timeout_outcome(&mut self, timeout: Duration) -> ConfigxResult<ConfigWaitOutcome> {
        let started = Instant::now();
        self.wait_timeout_outcome_with(timeout, || started.elapsed(), thread::sleep)
    }

    /// 可注入时钟与 sleep 的限时等待实现（用于确定性地测试 deadline 语义）。
    fn wait_timeout_outcome_with<Elapsed, Sleep>(
        &mut self,
        timeout: Duration,
        mut elapsed: Elapsed,
        mut sleep: Sleep,
    ) -> ConfigxResult<ConfigWaitOutcome>
    where
        Elapsed: FnMut() -> Duration,
        Sleep: FnMut(Duration),
    {
        let watch = Arc::clone(&self.watch);
        loop {
            let current_elapsed = elapsed();
            match watch.state.try_lock() {
                Ok(state) => {
                    if state.closed {
                        return Ok(ConfigWaitOutcome::Closed);
                    }
                    if state.generation > self.seen {
                        if elapsed() >= timeout {
                            return Ok(ConfigWaitOutcome::TimedOut);
                        }
                        self.seen = state.generation;
                        return Ok(ConfigWaitOutcome::Changed(ConfigChange {
                            generation: state.generation,
                        }));
                    }
                }
                Err(TryLockError::Poisoned(_)) => {
                    return Err(ConfigxError::conflict(WATCH_STATE_LOCK_POISONED_CONTEXT));
                }
                Err(TryLockError::WouldBlock) => {}
            }

            if current_elapsed >= timeout {
                return Ok(ConfigWaitOutcome::TimedOut);
            }
            let remaining = timeout.saturating_sub(current_elapsed);
            sleep(remaining.min(TIMED_WAIT_POLL_INTERVAL));
        }
    }

    /// 兼容限时等待接口：超时与关闭都折叠为 `None`。
    ///
    /// # Errors
    ///
    /// 状态锁中毒时返回错误。
    pub fn wait_timeout(&mut self, timeout: Duration) -> ConfigxResult<Option<ConfigChange>> {
        Ok(self.wait_timeout_outcome(timeout)?.into_change())
    }
}

/// 观察状态：已关闭优先于「有新变更」。
fn observe(seen: &mut u64, state: &WatchState) -> Option<ConfigWaitOutcome> {
    if state.closed {
        return Some(ConfigWaitOutcome::Closed);
    }
    if state.generation > *seen {
        *seen = state.generation;
        return Some(ConfigWaitOutcome::Changed(ConfigChange {
            generation: state.generation,
        }));
    }
    None
}
