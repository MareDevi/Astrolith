use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use astrolith_preview::{RecordingSurface, SurfaceError};
use gpui_kit::{Entity, WindowHandle, base::Root};

use super::*;
use crate::runtime;

/// A backend whose results are scripted, so the panel can be tested headlessly.
struct TestBackend {
    start: Mutex<Result<Url, String>>,
    stop: Mutex<Result<(), String>>,
    started: AtomicBool,
    stopped: AtomicBool,
    aborted: AtomicBool,
}

impl TestBackend {
    fn ready(url: &str) -> Arc<Self> {
        Arc::new(Self {
            start: Mutex::new(Ok(Url::parse(url).unwrap())),
            stop: Mutex::new(Ok(())),
            started: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            aborted: AtomicBool::new(false),
        })
    }

    fn failing(message: &str) -> Arc<Self> {
        Arc::new(Self {
            start: Mutex::new(Err(message.to_owned())),
            stop: Mutex::new(Ok(())),
            started: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            aborted: AtomicBool::new(false),
        })
    }
}

impl PreviewBackend for TestBackend {
    fn start(&self) -> PreviewFuture<Result<Url, String>> {
        self.started.store(true, Ordering::SeqCst);
        let result = self.start.lock().unwrap().clone();
        Box::pin(async move { result })
    }

    fn stop(&self) -> PreviewFuture<Result<(), String>> {
        self.stopped.store(true, Ordering::SeqCst);
        let result = self.stop.lock().unwrap().clone();
        Box::pin(async move { result })
    }

    fn abort(&self) {
        self.aborted.store(true, Ordering::SeqCst);
    }
}

fn mapping() -> RouteMapping {
    RouteMapping::new().with_collection("docs", "docs")
}

/// A root view so the panel renders inside a real test window, the same way the
/// shell hosts it. Without a window the test scheduler cannot drive the runtime
/// bridge deterministically.
struct Host {
    panel: Entity<PreviewPanel>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.panel.clone()
    }
}

fn mount(
    cx: &mut gpui_kit::TestAppContext,
    backend: Arc<TestBackend>,
) -> (
    WindowHandle<Root>,
    Entity<PreviewPanel>,
    Arc<RecordingSurface>,
) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        runtime::init(cx).unwrap();
        let surface = Arc::new(RecordingSurface::new());
        let mut created = None;
        let (handle, _host) =
            gpui_kit::open_window(gpui_kit::WindowOptions::default(), cx, |_window, cx| {
                let panel = cx.new(|cx| PreviewPanel::with_surface(cx, surface.clone()));
                panel.update(cx, |panel, cx| {
                    panel.attach(
                        backend,
                        ProjectId::new("preview-test").unwrap(),
                        PathBuf::from("/tmp/preview-test"),
                        mapping(),
                        cx,
                    );
                });
                created = Some(panel.clone());
                cx.new(|_cx| Host { panel })
            })
            .unwrap();
        (
            handle.downcast::<Root>().unwrap(),
            created.unwrap(),
            surface,
        )
    })
}

/// Serialize tests that drive the application service runtime.
///
/// The GPUI test scheduler and a per-test Tokio runtime do not compose reliably
/// when several of these scenarios run concurrently in one process: the
/// notification the condition waits on can be missed under load. The guard is a
/// spin flag rather than a `MutexGuard` so it never lives across an `await`.
struct Serial;

static BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

impl Serial {
    fn acquire() -> Self {
        while BUSY
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            std::thread::yield_now();
        }
        Self
    }
}

impl Drop for Serial {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::SeqCst);
    }
}

#[gpui_kit::test]
async fn start_becomes_ready_and_reports_the_url(cx: &mut gpui_kit::TestAppContext) {
    let _serial = Serial::acquire();
    let backend = TestBackend::ready("http://127.0.0.1:4321/");
    let (_handle, panel, _surface) = mount(cx, backend.clone());
    panel.update(cx, |panel, cx| panel.start(cx));
    cx.condition(&panel, |panel, _| panel.status().is_ready())
        .await;
    panel.read_with(cx, |panel, _| {
        assert!(backend.started.load(Ordering::SeqCst));
        assert_eq!(
            panel.status().url().map(|url| url.to_string()),
            Some("http://127.0.0.1:4321/".to_owned())
        );
        assert!(
            panel
                .message()
                .unwrap_or_default()
                .contains("Preview ready"),
            "message was {:?}",
            panel.message()
        );
    });
}

