use super::*;
use crate::discovery::Cancellation;
use crate::schema::test_support::SPAWN_LOCK;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// Install a fake Astro package plus a fake `node` executable script.
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
fn successful_run_passes_and_attaches_the_requested_revision() {
    let _guard = SPAWN_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let node = install_fake_project(temp.path(), "#!/bin/sh\nexit 0\n");
    let request =
        AstroValidationRequest::new(temp.path(), 7, Cancellation::default()).with_node(node);

    let report = validate_with_astro_tooling(&request).expect("tooling runs");

    assert_eq!(report.revision(), 7);
    assert_eq!(report.status(), AstroValidationStatus::Passed);
    assert!(report.passed());
    assert!(report.diagnostics().is_empty());
    assert!(report.is_current_for(7));
    assert!(!report.is_current_for(8));
    assert!(report.tool_status().is_some());
}

#[cfg(unix)]
#[test]
fn failed_run_maps_structured_errors_with_a_relative_location() {
    let _guard = SPAWN_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let node = install_fake_project(
        temp.path(),
        r#"#!/bin/sh
echo '[InvalidContentEntryDataError] docs -> slug data does not match collection schema.' >&2
echo '  Location:' >&2
echo "    $PWD/content/docs/intro.md:1:2" >&2
exit 1
"#,
    );
    let request =
        AstroValidationRequest::new(temp.path(), 3, Cancellation::default()).with_node(node);

    let report = validate_with_astro_tooling(&request).expect("tooling runs");

    assert_eq!(report.status(), AstroValidationStatus::Failed);
    assert_eq!(report.revision(), 3);
    assert_eq!(report.diagnostics().len(), 1);
    let diagnostic = &report.diagnostics()[0];
    assert_eq!(diagnostic.source, ValidationSource::AstroTooling);
    assert_eq!(diagnostic.severity, ValidationSeverity::Error);
    assert_eq!(diagnostic.path, "content/docs/intro.md");
    assert!(diagnostic.message.contains("InvalidContentEntryDataError"));
}

#[cfg(unix)]
#[test]
fn failed_run_without_structured_errors_still_reports_the_exit() {
    let _guard = SPAWN_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let node = install_fake_project(
        temp.path(),
        "#!/bin/sh\necho 'boom: content could not load' >&2\nexit 2\n",
    );
    let request =
        AstroValidationRequest::new(temp.path(), 1, Cancellation::default()).with_node(node);

    let report = validate_with_astro_tooling(&request).expect("tooling runs");

    assert_eq!(report.status(), AstroValidationStatus::Failed);
    assert_eq!(report.diagnostics().len(), 1);
    assert!(
        report.diagnostics()[0]
            .message
            .contains("astro sync exited with")
    );
}

#[test]
fn missing_installation_is_an_unavailable_report_not_a_hard_error() {
    let temp = tempfile::tempdir().unwrap();
    let request = AstroValidationRequest::new(temp.path(), 5, Cancellation::default());

    let report = validate_with_astro_tooling(&request).expect("unavailable is not an error");

    assert_eq!(report.status(), AstroValidationStatus::Unavailable);
    assert_eq!(report.revision(), 5);
    assert_eq!(report.diagnostics().len(), 1);
    assert_eq!(
        report.diagnostics()[0].source,
        ValidationSource::AstroTooling
    );
    assert!(report.diagnostics()[0].message.contains("unavailable"));
}

#[cfg(unix)]
#[test]
fn timeout_is_returned_as_a_tooling_error() {
    let _guard = SPAWN_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let node = install_fake_project(temp.path(), "#!/bin/sh\nsleep 5\n");
    let request = AstroValidationRequest::new(temp.path(), 1, Cancellation::default())
        .with_node(node)
        .with_timeout(Duration::from_millis(200));

    let started = Instant::now();
    let error = validate_with_astro_tooling(&request).expect_err("timeout fails");
    assert!(error.to_string().contains("timed out"));
    assert!(started.elapsed() < Duration::from_secs(4));
}

#[cfg(unix)]
#[test]
fn cancellation_is_returned_as_a_tooling_error() {
    let _guard = SPAWN_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let node = install_fake_project(temp.path(), "#!/bin/sh\nsleep 5\n");
    let cancellation = Cancellation::default();
    let trigger = cancellation.clone();
    let handle = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        trigger.cancel();
    });
    let request = AstroValidationRequest::new(temp.path(), 1, cancellation)
        .with_node(node)
        .with_timeout(Duration::from_secs(10));

    let error = validate_with_astro_tooling(&request).expect_err("cancellation fails");
    assert!(error.to_string().contains("cancelled"));
    handle.join().unwrap();
}

#[test]
fn stale_reports_are_rejected() {
    let report = AstroValidationReport {
        revision: 4,
        status: AstroValidationStatus::Passed,
        diagnostics: Vec::new(),
        tool_status: Some("exit status: 0".to_owned()),
    };

    assert!(report.reject_if_stale(4).is_ok());
    let error = report
        .reject_if_stale(5)
        .expect_err("newer revision rejects");
    assert_eq!(
        error,
        ValidationError::Stale {
            result_revision: 4,
            current_revision: 5,
        }
    );
    assert!(accept_for_revision(report.clone(), 4).is_ok());
    assert!(accept_for_revision(report, 5).is_err());
}

#[cfg(unix)]
#[test]
fn async_wrapper_runs_validation_on_the_blocking_lane() {
    let _guard = SPAWN_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let node = install_fake_project(temp.path(), "#!/bin/sh\nexit 0\n");
    let request =
        AstroValidationRequest::new(temp.path(), 9, Cancellation::default()).with_node(node);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    let report = runtime
        .block_on(validate_with_astro_tooling_async(request))
        .expect("async validation runs");

    assert_eq!(report.revision(), 9);
    assert!(report.passed());
}
