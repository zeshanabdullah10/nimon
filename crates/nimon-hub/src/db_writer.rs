//! Single ordered database writer.
//!
//! Every hub component that writes to SQLite (WebSocket sessions, the
//! alert manager, the action executor) submits its writes to one
//! [`DbWriter`]. Operations run one at a time on one worker, so dependent
//! rows are written in the order they were produced (edge row before
//! device row before alert/action rows) and a later write can never
//! overtake an earlier one of the same lane (e.g. `mark_notified` before
//! the alert insert).
//!
//! Two lanes:
//! - **priority** ([`DbWriter::submit`], [`DbWriter::call`]): alerts,
//!   actions, acknowledgements, registry rows. Never dropped; always run
//!   before queued low-priority work, so API calls do not wait behind a
//!   status backlog.
//! - **low** ([`DbWriter::submit_low`]): high-volume, self-refreshing data
//!   (device status snapshots, metric/prediction history, periodic
//!   maintenance). Bounded; when full (e.g. the database is write-locked
//!   by another process) new low-priority writes are shed instead of
//!   growing memory without bound. A low write never overtakes an earlier
//!   priority write (the worker only takes low work when the priority lane
//!   is empty); it may be overtaken by later priority writes, so nothing
//!   that a priority write depends on may be submitted as low.
//!
//! Each operation runs in its own task with a deadline: a panicking op is
//! logged and the worker continues; an op exceeding the deadline is
//! aborted. [`DbWriter::health`] exposes liveness for `/health`.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use sqlx::SqlitePool;
use tokio::sync::{mpsc, oneshot};
use tracing::{error, warn};

type BoxFut = Pin<Box<dyn Future<Output = ()> + Send>>;
type DbOp = Box<dyn FnOnce(SqlitePool) -> BoxFut + Send>;

/// Default capacity of the low-priority lane
pub const DEFAULT_LOW_CAPACITY: usize = 4096;
/// Default per-operation deadline
pub const DEFAULT_OP_TIMEOUT: Duration = Duration::from_secs(30);

/// Writer tuning
#[derive(Debug, Clone, Copy)]
pub struct DbWriterOptions {
    /// Low-priority writes queued before new ones are shed
    pub low_capacity: usize,
    /// Longest a single operation may run before it is aborted
    pub op_timeout: Duration,
}

impl Default for DbWriterOptions {
    fn default() -> Self {
        Self {
            low_capacity: DEFAULT_LOW_CAPACITY,
            op_timeout: DEFAULT_OP_TIMEOUT,
        }
    }
}

/// Writer liveness snapshot
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DbWriterHealth {
    /// The worker task is running
    pub alive: bool,
    /// Operations aborted by the deadline since the last successful one
    pub consecutive_timeouts: u64,
    /// Operations that panicked (total)
    pub panics: u64,
    /// Low-priority writes shed because the lane was full (total)
    pub shed: u64,
}

impl DbWriterHealth {
    /// Healthy: running and not currently timing out
    pub fn is_ok(&self) -> bool {
        self.alive && self.consecutive_timeouts == 0
    }
}

#[derive(Default)]
struct Stats {
    alive: AtomicBool,
    consecutive_timeouts: AtomicU64,
    panics: AtomicU64,
    shed: AtomicU64,
}

/// Marks the worker dead however it exits
struct AliveGuard(Arc<Stats>);

impl Drop for AliveGuard {
    fn drop(&mut self) {
        self.0.alive.store(false, Ordering::SeqCst);
    }
}

/// Cloneable handle to the ordered writer task.
#[derive(Clone)]
pub struct DbWriter {
    high: mpsc::UnboundedSender<DbOp>,
    low: mpsc::Sender<DbOp>,
    pool: SqlitePool,
    stats: Arc<Stats>,
}

impl DbWriter {
    /// Spawn the writer task on the current tokio runtime.
    pub fn spawn(pool: SqlitePool) -> Self {
        Self::spawn_with(pool, DbWriterOptions::default())
    }

