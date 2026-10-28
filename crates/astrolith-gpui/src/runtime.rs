//! Application-owned service execution; UI entities never cross this boundary.
use gpui_kit::{App, AppContext, Global, Task};
use std::{future::Future, io, sync::Arc};
use tokio::{
    runtime::{Builder, Handle, Runtime},
    sync::Semaphore,
    task::AbortHandle,
};

struct ServiceRuntime {
    owned: Option<Runtime>,
    handle: Handle,
    blocking_slots: Arc<Semaphore>,
}
impl Global for ServiceRuntime {}

impl Drop for ServiceRuntime {
    fn drop(&mut self) {
        if let Some(runtime) = self.owned.take() {
            // Quit observers drain important work before global disposal. Dropping
            // a runtime normally would block the native event thread here.
            // Bound the final drain: GPUI only gives quit observers 200 ms.
            runtime.shutdown_timeout(std::time::Duration::from_secs(2));
        }
    }
}

pub(crate) fn init(cx: &mut App) -> io::Result<()> {
    if !cx.has_global::<ServiceRuntime>() {
        let runtime = Builder::new_multi_thread()
            .worker_threads(2)
            .max_blocking_threads(4)
            .thread_name("astrolith-service")
            .enable_all()
            .build()?;
        cx.set_global(ServiceRuntime {
            handle: runtime.handle().clone(),
            owned: Some(runtime),
            blocking_slots: Arc::new(Semaphore::new(2)),
        });
    }
    Ok(())
}

struct AbortOnDrop(AbortHandle);
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Bridge service futures without detaching them from the owning GPUI task.
pub(crate) fn spawn<C, F, R>(cx: &C, future: F) -> Task<Result<R, String>>
where
    C: AppContext,
    F: Future<Output = R> + Send + 'static,
    R: Send + 'static,
{
    let join = cx.read_global(|service: &ServiceRuntime, _| service.handle.spawn(future));
    // Construct before the bridge is polled: dropping an unpolled Task must
    // still abort the Tokio task, which may already have started running.
    let cancel = AbortOnDrop(join.abort_handle());
    cx.background_spawn(async move {
        let _cancel = cancel;
        join.await.map_err(|error| error.to_string())
    })
}

/// Limit synchronous service work without occupying GPUI's CPU worker pool.
/// Cancellation cannot interrupt a started operation; its permit remains held.
pub(crate) fn blocking<C, F, R>(cx: &C, work: F) -> Task<Result<R, String>>
where
    C: AppContext,
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let slots = cx.read_global(|service: &ServiceRuntime, _| service.blocking_slots.clone());
    let task = spawn(cx, async move {
        let permit = slots
            .acquire_owned()
            .await
            .map_err(|error| error.to_string())?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            work()
        })
        .await
        .map_err(|error| error.to_string())
    });
    cx.background_spawn(async move { task.await.and_then(|result| result) })
}

