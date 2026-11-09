use super::*;
use crate::discovery::Cancellation;
use crate::schema::model::FieldKind;
use std::fs;
use std::path::Path;

#[cfg(unix)]
use crate::schema::test_support::SPAWN_LOCK;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::path::PathBuf;

#[cfg(unix)]
const VALID_DOCS: &str = r#"#!/bin/sh
mkdir -p .astro/collections
cat > .astro/collections/docs.schema.json <<'JSON'
{"type":"object","properties":{"title":{"type":"string"}},"required":["title"]}
JSON
"#;

#[cfg(unix)]
fn install_fake_project(root: &Path, script: &str) -> PathBuf {
    let astro = root.join("node_modules/astro");
    fs::create_dir_all(&astro).unwrap();
    fs::write(
        astro.join("package.json"),
        r#"{"name":"astro","bin":{"astro":"astro.js"}}"#,
    )
    .unwrap();
    fs::write(astro.join("astro.js"), "// fake Astro entry\n").unwrap();
    let node = root.join("fake-node.sh");
    fs::write(&node, script).unwrap();
    let mut permissions = fs::metadata(&node).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&node, permissions).unwrap();
    node
}

#[cfg(unix)]
#[test]
fn generates_and_normalizes_collection_schemas() {
    let temp = tempfile::tempdir().unwrap();
    let _guard = SPAWN_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let node = install_fake_project(temp.path(), VALID_DOCS);
    // A matching content config keeps the helper pass a no-op.
    fs::create_dir_all(temp.path().join("src")).unwrap();
    fs::write(
        temp.path().join("src/content.config.ts"),
        "const docs = defineCollection({ schema: z.object({ title: z.string() }) });\nexport const collections = { docs };\n",
    )
    .unwrap();
    let request =
        SchemaGenerationRequest::new(temp.path(), Cancellation::default()).with_node(node);

    let report = generate_collection_schemas(&request).expect("generation succeeds");
    assert_eq!(report.schemas.len(), 1);
    assert_eq!(report.schemas[0].collection, "docs");
    assert_eq!(report.schemas[0].schema.fields.len(), 1);
    assert_eq!(report.schemas[0].schema.fields[0].name, "title");
    assert!(report.diagnostics.is_empty());
}

#[cfg(unix)]
#[test]
fn nonzero_sync_exit_still_loads_written_schemas() {
    let temp = tempfile::tempdir().unwrap();
    let _guard = SPAWN_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let node = install_fake_project(
        temp.path(),
        r#"#!/bin/sh
mkdir -p .astro/collections
cat > .astro/collections/docs.schema.json <<'JSON'
{"type":"object","properties":{"title":{"type":"string"}}}
JSON
echo "content validation failed" >&2
exit 3
"#,
    );
    let request =
        SchemaGenerationRequest::new(temp.path(), Cancellation::default()).with_node(node);

    let report = generate_collection_schemas(&request).expect("schemas still load");
    assert_eq!(report.schemas.len(), 1);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.contains("exited"))
    );
}

#[cfg(unix)]
#[test]
fn malformed_schema_is_diagnosed_without_losing_valid_ones() {
    let temp = tempfile::tempdir().unwrap();
    let _guard = SPAWN_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let node = install_fake_project(
        temp.path(),
        r#"#!/bin/sh
mkdir -p .astro/collections
cat > .astro/collections/docs.schema.json <<'JSON'
{"type":"object","properties":{"title":{"type":"string"}}}
JSON
printf 'not json' > .astro/collections/broken.schema.json
"#,
    );
    let request =
        SchemaGenerationRequest::new(temp.path(), Cancellation::default()).with_node(node);

    let report = generate_collection_schemas(&request).expect("valid schema still loads");
    assert_eq!(report.schemas.len(), 1);
    assert_eq!(report.schemas[0].collection, "docs");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.contains("broken"))
    );
}

#[cfg(unix)]
#[test]
fn missing_generated_schemas_is_an_error() {
    let temp = tempfile::tempdir().unwrap();
    let _guard = SPAWN_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let node = install_fake_project(temp.path(), "#!/bin/sh\nexit 0\n");
    let request =
        SchemaGenerationRequest::new(temp.path(), Cancellation::default()).with_node(node);

    let error = generate_collection_schemas(&request).expect_err("no schemas is an error");
    assert!(error.to_string().contains("produce"));
}

