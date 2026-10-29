use astrolith_astro::{
    Cancellation, DiscoveryOutcome, DiscoveryRequest, ProjectManifest, inspect_project,
};
use astrolith_core::ProjectId;
use astrolith_workspace::{CollectionWatcher, WatchBatch};
use std::{
    fs,
    path::Path,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn watcher_api_is_send_sync_and_drains_without_blocking() {
    let temp = tempfile::tempdir().unwrap();
    let watcher = Arc::new(CollectionWatcher::new(temp.path()).unwrap());
    assert_send_sync::<Arc<CollectionWatcher>>();
    assert_eq!(watcher.drain(), WatchBatch::default());
}

fn wait_for_change(watcher: &CollectionWatcher) -> WatchBatch {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let batch = watcher.drain();
        if batch.changed || batch.needs_rescan {
            return batch;
        }
        assert!(Instant::now() < deadline, "watcher did not observe change");
        thread::sleep(Duration::from_millis(25));
    }
}

fn write_project(root: &Path) {
    fs::create_dir_all(root.join("node_modules/astro")).unwrap();
    fs::create_dir_all(root.join("src/posts/guides")).unwrap();
    fs::write(
        root.join("package.json"),
        r#"{"dependencies":{"astro":"5.18.2"}}"#,
    )
    .unwrap();
    fs::write(
        root.join("node_modules/astro/package.json"),
        r#"{"version":"5.18.2"}"#,
    )
    .unwrap();
    write_content_config(root, "src/posts", 7);
}

fn write_content_config(root: &Path, base: &str, generation: u64) {
    fs::write(
        root.join("src/content.config.ts"),
        format!(
            "import {{defineCollection}} from 'astro:content';\nimport {{glob}} from 'astro/loaders';\nconst docs = defineCollection({{loader:glob({{base:{base:?},pattern:'**/*.md'}})}});\nexport const collections = {{docs}};\n// generation {generation}\n"
        ),
    )
    .unwrap();
}

fn discover(root: &Path, generation: u64) -> ProjectManifest {
    let request = DiscoveryRequest::new(
        ProjectId::new("watcher-fixture").unwrap(),
        generation,
        root.to_path_buf(),
        Cancellation::default(),
    );
    match inspect_project(&request).unwrap() {
        DiscoveryOutcome::Ready(manifest) => manifest,
        DiscoveryOutcome::Unsupported { diagnostics, .. } => {
            panic!("discovery unexpectedly rejected watcher fixture: {diagnostics:?}")
        }
    }
}

fn project() -> (tempfile::TempDir, ProjectManifest) {
    let temp = tempfile::tempdir().unwrap();
    write_project(temp.path());
    let manifest = discover(temp.path(), 7);
    (temp, manifest)
}

fn assert_quiet(watcher: &CollectionWatcher, duration: Duration) {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        assert_eq!(watcher.drain(), WatchBatch::default());
        thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn create_delete_and_rename_mark_changes_after_the_merge_window() {
    let temp = tempfile::tempdir().unwrap();
    let watcher = CollectionWatcher::new(temp.path()).unwrap();
    let first = temp.path().join("first.md");
    let renamed = temp.path().join("renamed.md");

    fs::write(&first, "# first\n").unwrap();
    assert!(wait_for_change(&watcher).changed);
    fs::rename(&first, &renamed).unwrap();
    assert!(wait_for_change(&watcher).changed);
    fs::remove_file(&renamed).unwrap();
    assert!(wait_for_change(&watcher).changed);
}

#[test]
fn burst_merges_into_one_batch_after_one_hundred_milliseconds() {
    let temp = tempfile::tempdir().unwrap();
    let watcher = CollectionWatcher::new(temp.path()).unwrap();
    fs::write(temp.path().join("one.md"), "# one\n").unwrap();
    fs::write(temp.path().join("two.mdx"), "# two\n").unwrap();
    let started = Instant::now();
    let first = wait_for_change(&watcher);
    assert!(started.elapsed() >= Duration::from_millis(90));
    assert!(first.changed);
    thread::sleep(Duration::from_millis(50));
    assert_eq!(watcher.drain(), WatchBatch::default());
}

#[test]
fn package_and_content_config_changes_request_discovery_refresh() {
    let temp = tempfile::tempdir().unwrap();
    let watcher = CollectionWatcher::new(temp.path()).unwrap();
    thread::sleep(Duration::from_millis(50));
    fs::write(
        temp.path().join("content.config.ts"),
        "export const collections = {};\n",
    )
    .unwrap();
    let batch = wait_for_change(&watcher);
    assert!(batch.changed);
    assert!(batch.config_changed);
}

#[test]
fn project_watcher_excludes_node_modules_and_tracks_collection_sources() {
    let (temp, manifest) = project();
    let watcher = CollectionWatcher::for_project(&manifest).unwrap();

    fs::write(
        temp.path().join("node_modules/astro/ignored.md"),
        "# ignored\n",
    )
    .unwrap();
    assert_quiet(&watcher, Duration::from_millis(250));

    fs::write(temp.path().join("src/posts/live.md"), "# live\n").unwrap();
    assert!(wait_for_change(&watcher).changed);
}

#[test]
fn nested_directory_creation_can_be_refreshed_and_tracked() {
    let (temp, manifest) = project();
    let watcher = CollectionWatcher::for_project(&manifest).unwrap();
    let nested = temp.path().join("src/posts/guides/deep");

    fs::create_dir(&nested).unwrap();
    assert!(wait_for_change(&watcher).changed);

    let refreshed = discover(temp.path(), manifest.generation() + 1);
    watcher.refresh(&refreshed).unwrap();
    fs::write(nested.join("nested.md"), "# nested\n").unwrap();
    assert!(wait_for_change(&watcher).changed);
}

#[test]
fn project_config_parent_change_requests_discovery_refresh() {
    let (temp, manifest) = project();
    let watcher = CollectionWatcher::for_project(&manifest).unwrap();

    write_content_config(temp.path(), "src/posts", manifest.generation() + 1);
    let batch = wait_for_change(&watcher);
    assert!(batch.changed);
    assert!(batch.config_changed);
}

#[test]
fn project_root_collection_base_is_selectively_tracked() {
    let (temp, _initial) = project();
    write_content_config(temp.path(), ".", 8);
    let manifest = discover(temp.path(), 8);

    let watcher = CollectionWatcher::for_project(&manifest).unwrap();
    fs::write(temp.path().join("src/posts/root-base.md"), "# root base\n").unwrap();
    assert!(wait_for_change(&watcher).changed);

    fs::write(
        temp.path().join("node_modules/astro/still-ignored.md"),
        "# ignored\n",
    )
    .unwrap();
    assert_quiet(&watcher, Duration::from_millis(250));
}
