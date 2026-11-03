//! Unix integration tests for the dev-server lifecycle and process-tree ownership.
//!
//! These use small `#!/bin/sh` fixtures that honour the same argv as the Astro
//! CLI (`dev --host 127.0.0.1 --port N`) and read `ASTROLITH_PREVIEW_PORT`.
//! Everything for one scenario runs inside a single runtime so the drain tasks
//! and process reaper stay alive across start/stop/wait.

use std::{fs, path::Path, time::Duration};

use astrolith_core::ProjectId;
use tempfile::TempDir;

use crate::{
    process::OUTPUT_CAP,
    server::{DevServer, DevServerConfig, DevServerError, ServerStatus},
    test_support::{block_on, write_script},
};

/// Parses `--port <n>` into `$PORT` for the fixtures.
const PRELUDE: &str = r#"PORT=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --port) shift; PORT="$1" ;;
  esac
  shift
done
"#;

/// Emits a readiness line in the shape Astro prints once listening.
const READY: &str = r#"printf '  ┃ Local    http://127.0.0.1:%s/\n' "$PORT""#;

fn project() -> ProjectId {
    ProjectId::new("test-project").unwrap()
}

fn direct_server(program: &Path, root: &Path) -> DevServer {
    let config = DevServerConfig::new(root)
        .with_script_runner("/bin/sh", program.to_path_buf())
        .with_startup_timeout(Duration::from_secs(5))
        .with_stop_grace(Duration::from_secs(2));
    DevServer::new(project(), config)
}

fn process_alive(pid: i32) -> bool {
    use nix::{
        sys::signal::{Signal, kill},
        unistd::Pid,
    };
    kill(Pid::from_raw(pid), None::<Signal>).is_ok()
}

#[test]
fn startup_reaches_ready_then_stops() {
    block_on(async {
        let dir = TempDir::new().unwrap();
        let body = format!("{PRELUDE}\n{READY}\nwhile true; do sleep 0.1; done\n");
        let script = write_script(dir.path(), "astro.sh", &body);
        let mut server = direct_server(&script, dir.path());

        assert_eq!(server.status(), &ServerStatus::Stopped);
        let url = server.start().await.expect("server should become ready");

        assert_eq!(server.status().label(), "ready");
        assert_eq!(url.scheme(), "http");
        assert_eq!(url.host_str(), Some("127.0.0.1"));
        assert!(url.port().unwrap() > 0);
        assert!(server.pid().is_some());

        server.stop().await.expect("graceful stop");
        assert_eq!(server.status(), &ServerStatus::Stopped);
        assert!(server.pid().is_none());
    });
}

#[test]
fn resolves_project_local_astro_entry() {
    block_on(async {
        let dir = TempDir::new().unwrap();
        let astro = dir.path().join("node_modules/astro");
        fs::create_dir_all(&astro).unwrap();
        fs::write(
            astro.join("package.json"),
            r#"{"name":"astro","version":"5.18.2","bin":"cli.sh"}"#,
        )
        .unwrap();
        let body = format!("{PRELUDE}\n{READY}\nwhile true; do sleep 0.1; done\n");
        // The resolved entry is run through `/bin/sh`, so a shell fixture stands
        // in for the Astro CLI while the real node_modules resolution runs.
        write_script(&astro, "cli.sh", &body);

        let config = DevServerConfig::new(dir.path())
            .with_node("/bin/sh")
            .with_startup_timeout(Duration::from_secs(5))
            .with_stop_grace(Duration::from_secs(2));
        let mut server = DevServer::new(project(), config);

        let url = server
            .start()
            .await
            .expect("resolved Astro entry should start");
        assert!(url.port().unwrap() > 0);
        server.stop().await.unwrap();
    });
}

#[test]
fn missing_install_transitions_to_failed() {
    block_on(async {
        let dir = TempDir::new().unwrap();
        let mut server = DevServer::new(
            project(),
            DevServerConfig::new(dir.path()).with_startup_timeout(Duration::from_secs(1)),
        );

        let error = server.start().await.expect_err("missing install must fail");
        assert!(matches!(error, DevServerError::NotInstalled(_)));
        match server.status() {
            ServerStatus::Failed(message) => assert!(message.contains("Astro")),
            other => panic!("expected Failed, got {other:?}"),
        }
    });
}