#[cfg(unix)]
#[test]
fn timeout_kills_the_tool() {
    let temp = tempfile::tempdir().unwrap();
    let _guard = SPAWN_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let node = install_fake_project(temp.path(), "#!/bin/sh\nsleep 5\n");
    let request = SchemaGenerationRequest::new(temp.path(), Cancellation::default())
        .with_node(node)
        .with_timeout(std::time::Duration::from_millis(200));

    let started = std::time::Instant::now();
    let error = generate_collection_schemas(&request).expect_err("timeout is an error");
    assert!(error.to_string().contains("timed out"));
    assert!(started.elapsed() < std::time::Duration::from_secs(4));
}

#[cfg(unix)]
#[test]
fn cancellation_kills_the_tool() {
    let temp = tempfile::tempdir().unwrap();
    let _guard = SPAWN_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let node = install_fake_project(temp.path(), "#!/bin/sh\nsleep 5\n");
    let cancellation = Cancellation::default();
    let trigger = cancellation.clone();
    let handle = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(100));
        trigger.cancel();
    });
    let request = SchemaGenerationRequest::new(temp.path(), cancellation)
        .with_node(node)
        .with_timeout(std::time::Duration::from_secs(10));

    let error = generate_collection_schemas(&request).expect_err("cancellation is an error");
    assert!(error.to_string().contains("cancelled"));
    handle.join().unwrap();
}

#[test]
fn missing_installation_is_reported() {
    let temp = tempfile::tempdir().unwrap();
    let error = resolve_astro_entry(temp.path()).expect_err("missing astro is an error");
    assert!(error.to_string().contains("not installed"));
}

#[test]
fn freshness_compares_schema_and_config_timestamps() {
    use std::time::{Duration, SystemTime};

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir_all(root.join(".astro/collections")).unwrap();
    let schema = root.join(".astro/collections/docs.schema.json");
    fs::write(&schema, "{}").unwrap();
    let config = root.join("workspace/content.config.mjs");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(&config, "").unwrap();

    let set_modified = |path: &Path, seconds: u64| {
        let file = fs::File::options().write(true).open(path).unwrap();
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(seconds);
        file.set_times(fs::FileTimes::new().set_modified(time))
            .unwrap();
    };

    set_modified(&schema, 1_000);
    set_modified(&config, 2_000);
    assert!(
        schemas_are_stale(root, &config).unwrap(),
        "older schema is stale"
    );

    set_modified(&schema, 3_000);
    assert!(
        !schemas_are_stale(root, &config).unwrap(),
        "newer schema is fresh"
    );

    let other = tempfile::tempdir().unwrap();
    let other_config = other.path().join("content.config.mjs");
    fs::write(&other_config, "").unwrap();
    assert!(
        schemas_are_stale(other.path(), &other_config).unwrap(),
        "missing .astro is stale"
    );
}

#[test]
fn freshness_rejects_superseded_generation_results() {
    let mut freshness = SchemaFreshness::default();
    assert_eq!(freshness.installed(), None);

    let SchemaRefresh::Regenerate { request: first } = freshness.reconcile(10) else {
        panic!("the initial fingerprint must regenerate");
    };
    assert_eq!(
        freshness.reconcile(10),
        SchemaRefresh::Regenerate { request: first },
        "polling the same fingerprint reuses the in-flight request"
    );

    let SchemaRefresh::Regenerate { request: second } = freshness.reconcile(20) else {
        panic!("a changed fingerprint must regenerate");
    };
    assert_ne!(first, second);
    assert!(
        !freshness.accept(first, 10),
        "a result superseded by a newer fingerprint must be dropped"
    );
    assert_eq!(freshness.installed(), None);
    assert!(freshness.accept(second, 20));
    assert_eq!(freshness.installed(), Some(20));
    assert_eq!(freshness.reconcile(20), SchemaRefresh::Current);

    let SchemaRefresh::Regenerate { request: third } = freshness.reconcile(30) else {
        panic!("a changed fingerprint must regenerate");
    };
    freshness.reject(third);
    assert_eq!(
        freshness.reconcile(30),
        SchemaRefresh::Current,
        "a failed generation is not retried until the fingerprint changes"
    );
    assert!(
        !freshness.accept(third, 30),
        "a rejected request stays stale"
    );

    let SchemaRefresh::Regenerate { request: fourth } = freshness.reconcile(40) else {
        panic!("a new fingerprint after a failure must regenerate");
    };
    assert!(freshness.accept(fourth, 40));
    assert_eq!(freshness.reconcile(40), SchemaRefresh::Current);
}