    /// Spawn the writer task with explicit tuning.
    pub fn spawn_with(pool: SqlitePool, options: DbWriterOptions) -> Self {
        let (high, mut high_rx) = mpsc::unbounded_channel::<DbOp>();
        let (low, mut low_rx) = mpsc::channel::<DbOp>(options.low_capacity.max(1));
        let stats = Arc::new(Stats::default());
        stats.alive.store(true, Ordering::SeqCst);
        let task_pool = pool.clone();
        let task_stats = Arc::clone(&stats);
        tokio::spawn(async move {
            let _alive = AliveGuard(Arc::clone(&task_stats));
            let mut low_open = true;
            loop {
                let op = tokio::select! {
                    biased;
                    op = high_rx.recv() => match op {
                        Some(op) => op,
                        // Every handle dropped: drain what is left, stop
                        None => {
                            while let Ok(op) = low_rx.try_recv() {
                                run_op(op, &task_pool, &task_stats, options.op_timeout).await;
                            }
                            break;
                        }
                    },
                    op = low_rx.recv(), if low_open => match op {
                        Some(op) => op,
                        None => {
                            low_open = false;
                            continue;
                        }
                    },
                };
                run_op(op, &task_pool, &task_stats, options.op_timeout).await;
            }
        });
        Self {
            high,
            low,
            pool,
            stats,
        }
    }

    /// The pool the writer uses (for reads).
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Liveness snapshot (for `/health`)
    pub fn health(&self) -> DbWriterHealth {
        DbWriterHealth {
            alive: self.stats.alive.load(Ordering::SeqCst),
            consecutive_timeouts: self.stats.consecutive_timeouts.load(Ordering::SeqCst),
            panics: self.stats.panics.load(Ordering::SeqCst),
            shed: self.stats.shed.load(Ordering::SeqCst),
        }
    }

    /// Enqueue a priority write (never dropped). Runs after every
    /// previously submitted priority write.
    pub fn submit<F, Fut>(&self, op: F)
    where
        F: FnOnce(SqlitePool) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let boxed: DbOp = Box::new(move |pool| Box::pin(op(pool)));
        if self.high.send(boxed).is_err() {
            warn!("database writer is stopped; dropping write");
        }
    }

    /// Enqueue a low-priority, sheddable write (status snapshots, history
    /// samples, maintenance). Returns false when it was shed.
    pub fn submit_low<F, Fut>(&self, op: F) -> bool
    where
        F: FnOnce(SqlitePool) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let boxed: DbOp = Box::new(move |pool| Box::pin(op(pool)));
        match self.low.try_send(boxed) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                let shed = self.stats.shed.fetch_add(1, Ordering::SeqCst) + 1;
                if shed == 1 || shed.is_multiple_of(1000) {
                    warn!(
                        "database writer backlog full: shedding low-priority writes ({} so far)",
                        shed
                    );
                }
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                warn!("database writer is stopped; dropping write");
                false
            }
        }
    }

    /// Enqueue a priority operation whose result the caller wants back.
    /// The receiver errors when the writer is stopped or the operation
    /// panicked or timed out.
    pub fn call<T, F, Fut>(&self, op: F) -> oneshot::Receiver<T>
    where
        T: Send + 'static,
        F: FnOnce(SqlitePool) -> Fut + Send + 'static,
        Fut: Future<Output = T> + Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        self.submit(move |pool| async move {
            let _ = tx.send(op(pool).await);
        });
        rx
    }

    /// Wait until every write (both lanes) submitted before this call has
    /// completed. The barrier travels the low lane: it runs after the
    /// earlier low writes (FIFO) and only once the priority lane is empty.
    pub async fn flush(&self) {
        let (tx, rx) = oneshot::channel::<()>();
        let barrier: DbOp = Box::new(move |_| {
            Box::pin(async move {
                let _ = tx.send(());
            })
        });
        if self.low.send(barrier).await.is_ok() {
            let _ = rx.await;
        }
    }
}

