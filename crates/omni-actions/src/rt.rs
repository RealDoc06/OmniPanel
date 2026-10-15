//! Spawning background tasks from sync code.

use std::future::Future;
use std::sync::OnceLock;

use tokio::runtime::{Builder, Handle, Runtime};

/// Spawn on the caller's tokio runtime. `Executor::configure` is sync and may
/// be called from a plain thread, in which case a single-worker runtime
/// (created on first use) runs the integration tasks.
pub(crate) fn spawn<F>(task: F) -> tokio::task::JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    static FALLBACK: OnceLock<Runtime> = OnceLock::new();
    match Handle::try_current() {
        Ok(handle) => handle.spawn(task),
        Err(_) => FALLBACK
            .get_or_init(|| {
                Builder::new_multi_thread()
                    .worker_threads(1)
                    .thread_name("omni-actions")
                    .enable_all()
                    .build()
                    .expect("failed to start the omni-actions runtime")
            })
            .spawn(task),
    }
}