#[test]
fn freshness_checks_the_pending_fingerprint_before_installing() {
    let mut freshness = SchemaFreshness::default();
    let SchemaRefresh::Regenerate { request } = freshness.reconcile(10) else {
        panic!("the initial fingerprint must regenerate");
    };
    assert!(
        !freshness.accept(request, 11),
        "a fingerprint that changed while generating must not be installed"
    );
    assert_eq!(freshness.installed(), None);
    let SchemaRefresh::Regenerate { request: retry } = freshness.reconcile(11) else {
        panic!("the changed fingerprint must regenerate again");
    };
    assert_ne!(request, retry);
    assert!(freshness.accept(retry, 11));
    assert_eq!(freshness.installed(), Some(11));
    assert_eq!(freshness.reconcile(11), SchemaRefresh::Current);
}

#[test]
fn freshness_retries_a_failed_fingerprint_after_reverting_inputs() {
    let mut freshness = SchemaFreshness::default();
    let SchemaRefresh::Regenerate { request: first } = freshness.reconcile(1) else {
        panic!("the initial fingerprint must regenerate");
    };
    assert!(freshness.accept(first, 1));

    let SchemaRefresh::Regenerate { request: failing } = freshness.reconcile(2) else {
        panic!("a changed fingerprint must regenerate");
    };
    freshness.reject(failing);
    assert_eq!(
        freshness.reconcile(2),
        SchemaRefresh::Current,
        "the failing fingerprint must not be retried every poll"
    );

    // Reverting the inputs to the installed state clears the failure, so going
    // back to the failing fingerprint regenerates instead of staying stuck.
    assert_eq!(freshness.reconcile(1), SchemaRefresh::Current);
    assert!(
        matches!(freshness.reconcile(2), SchemaRefresh::Regenerate { .. }),
        "a failure must not outlive a return to the installed inputs"
    );
}

#[test]
fn schema_fingerprint_tracks_content_not_timestamps() {
    use std::time::{Duration, SystemTime};

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir_all(root.join(".astro/collections")).unwrap();
    let schema = root.join(".astro/collections/docs.schema.json");
    fs::write(&schema, "{\"type\":\"object\"}").unwrap();
    let config = root.join("content.config.mjs");
    fs::write(&config, "// config\n").unwrap();

    let initial = schema_fingerprint(root).unwrap();

    let file = fs::File::options().write(true).open(&schema).unwrap();
    file.set_times(
        fs::FileTimes::new().set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(5_000)),
    )
    .unwrap();
    assert_eq!(
        schema_fingerprint(root).unwrap(),
        initial,
        "a new modification time with identical bytes is not a change"
    );

    fs::write(&schema, "{\"type\":\"array\"}").unwrap();
    let changed = schema_fingerprint(root).unwrap();
    assert_ne!(changed, initial, "changed schema bytes must change it");

    fs::write(&config, "// changed config\n").unwrap();
    assert_ne!(
        schema_fingerprint(root).unwrap(),
        changed,
        "changed content config bytes must change it"
    );
}

#[test]
fn rejects_bin_without_astro_entry() {
    let temp = tempfile::tempdir().unwrap();
    let astro = temp.path().join("node_modules/astro");
    fs::create_dir_all(&astro).unwrap();
    fs::write(astro.join("package.json"), r#"{"bin":{"other":"x.js"}}"#).unwrap();
    let error = resolve_astro_entry(temp.path()).expect_err("missing astro bin is an error");
    assert!(error.to_string().contains("does not declare"));
}

#[test]
fn rejects_bin_that_escapes_the_project() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("outside.js"), "// outside\n").unwrap();
    let root = temp.path().join("project");
    let astro = root.join("node_modules/astro");
    fs::create_dir_all(&astro).unwrap();
    fs::write(
        astro.join("package.json"),
        r#"{"bin":{"astro":"../../../outside.js"}}"#,
    )
    .unwrap();
    let error = resolve_astro_entry(&root).expect_err("escaping bin is an error");
    assert!(error.to_string().contains("escapes"));
}