/// Run one operation in its own task: a panic is contained, a deadline
/// overrun aborts it.
async fn run_op(op: DbOp, pool: &SqlitePool, stats: &Stats, timeout: Duration) {
    let mut handle = tokio::spawn(op(pool.clone()));
    match tokio::time::timeout(timeout, &mut handle).await {
        Ok(Ok(())) => {
            stats.consecutive_timeouts.store(0, Ordering::SeqCst);
        }
        Ok(Err(e)) if e.is_panic() => {
            stats.panics.fetch_add(1, Ordering::SeqCst);
            stats.consecutive_timeouts.store(0, Ordering::SeqCst);
            error!("database write panicked (writer continues): {}", e);
        }
        Ok(Err(e)) => {
            warn!("database write was cancelled: {}", e);
        }
        Err(_) => {
            handle.abort();
            let n = stats.consecutive_timeouts.fetch_add(1, Ordering::SeqCst) + 1;
            error!(
                "database write exceeded {}s and was aborted ({} in a row)",
                timeout.as_secs_f32(),
                n
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    async fn pool() -> SqlitePool {
        nimon_core::db::connect(":memory:").await.unwrap()
    }

    #[tokio::test]
    async fn test_writes_run_in_order() {
        let pool = pool().await;
        let writer = DbWriter::spawn(pool.clone());
        writer.submit(|pool| async move {
            sqlx::query("CREATE TABLE t (v INTEGER)")
                .execute(&pool)
                .await
                .unwrap();
        });
        for i in 0..20 {
            writer.submit(move |pool| async move {
                sqlx::query("INSERT INTO t (v) VALUES (?)")
                    .bind(i)
                    .execute(&pool)
                    .await
                    .unwrap();
            });
        }
        let values: Vec<i64> = writer
            .call(|pool| async move {
                sqlx::query_scalar("SELECT v FROM t ORDER BY rowid")
                    .fetch_all(&pool)
                    .await
                    .unwrap()
            })
            .await
            .unwrap();
        assert_eq!(values, (0..20).collect::<Vec<i64>>());
    }

    /// Block the worker until the returned sender fires
    fn block(writer: &DbWriter) -> oneshot::Sender<()> {
        let (tx, rx) = oneshot::channel::<()>();
        writer.submit(move |_| async move {
            let _ = rx.await;
        });
        tx
    }

    #[tokio::test]
    async fn test_low_priority_is_bounded_and_shed_priority_never_dropped() {
        let writer = DbWriter::spawn_with(
            pool().await,
            DbWriterOptions {
                low_capacity: 8,
                ..Default::default()
            },
        );
        let gate = block(&writer);
        tokio::task::yield_now().await;
        let order = Arc::new(Mutex::new(Vec::new()));
        let mut accepted = 0;
        for i in 0..100 {
            let order = Arc::clone(&order);
            if writer
                .submit_low(move |_| async move { order.lock().unwrap().push(format!("low{}", i)) })
            {
                accepted += 1;
            }
        }
        assert!(accepted <= 9, "bounded lane: {} accepted", accepted);
        assert!(writer.health().shed >= 91);
        // Priority writes are never shed, and run before the low backlog
        for i in 0..100 {
            let order = Arc::clone(&order);
            writer.submit(move |_| async move { order.lock().unwrap().push(format!("high{}", i)) });
        }
        let order_ack = Arc::clone(&order);
        let ack = writer.call(move |_| async move {
            order_ack.lock().unwrap().push("ack".to_string());
        });
        gate.send(()).unwrap();
        ack.await.unwrap();
        // The low backlog drains afterwards; flush covers both lanes
        writer.flush().await;
        let seen = order.lock().unwrap();
        assert_eq!(seen.len(), 101 + accepted, "{:?}", seen);
        assert_eq!(seen.iter().filter(|s| s.starts_with("high")).count(), 100);
        let ack_at = seen.iter().position(|s| s == "ack").unwrap();
        let first_low = seen.iter().position(|s| s.starts_with("low")).unwrap();
        assert!(ack_at < first_low, "ack ran before the queued low writes");
    }

    #[tokio::test]
    async fn test_panic_in_one_op_does_not_kill_the_writer() {
        let writer = DbWriter::spawn(pool().await);
        writer.submit(|_| async { panic!("boom") });
        let value = writer.call(|_| async { 42 }).await.unwrap();
        assert_eq!(value, 42);
        let health = writer.health();
        assert!(health.alive);
        assert_eq!(health.panics, 1);
        assert!(health.is_ok());
    }

    #[tokio::test]
    async fn test_op_timeout_aborts_and_is_reported() {
        let writer = DbWriter::spawn_with(
            pool().await,
            DbWriterOptions {
                op_timeout: Duration::from_millis(100),
                ..Default::default()
            },
        );
        let stuck = writer.call(|_| async {
            tokio::time::sleep(Duration::from_secs(30)).await;
        });
        let started = std::time::Instant::now();
        assert!(stuck.await.is_err(), "the stuck op is aborted");
        assert!(started.elapsed() < Duration::from_secs(5));
        let health = writer.health();
        assert_eq!(health.consecutive_timeouts, 1);
        assert!(!health.is_ok());
        // The next op runs and clears the condition
        writer.call(|_| async {}).await.unwrap();
        assert!(writer.health().is_ok());
    }
}
