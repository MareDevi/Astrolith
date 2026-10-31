//! M05 gate (opt-in): a saved entry edit is reflected by the real preview.
//!
//! This is the plan's M05 acceptance check — "editing an article and saving it
//! results in an updated real Astro preview page" — exercised against the
//! project's own installed Astro release rather than a mock:
//!
//! 1. copy the frozen `astro7` fixture to a temporary directory,
//! 2. `bun install --frozen-lockfile` it,
//! 3. start the real `astro dev` through [`DevServer`],
//! 4. fetch the entry route's HTML and assert the original title is served,
//! 5. rewrite the entry's frontmatter and body on disk (what a save does),
//! 6. poll until the served HTML reflects the new title and body,
//! 7. stop the server and assert it stopped and released its port.
//!
//! It is `#[ignore]`-d because it installs packages and starts a server:
//!
//! ```text
//! cargo test -p astrolith-preview --locked --test preview_gate -- --ignored --nocapture
//! ```
//!
//! Requirements: `bun` and `node` on `PATH` and network/cache access for the
//! first install. The browser itself is not launched: native browser and
//! rendering acceptance remain manual evidence.

use std::{
    io::{Read, Write},
    net::TcpStream,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use astrolith_core::{EntryId, ProjectId};
use astrolith_preview::{
    DevServer, DevServerConfig, RouteMapping, ServerStatus, resolve_entry_url,
};
use url::Url;

/// Fixture used by the gate. Astro 7 is the newest supported release.
const FIXTURE: &str = "fixtures/astro7";
/// Entry rendered by the fixture's `workspace/pages/docs/[...slug].astro`.
///
/// `content/docs/introduction.md` declares a frontmatter `slug`, so the content
/// layer's entry id — and therefore the route segment — is `stable-source-slug`.
const ENTRY_ID: &str = "docs/stable-source-slug";
/// How long the dev server may take to reflect a saved edit.
const REFRESH_DEADLINE: Duration = Duration::from_secs(60);

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

fn copy_dir_all(source: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let destination = target.join(entry.file_name());
        if kind.is_dir() {
            copy_dir_all(&entry.path(), &destination)?;
        } else {
            std::fs::copy(entry.path(), destination)?;
        }
    }
    Ok(())
}

fn install(project: &Path) -> Result<(), String> {
    let output = std::process::Command::new("bun")
        .args(["install", "--frozen-lockfile"])
        .current_dir(project)
        .output()
        .map_err(|error| format!("could not run bun: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "bun install failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

/// Minimal HTTP/1.1 GET. The response is returned as lossy text because the
/// assertions look for markup markers rather than parsing a full response.
fn http_get(url: &Url) -> std::io::Result<String> {
    let host = url.host_str().unwrap_or("127.0.0.1");
    let port = url.port().unwrap_or(80);
    let mut stream = TcpStream::connect((host, port))?;
    stream.set_read_timeout(Some(Duration::from_secs(15)))?;
    write!(
        stream,
        "GET {} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n",
        url.path()
    )?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    Ok(response)
}

fn entry_url(base: &Url) -> Url {
    let entry = EntryId::new(
        ENTRY_ID.split_once('/').expect("collection/id").0,
        ENTRY_ID.split_once('/').expect("collection/id").1,
    )
    .expect("valid entry id");
    let mapping = RouteMapping::new().with_collection("docs", "docs");
    resolve_entry_url(&entry, &mapping, base).expect("entry route resolves")
}

fn wait_for(deadline: Duration, mut attempt: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < deadline {
        if attempt() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(400));
    }
    false
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("build runtime")
}

#[test]
#[ignore = "opt-in: installs the frozen fixture and runs the real astro dev server"]
fn saved_edit_is_reflected_by_the_running_astro_preview() {
    let fixture = repo_root().join(FIXTURE);
    let temp = tempfile::tempdir().expect("temp dir");
    let project = temp.path().join("project");
    copy_dir_all(&fixture, &project).expect("copy fixture");
    install(&project).expect("install fixture dependencies");

    let runtime = runtime();
    let mut server = DevServer::new(
        ProjectId::new("preview-gate").expect("project id"),
        DevServerConfig::new(&project).with_startup_timeout(Duration::from_secs(120)),
    );
    let base = runtime
        .block_on(server.start())
        .expect("dev server becomes ready");
    assert!(
        matches!(server.status(), ServerStatus::Ready(_)),
        "status must be Ready after start, was {:?}",
        server.status()
    );
    let url = entry_url(&base);
    assert_eq!(url.path(), "/docs/stable-source-slug");

    // 1. The installed project serves the entry route with its current title.
    let html = http_get(&url).expect("GET entry route");
    assert!(
        html.contains("data-astrolith-preview"),
        "the route must be served by the fixture page, got:\n{html}"
    );
    assert!(
        html.contains("Introduction"),
        "the original title must be served, got:\n{html}"
    );

    // 2. Save a frontmatter + body edit, exactly as the editor's save writes it.
    let source = project.join("content/docs/introduction.md");
    let original = std::fs::read_to_string(&source).expect("read entry");
    let edited = original
        .replace("title: Introduction", "title: Preview Gate Updated")
        .replace("# Introduction", "# Preview Gate Body 中文");
    assert_ne!(original, edited, "the edit must change the entry");
    std::fs::write(&source, &edited).expect("write entry");

    // 3. Astro's file watcher must re-serve the page with the new content.
    let refreshed = wait_for(REFRESH_DEADLINE, || {
        http_get(&url)
            .map(|html| {
                html.contains("Preview Gate Updated") && html.contains("Preview Gate Body 中文")
            })
            .unwrap_or(false)
    });
    assert!(
        refreshed,
        "the saved edit must appear in the served preview within {REFRESH_DEADLINE:?}"
    );

    // 4. Stopping releases the port and leaves no orphaned dev server behind.
    runtime.block_on(server.stop()).expect("stop server");
    assert!(
        matches!(server.status(), ServerStatus::Stopped),
        "status must be Stopped after stop, was {:?}",
        server.status()
    );
    let port = base.port().unwrap_or(80);
    let released = wait_for(Duration::from_secs(10), || {
        TcpStream::connect(("127.0.0.1", port)).is_err()
    });
    assert!(released, "the dev server port must be released after stop");
}