#[test]
fn freshness_is_stale_when_any_schema_predates_config() {
    use std::time::{Duration, SystemTime};

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir_all(root.join(".astro/collections")).unwrap();
    let first = root.join(".astro/collections/a.schema.json");
    let second = root.join(".astro/collections/b.schema.json");
    fs::write(&first, "{}").unwrap();
    fs::write(&second, "{}").unwrap();
    let config = root.join("content.config.mjs");
    fs::write(&config, "").unwrap();

    let set_modified = |path: &Path, seconds: u64| {
        let file = fs::File::options().write(true).open(path).unwrap();
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(seconds);
        file.set_times(fs::FileTimes::new().set_modified(time))
            .unwrap();
    };
    set_modified(&config, 2_000);
    set_modified(&first, 3_000);
    set_modified(&second, 1_000);
    assert!(
        schemas_are_stale(root, &config).unwrap(),
        "one old schema makes the whole set stale"
    );

    set_modified(&second, 3_000);
    assert!(!schemas_are_stale(root, &config).unwrap());
}

#[test]
fn missing_content_config_is_stale() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("missing.config.mjs");
    assert!(schemas_are_stale(temp.path(), &missing).unwrap());
}

#[cfg(unix)]
const HELPERS_SCHEMA: &str = r#"#!/bin/sh
mkdir -p .astro/collections
cat > .astro/collections/docs.schema.json <<'JSON'
{"type":"object","properties":{"title":{"type":"string"},"cover":{"type":"string"},"author":{"anyOf":[{"type":"string"},{"type":"object","properties":{"id":{"type":"string"},"collection":{"type":"string"}},"required":["id","collection"]},{"type":"object","properties":{"slug":{"type":"string"},"collection":{"type":"string"}},"required":["slug","collection"]}]}}}
JSON
"#;

#[cfg(unix)]
fn write_helpers_config(root: &Path) {
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src/content.config.ts"),
        "const docs = defineCollection({ schema: ({ image }) => z.object({ title: z.string(), cover: image(), author: reference(\"authors\") }) });\nexport const collections = { docs };\n",
    )
    .unwrap();
}

#[cfg(unix)]
#[test]
fn generate_applies_astro_helper_resolution_end_to_end() {
    let _guard = SPAWN_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let node = install_fake_project(temp.path(), HELPERS_SCHEMA);
    write_helpers_config(temp.path());
    let request =
        SchemaGenerationRequest::new(temp.path(), Cancellation::default()).with_node(node);

    let report = generate_collection_schemas(&request).expect("generation succeeds");
    assert_eq!(report.schemas.len(), 1);
    let schema = &report.schemas[0].schema;

    let cover = schema
        .fields
        .iter()
        .find(|field| field.name == "cover")
        .expect("cover field");
    assert_eq!(cover.kind, FieldKind::Image, "image() must refine to Image");

    let author = schema
        .fields
        .iter()
        .find(|field| field.name == "author")
        .expect("author field");
    assert_eq!(
        author.kind,
        FieldKind::Reference {
            collections: vec!["authors".to_owned()]
        },
        "reference() must refine to Reference with its collection"
    );

    assert!(
        !report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.contains("skipped")),
        "unexpected helper diagnostics: {:?}",
        report.diagnostics
    );
}

#[cfg(unix)]
#[test]
fn malformed_content_config_is_non_fatal_and_keeps_schemas() {
    let _guard = SPAWN_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let node = install_fake_project(temp.path(), HELPERS_SCHEMA);
    fs::create_dir_all(temp.path().join("src")).unwrap();
    fs::write(
        temp.path().join("src/content.config.ts"),
        "const docs = defineCollection({ schema: z.object({ cover: image( })",
    )
    .unwrap();
    let request =
        SchemaGenerationRequest::new(temp.path(), Cancellation::default()).with_node(node);

    let report = generate_collection_schemas(&request).expect("schemas still load");
    assert_eq!(report.schemas.len(), 1);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.contains("invalid TypeScript syntax")),
        "expected a helper parse diagnostic: {:?}",
        report.diagnostics
    );
    // Never silently coerce: the cover stays a string when the config cannot be
    // interpreted to prove it is an image.
    let cover = report.schemas[0]
        .schema
        .fields
        .iter()
        .find(|field| field.name == "cover")
        .expect("cover field");
    assert_eq!(cover.kind, FieldKind::String { format: None });
}

#[cfg(unix)]
#[test]
fn missing_content_config_is_a_non_fatal_helper_diagnostic() {
    let _guard = SPAWN_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let node = install_fake_project(temp.path(), VALID_DOCS);
    let request =
        SchemaGenerationRequest::new(temp.path(), Cancellation::default()).with_node(node);

    let report = generate_collection_schemas(&request).expect("schemas still load");
    assert_eq!(report.schemas.len(), 1);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.contains("content config not found")),
        "expected an absent-config diagnostic: {:?}",
        report.diagnostics
    );
}