#[gpui_kit::test]
async fn start_failure_is_reported_and_not_ready(cx: &mut gpui_kit::TestAppContext) {
    let _serial = Serial::acquire();
    let backend = TestBackend::failing("astro is not installed");
    let (_handle, panel, _surface) = mount(cx, backend);
    panel.update(cx, |panel, cx| panel.start(cx));
    cx.condition(&panel, |panel, _| {
        matches!(panel.status(), ServerStatus::Failed(_))
    })
    .await;
    panel.read_with(cx, |panel, _| {
        assert!(
            panel
                .message()
                .unwrap_or_default()
                .contains("astro is not installed"),
            "message was {:?}",
            panel.message()
        );
    });
}

#[gpui_kit::test]
async fn stop_returns_to_stopped(cx: &mut gpui_kit::TestAppContext) {
    let _serial = Serial::acquire();
    let backend = TestBackend::ready("http://127.0.0.1:4321/");
    let (_handle, panel, _surface) = mount(cx, backend.clone());
    panel.update(cx, |panel, cx| panel.start(cx));
    cx.condition(&panel, |panel, _| panel.status().is_ready())
        .await;
    panel.update(cx, |panel, cx| panel.stop(cx));
    cx.condition(&panel, |panel, _| {
        matches!(panel.status(), ServerStatus::Stopped)
    })
    .await;
    assert!(backend.stopped.load(Ordering::SeqCst));
}

#[gpui_kit::test]
fn detach_aborts_the_backend_and_resets_state(cx: &mut gpui_kit::TestAppContext) {
    let backend = TestBackend::ready("http://127.0.0.1:4321/");
    let (_handle, panel, _surface) = mount(cx, backend.clone());
    panel.update(cx, |panel, cx| panel.detach(cx));
    panel.read_with(cx, |panel, _| {
        assert!(backend.aborted.load(Ordering::SeqCst));
        assert!(panel.project().is_none());
        assert!(matches!(panel.status(), ServerStatus::Stopped));
    });
}

#[gpui_kit::test]
fn open_without_a_ready_server_reports_not_ready(cx: &mut gpui_kit::TestAppContext) {
    let backend = TestBackend::ready("http://127.0.0.1:4321/");
    let (_handle, panel, surface) = mount(cx, backend);
    let entry = EntryId::new("docs", "guides/nested").unwrap();
    panel.update(cx, |panel, cx| {
        panel.set_entry(Some(entry.clone()), cx);
        panel.open_entry(&entry, cx);
    });
    panel.read_with(cx, |panel, _| {
        assert!(
            panel.message().unwrap_or_default().contains("not ready"),
            "message was {:?}",
            panel.message()
        );
    });
    assert!(surface.opened().is_empty());
}

#[gpui_kit::test]
fn open_ready_entry_launches_the_surface(cx: &mut gpui_kit::TestAppContext) {
    let backend = TestBackend::ready("http://127.0.0.1:4321/");
    let (_handle, panel, surface) = mount(cx, backend);
    // Set the ready state directly: the start path is covered separately, and a
    // synchronous open keeps this test independent of scheduler timing.
    panel.update(cx, |panel, cx| {
        panel.status = ServerStatus::Ready(Url::parse("http://127.0.0.1:4321/").unwrap());
        cx.notify();
    });
    let entry = EntryId::new("docs", "guides/nested").unwrap();
    panel.update(cx, |panel, cx| {
        panel.set_entry(Some(entry.clone()), cx);
        panel.open_entry(&entry, cx);
    });
    assert_eq!(
        surface.opened(),
        vec!["http://127.0.0.1:4321/docs/guides/nested".to_owned()]
    );
}

#[gpui_kit::test]
fn surface_failure_is_surfaced(cx: &mut gpui_kit::TestAppContext) {
    let backend = TestBackend::ready("http://127.0.0.1:4321/");
    let (_handle, panel, _surface) = mount(cx, backend);
    panel.update(cx, |panel, cx| {
        panel.status = ServerStatus::Ready(Url::parse("http://127.0.0.1:4321/").unwrap());
        cx.notify();
    });
    panel.update(cx, |panel, cx| {
        panel.surface = Arc::new(RecordingSurface::failing(SurfaceError::LaunchFailed {
            command: "xdg-open".to_owned(),
            message: "missing".to_owned(),
        }));
        let entry = EntryId::new("docs", "intro").unwrap();
        panel.open_entry(&entry, cx);
    });
    panel.read_with(cx, |panel, _| {
        assert!(
            panel
                .message()
                .unwrap_or_default()
                .contains("could not launch"),
            "message was {:?}",
            panel.message()
        );
    });
}