#[test]
fn bad_exit_transitions_to_failed_with_stderr() {
    block_on(async {
        let dir = TempDir::new().unwrap();
        let body = format!("{PRELUDE}\nprintf 'boom: cannot start\\n' >&2\nexit 1\n");
        let script = write_script(dir.path(), "astro.sh", &body);
        let mut server = direct_server(&script, dir.path());

        let error = server.start().await.expect_err("bad exit must fail");
        assert!(matches!(error, DevServerError::Exited(_)), "got {error:?}");
        match server.status() {
            ServerStatus::Failed(message) => assert!(message.contains("boom")),
            other => panic!("expected Failed, got {other:?}"),
        }
        let failure = server.failure().expect("a failed start retains its detail");
        assert!(
            failure
                .stderr_excerpt()
                .is_some_and(|stderr| stderr.contains("boom: cannot start")),
            "stderr excerpt was {:?}",
            failure.stderr_excerpt()
        );
    });
}

#[test]
fn occupied_port_reports_recoverable_failure() {
    block_on(async {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let occupied = listener.local_addr().unwrap().port();

        let dir = TempDir::new().unwrap();
        let body =
            format!("{PRELUDE}\nprintf 'Error: Port %s is in use\\n' \"$PORT\" >&2\nexit 1\n");
        let script = write_script(dir.path(), "astro.sh", &body);
        let config = DevServerConfig::new(dir.path())
            .with_script_runner("/bin/sh", &script)
            .with_port(occupied)
            .with_startup_timeout(Duration::from_secs(2));
        let mut server = DevServer::new(project(), config);

        let error = server.start().await.expect_err("occupied port must fail");
        assert!(
            matches!(error, DevServerError::PortInUse(port) if port == occupied),
            "got {error:?}"
        );
        match server.status() {
            ServerStatus::Failed(message) => assert!(message.contains("in use")),
            other => panic!("expected Failed, got {other:?}"),
        }
        // The editor remains usable; the port holder is untouched.
        assert_eq!(listener.local_addr().unwrap().port(), occupied);
    });
}

#[test]
fn auto_incremented_port_warning_is_not_a_failure() {
    block_on(async {
        let dir = TempDir::new().unwrap();
        // Astro warns, then binds the next port a moment later. The warning alone
        // must not terminate a server that is still starting.
        let body = format!(
            "{PRELUDE}\nprintf 'Port %s is in use, trying another one...\\n' \"$PORT\" >&2\nsleep 0.2\nprintf '  ┃ Local    http://127.0.0.1:45679/\\n'\nwhile true; do sleep 0.1; done\n"
        );
        let script = write_script(dir.path(), "astro.sh", &body);
        let mut server = direct_server(&script, dir.path());

        let url = server
            .start()
            .await
            .expect("an auto-incremented port must still start");
        assert_eq!(url.port(), Some(45679));
        server.stop().await.expect("stop");
    });
}

#[test]
fn astro7_starts_foreground_with_ignore_lock() {
    block_on(async {
        let dir = TempDir::new().unwrap();
        let astro = dir.path().join("node_modules/astro");
        fs::create_dir_all(&astro).unwrap();
        fs::write(
            astro.join("package.json"),
            r#"{"name":"astro","version":"7.4.4","bin":"cli.sh"}"#,
        )
        .unwrap();
        let body =
            format!("printf '%s\\n' \"$@\" > args.txt\n{READY}\nwhile true; do sleep 0.1; done\n");
        write_script(&astro, "cli.sh", &body);

        let config = DevServerConfig::new(dir.path())
            .with_node("/bin/sh")
            .with_startup_timeout(Duration::from_secs(5))
            .with_stop_grace(Duration::from_secs(1));
        let mut server = DevServer::new(project(), config);
        server.start().await.expect("Astro 7 fixture becomes ready");
        server.stop().await.expect("stop");

        let args = fs::read_to_string(dir.path().join("args.txt")).unwrap();
        assert!(
            args.lines().any(|line| line == "--ignore-lock"),
            "Astro 7 must run in the foreground: {args:?}"
        );
    });
}

#[test]
fn astro5_has_no_astro7_only_flag() {
    block_on(async {
        let dir = TempDir::new().unwrap();
        let astro = dir.path().join("node_modules/astro");
        fs::create_dir_all(&astro).unwrap();
        fs::write(
            astro.join("package.json"),
            r#"{"name":"astro","version":"5.18.2","bin":"cli.sh"}"#,
        )
        .unwrap();
        let body =
            format!("printf '%s\\n' \"$@\" > args.txt\n{READY}\nwhile true; do sleep 0.1; done\n");
        write_script(&astro, "cli.sh", &body);

        let config = DevServerConfig::new(dir.path())
            .with_node("/bin/sh")
            .with_startup_timeout(Duration::from_secs(5))
            .with_stop_grace(Duration::from_secs(1));
        let mut server = DevServer::new(project(), config);
        server.start().await.expect("Astro 5 fixture becomes ready");
        server.stop().await.expect("stop");

        let args = fs::read_to_string(dir.path().join("args.txt")).unwrap();
        assert!(
            !args.lines().any(|line| line == "--ignore-lock"),
            "Astro 5 must not receive an unknown flag: {args:?}"
        );
    });
}