pub(crate) fn flush_on_quit<C, F>(cx: &C, work: F) -> Task<Result<(), String>>
where
    C: AppContext,
    F: FnOnce() + Send + 'static,
{
    // Submit immediately, outside ordinary file admission. Unlike cancellable
    // reads, this final atomic save must survive the observer being dropped.
    let join = cx.read_global(|service: &ServiceRuntime, _| service.handle.spawn_blocking(work));
    cx.background_spawn(async move { join.await.map_err(|error| error.to_string()) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::TestAppContext;
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };
    use tokio::sync::oneshot;

    fn setup(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        cx.update(|cx| {
            init(cx).unwrap();
            init(cx).unwrap();
        });
    }

    #[gpui_kit::test]
    async fn shutdown_drains_critical_flush_without_waiting_for_file_admission(
        cx: &mut TestAppContext,
    ) {
        setup(cx);
        let slots = cx.update(|cx| {
            cx.read_global(|service: &ServiceRuntime, _| service.blocking_slots.clone())
        });
        let _held = slots.acquire_many_owned(2).await.unwrap();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let flush = cx.update(|cx| {
            flush_on_quit(cx, move || {
                std::thread::sleep(Duration::from_millis(250));
                done_tx.send(()).unwrap();
            })
        });
        // Simulate GPUI abandoning its quit observer after its 200 ms budget.
        drop(flush);
        cx.update(|cx| drop(cx.remove_global::<ServiceRuntime>()));
        assert_eq!(done_rx.try_recv(), Ok(()));
    }

    #[gpui_kit::test]
    async fn service_results_and_blocking_failures_reach_the_caller(cx: &mut TestAppContext) {
        setup(cx);
        let answer = cx.update(|cx| blocking(cx, || 42)).await;
        assert_eq!(answer.unwrap(), 42);
        let failure = cx
            .update(|cx| blocking(cx, || Err::<(), _>(io::Error::other("disk failed"))))
            .await;
        assert_eq!(failure.unwrap().unwrap_err().to_string(), "disk failed");
        let panic = cx
            .update(|cx| blocking(cx, || panic!("worker failed")))
            .await;
        assert!(panic.unwrap_err().contains("worker failed"));
    }

    #[gpui_kit::test]
    async fn service_future_has_a_working_timer_reactor(cx: &mut TestAppContext) {
        setup(cx);
        let result = cx
            .update(|cx| {
                spawn(cx, async {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                    7
                })
            })
            .await;
        assert_eq!(result.unwrap(), 7);
    }

    struct Dropped(Option<oneshot::Sender<()>>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            if let Some(sender) = self.0.take() {
                let _ = sender.send(());
            }
        }
    }

    #[gpui_kit::test]
    async fn dropping_unpolled_gpui_handle_aborts_its_service_future(cx: &mut TestAppContext) {
        setup(cx);
        let (started_tx, started_rx) = oneshot::channel();
        let (dropped_tx, dropped_rx) = oneshot::channel();
        let dropped = Dropped(Some(dropped_tx));
        let task = cx.update(|cx| {
            spawn(cx, async move {
                let _dropped = dropped;
                let _ = started_tx.send(());
                std::future::pending::<()>().await;
            })
        });
        started_rx.await.unwrap();
        // The Tokio task has started, but its GPUI bridge has never been polled.
        drop(task);
        let cancelled = cx
            .update(|cx| {
                spawn(cx, async {
                    tokio::time::timeout(Duration::from_secs(1), dropped_rx).await
                })
            })
            .await
            .unwrap();
        assert!(
            cancelled.is_ok(),
            "dropping the UI owner must abort service work"
        );
    }

    #[gpui_kit::test]
    async fn cancelled_queued_file_work_never_runs(cx: &mut TestAppContext) {
        setup(cx);
        let slots = cx.update(|cx| {
            cx.read_global(|service: &ServiceRuntime, _| service.blocking_slots.clone())
        });
        let held = slots.acquire_many_owned(2).await.unwrap();
        let (dropped_tx, dropped_rx) = oneshot::channel();
        let dropped = Dropped(Some(dropped_tx));
        let started = Arc::new(AtomicUsize::new(0));
        let counter = started.clone();
        let queued = cx.update(|cx| {
            blocking(cx, move || {
                let _dropped = dropped;
                counter.fetch_add(1, Ordering::SeqCst);
            })
        });
        drop(queued);
        let discarded = cx
            .update(|cx| {
                spawn(cx, async {
                    tokio::time::timeout(Duration::from_secs(1), dropped_rx).await
                })
            })
            .await
            .unwrap();
        assert!(
            discarded.is_ok(),
            "queued work must be discarded before admission opens"
        );
        drop(held);
        cx.update(|cx| blocking(cx, || ())).await.unwrap();
        assert_eq!(started.load(Ordering::SeqCst), 0);
    }

    #[gpui_kit::test]
    async fn cancelled_running_jobs_keep_their_permits_until_they_finish(cx: &mut TestAppContext) {
        setup(cx);
        let mut jobs = Vec::new();
        let mut releases = Vec::new();
        let mut starts = Vec::new();
        for _ in 0..2 {
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            let (started_tx, started_rx) = oneshot::channel();
            releases.push(release_tx);
            starts.push(started_rx);
            jobs.push(cx.update(|cx| {
                blocking(cx, move || {
                    let _ = started_tx.send(());
                    release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
                })
            }));
        }
        for started in starts {
            started.await.unwrap();
        }
        drop(jobs);
        let started = Arc::new(AtomicUsize::new(0));
        let counter = started.clone();
        let third = cx.update(|cx| blocking(cx, move || counter.fetch_add(1, Ordering::SeqCst)));
        cx.update(|cx| {
            spawn(cx, async {
                tokio::time::sleep(Duration::from_millis(20)).await
            })
        })
        .await
        .unwrap();
        assert_eq!(
            started.load(Ordering::SeqCst),
            0,
            "cancelled running work must retain admission permits"
        );
        for release in releases {
            release.send(()).unwrap();
        }
        assert_eq!(third.await.unwrap(), 0);
        assert_eq!(started.load(Ordering::SeqCst), 1);
    }
}