#[test]
fn startup_timeout_transitions_to_failed() {
    block_on(async {
        let dir = TempDir::new().unwrap();
        let body = format!("{PRELUDE}\nsleep 30\n");
        let script = write_script(dir.path(), "astro.sh", &body);
        let config = DevServerConfig::new(dir.path())
            .with_script_runner("/bin/sh", &script)
            .with_startup_timeout(Duration::from_millis(300));
        let mut server = DevServer::new(project(), config);

        let error = server.start().await.expect_err("timeout must fail");
        assert!(matches!(error, DevServerError::Timeout(_)), "got {error:?}");
        assert!(matches!(server.status(), ServerStatus::Failed(_)));
    });
}

#[test]
fn cancellation_during_startup_stops_and_reaps_child() {
    block_on(async {
        let dir = TempDir::new().unwrap();
        let pidfile = dir.path().join("child.pid");
        let body = format!("{PRELUDE}\necho $$ > \"{}\"\nsleep 30\n", pidfile.display());
        let script = write_script(dir.path(), "astro.sh", &body);
        let mut server = direct_server(&script, dir.path());

        let cancellation = astrolith_astro::Cancellation::default();
        let trigger = cancellation.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            trigger.cancel();
        });

        let error = server
            .start_cancellable(&cancellation)
            .await
            .expect_err("cancellation must abort startup");
        assert!(matches!(error, DevServerError::Cancelled), "got {error:?}");
        assert_eq!(server.status(), &ServerStatus::Stopped);
        assert!(server.pid().is_none());

        let pid: i32 = fs::read_to_string(&pidfile)
            .expect("child writes its pid")
            .trim()
            .parse()
            .unwrap();
        assert!(
            !process_alive(pid),
            "cancelled child {pid} must be terminated and reaped"
        );
    });
}

#[test]
fn process_exit_is_observed() {
    block_on(async {
        let dir = TempDir::new().unwrap();
        let body = format!("{PRELUDE}\n{READY}\nsleep 0.8\nexit 0\n");
        let script = write_script(dir.path(), "astro.sh", &body);
        let mut server = direct_server(&script, dir.path());

        server.start().await.expect("becomes ready");
        let status = server.wait_for_exit().await.expect("wait for exit");
        assert_eq!(status, ServerStatus::Stopped);
        assert!(server.pid().is_none());
    });
}

#[test]
fn grandchild_is_killed_with_the_process_group() {
    block_on(async {
        let dir = TempDir::new().unwrap();
        let body = format!(
            "{PRELUDE}\nsleep 300 &\necho \"GRANDCHILD:$!\"\n{READY}\nwhile true; do sleep 0.1; done\n"
        );
        let script = write_script(dir.path(), "astro.sh", &body);
        let mut server = direct_server(&script, dir.path());

        server.start().await.expect("becomes ready");

        let mut grandchild = None;
        for _ in 0..80 {
            if let Some(pid) = server.stdout_text().lines().find_map(|line| {
                line.trim()
                    .strip_prefix("GRANDCHILD:")
                    .and_then(|value| value.trim().parse::<i32>().ok())
            }) {
                grandchild = Some(pid);
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let grandchild = grandchild.expect("fixture prints grandchild pid");
        assert!(
            process_alive(grandchild),
            "grandchild should be running before stop"
        );

        server.stop().await.expect("stop");

        for _ in 0..120 {
            if !process_alive(grandchild) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(
            !process_alive(grandchild),
            "grandchild {grandchild} must not survive the process group kill"
        );
    });
}

#[test]
fn output_flooding_stays_under_cap_and_never_deadlocks() {
    block_on(async {
        let dir = TempDir::new().unwrap();
        // ~4 MiB of stdout, four times the per-stream cap.
        let body = format!(
            "{PRELUDE}\n{READY}\ndd if=/dev/zero bs=65536 count=64 2>/dev/null | tr '\\0' 'a'\nsleep 2\n"
        );
        let script = write_script(dir.path(), "astro.sh", &body);
        let mut server = direct_server(&script, dir.path());

        server.start().await.expect("becomes ready while flooding");

        let target = 4 * 1024 * 1024;
        let mut all_drained = false;
        for _ in 0..400 {
            if let Some(capture) = server.stdout_capture()
                && capture.total_received() >= target
            {
                all_drained = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(all_drained, "the drain must consume every flooded byte");

        let capture = server.stdout_capture().expect("still running");
        assert_eq!(
            capture.len(),
            OUTPUT_CAP,
            "retained bytes are capped at 1 MiB"
        );
        assert!(capture.is_truncated(), "overflow is flagged");
        assert!(capture.total_received() >= target);
        assert!(!server.stdout_text().is_empty());

        server.stop().await.expect("stop after flood");
    });
}
