use astrolith_astro::{Cancellation, DiscoveryOutcome, DiscoveryRequest, inspect_project};
use astrolith_core::ProjectId;
use std::path::Path;

fn request(root: &Path) -> DiscoveryRequest {
    DiscoveryRequest::new(
        ProjectId::new("project").unwrap(),
        7,
        root.into(),
        Cancellation::default(),
    )
}
fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("node_modules/astro")).unwrap();
    std::fs::create_dir_all(dir.path().join("src/posts")).unwrap();
    std::fs::write(
        dir.path().join("package.json"),
        r#"{"devDependencies":{"astro":"^5"}}"#,
    )
    .unwrap();
    std::fs::write(
        dir.path().join("node_modules/astro/package.json"),
        r#"{"version":"5.18.2"}"#,
    )
    .unwrap();
    std::fs::write(dir.path().join("src/content.config.ts"), "import {defineCollection} from 'astro:content'; import {glob} from 'astro/loaders'; export const collections = {blog: defineCollection({loader: glob({base:'./src/posts',pattern:'**/*.md'})})};").unwrap();
    dir
}
#[test]
fn default_project_config_produces_canonical_collection_descriptors() {
    let dir = fixture();
    let DiscoveryOutcome::Ready(manifest) = inspect_project(&request(dir.path())).unwrap() else {
        panic!("supported fixture")
    };
    assert_eq!(manifest.root(), dir.path().canonicalize().unwrap());
    assert_eq!(manifest.generation(), 7);
    assert_eq!(manifest.collections()[0].id().as_str(), "blog");
    assert_eq!(
        manifest.collections()[0].base(),
        dir.path().join("src/posts").canonicalize().unwrap()
    );
}
#[test]
fn missing_legacy_dynamic_and_uninstalled_configs_never_publish_empty_collections() {
    let dir = fixture();
    std::fs::remove_file(dir.path().join("src/content.config.ts")).unwrap();
    assert!(matches!(
        inspect_project(&request(dir.path())).unwrap(),
        DiscoveryOutcome::Unsupported { .. }
    ));
    std::fs::create_dir_all(dir.path().join("src/content")).unwrap();
    std::fs::write(
        dir.path().join("src/content/config.ts"),
        "export const collections = {};",
    )
    .unwrap();
    assert!(matches!(
        inspect_project(&request(dir.path())).unwrap(),
        DiscoveryOutcome::Unsupported { .. }
    ));
    std::fs::remove_file(dir.path().join("node_modules/astro/package.json")).unwrap();
    assert!(matches!(
        inspect_project(&request(dir.path())).unwrap(),
        DiscoveryOutcome::Unsupported { .. }
    ));
}
#[test]
fn custom_src_dir_and_ambiguous_config_are_explicit() {
    let dir = fixture();
    std::fs::rename(dir.path().join("src"), dir.path().join("www")).unwrap();
    std::fs::write(
        dir.path().join("astro.config.mjs"),
        "import {defineConfig} from 'astro/config'; export default defineConfig({srcDir:'www'});",
    )
    .unwrap();
    std::fs::write(dir.path().join("www/content.config.ts"),"import {defineCollection} from 'astro:content'; import {glob} from 'astro/loaders'; export const collections = {blog:defineCollection({loader:glob({base:'www/posts',pattern:'**/*.md'})})};").unwrap();
    assert!(matches!(
        inspect_project(&request(dir.path())).unwrap(),
        DiscoveryOutcome::Ready(_)
    ));
    std::fs::write(dir.path().join("astro.config.ts"), "").unwrap();
    assert!(matches!(
        inspect_project(&request(dir.path())).unwrap(),
        DiscoveryOutcome::Unsupported { .. }
    ));
}
#[test]
fn invalid_metadata_conflicts_oversize_and_cancel_fail() {
    let dir = fixture();
    let req = request(dir.path());
    req.cancellation().cancel();
    assert!(inspect_project(&req).is_err());
    for json in [
        "[]",
        "not json",
        r#"{"dependencies":{"astro":"5"},"devDependencies":{"astro":"6"}}"#,
    ] {
        std::fs::write(dir.path().join("package.json"), json).unwrap();
        assert!(inspect_project(&request(dir.path())).is_err(), "{json}");
    }
    std::fs::write(dir.path().join("package.json"), " ".repeat(1024 * 1024 + 1)).unwrap();
    assert!(inspect_project(&request(dir.path())).is_err());
}
#[cfg(unix)]
#[test]
fn config_and_base_symlink_escape_are_rejected() {
    use std::os::unix::fs::symlink;
    let dir = fixture();
    let outside = tempfile::tempdir().unwrap();
    std::fs::remove_dir(dir.path().join("src/posts")).unwrap();
    symlink(outside.path(), dir.path().join("src/posts")).unwrap();
    assert!(matches!(
        inspect_project(&request(dir.path())).unwrap(),
        DiscoveryOutcome::Unsupported { .. }
    ));
    std::fs::write(
        outside.path().join("config.ts"),
        "export const collections = {};",
    )
    .unwrap();
    std::fs::remove_file(dir.path().join("src/content.config.ts")).unwrap();
    symlink(
        outside.path().join("config.ts"),
        dir.path().join("src/content.config.ts"),
    )
    .unwrap();
    assert!(inspect_project(&request(dir.path())).is_err());
}

#[test]
fn unverified_or_malformed_installed_versions_are_unsupported() {
    let dir = fixture();
    for json in [r#"{"version":"99.0.0"}"#, r#"{"version":42}"#, "not json"] {
        std::fs::write(dir.path().join("node_modules/astro/package.json"), json).unwrap();
        assert!(matches!(
            inspect_project(&request(dir.path())).unwrap(),
            DiscoveryOutcome::Unsupported { .. }
        ));
    }
}
#[cfg(unix)]
#[test]
fn special_metadata_files_are_rejected_before_opening() {
    let dir = fixture();
    std::fs::remove_file(dir.path().join("package.json")).unwrap();
    let _socket = std::os::unix::net::UnixListener::bind(dir.path().join("package.json")).unwrap();
    assert!(inspect_project(&request(dir.path())).is_err());
    std::fs::remove_file(dir.path().join("package.json")).unwrap();
    let path = dir.path().join("package.json");
    let result = std::process::Command::new("mkfifo")
        .arg(&path)
        .status()
        .unwrap();
    assert!(result.success());
    let (tx, rx) = std::sync::mpsc::channel();
    let root = dir.path().to_owned();
    std::thread::spawn(move || {
        let _ = tx.send(inspect_project(&request(&root)).is_err());
    });
    assert!(
        rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap(),
        "FIFO metadata must not block discovery"
    );
}

fn set_mdx_pattern(dir: &Path) {
    let config = dir.join("src/content.config.ts");
    let code = std::fs::read_to_string(&config)
        .unwrap()
        .replace("**/*.{md,mdx}", "**/*.mdx")
        .replace("**/*.md'", "**/*.mdx'");
    std::fs::write(config, code).unwrap();
}

fn set_mdx_integration(dir: &Path) {
    std::fs::write(dir.join("astro.config.mjs"),"import {defineConfig} from 'astro/config'; import mdx from '@astrojs/mdx'; export default defineConfig({integrations:[mdx()]});").unwrap();
}

fn write_mdx(dir: &Path, version: &str, peer: &str) {
    std::fs::create_dir_all(dir.join("node_modules/@astrojs/mdx")).unwrap();
    std::fs::write(
        dir.join("node_modules/@astrojs/mdx/package.json"),
        format!(r#"{{"version":"{version}","peerDependencies":{{"astro":"{peer}"}}}}"#),
    )
    .unwrap();
}

#[test]
fn mdx_patterns_require_configured_matching_mdx_integration() {
    let dir = fixture();
    set_mdx_pattern(dir.path());
    assert!(
        matches!(
            inspect_project(&request(dir.path())).unwrap(),
            DiscoveryOutcome::Unsupported { .. }
        ),
        "no MDX integration cannot produce authoritative MDX entries"
    );
    set_mdx_integration(dir.path());
    assert!(
        matches!(
            inspect_project(&request(dir.path())).unwrap(),
            DiscoveryOutcome::Unsupported { .. }
        ),
        "configured but missing MDX package"
    );
    std::fs::create_dir_all(dir.path().join("node_modules/@astrojs/mdx")).unwrap();
    for (version, peer) in [
        ("3.9.0", "^4.0.0"),
        ("5.0.0", "^6.0.0"),
        ("6.0.3", "^6.4.0"),
        ("8.1.2", "^7.2.10"),
    ] {
        write_mdx(dir.path(), version, peer);
        assert!(
            matches!(
                inspect_project(&request(dir.path())).unwrap(),
                DiscoveryOutcome::Unsupported { .. }
            ),
            "MDX {version} does not pair with Astro 5"
        );
    }
    for (version, peer) in [("4.3.14", "^5.0.0"), ("4.99.0", "^5.0.0")] {
        write_mdx(dir.path(), version, peer);
        assert!(
            matches!(
                inspect_project(&request(dir.path())).unwrap(),
                DiscoveryOutcome::Ready(_)
            ),
            "Astro 5 must accept MDX {version}"
        );
    }
    std::fs::write(
        dir.path().join("node_modules/@astrojs/mdx/package.json"),
        r#"{"version":"4.99.0"}"#,
    )
    .unwrap();
    assert!(
        matches!(
            inspect_project(&request(dir.path())).unwrap(),
            DiscoveryOutcome::Unsupported { .. }
        ),
        "an MDX package without a declared Astro peer cannot be verified"
    );
}

#[test]
fn supported_majors_accept_any_patch_release() {
    let dir = fixture();
    for version in [
        "5.0.0", "5.99.1", "6.0.0", "6.3.7", "6.12.3", "7.0.0", "7.8.9",
    ] {
        std::fs::write(
            dir.path().join("node_modules/astro/package.json"),
            format!(r#"{{"version":"{version}"}}"#),
        )
        .unwrap();
        assert!(
            matches!(
                inspect_project(&request(dir.path())).unwrap(),
                DiscoveryOutcome::Ready(_)
            ),
            "Astro {version} is in a supported major"
        );
    }
    for version in [
        "4.99.0",
        "8.0.0",
        "6.0.0-beta.1",
        "7.4.4-rc.0",
        "not-a-version",
        "",
    ] {
        std::fs::write(
            dir.path().join("node_modules/astro/package.json"),
            format!(r#"{{"version":"{version}"}}"#),
        )
        .unwrap();
        assert!(
            matches!(
                inspect_project(&request(dir.path())).unwrap(),
                DiscoveryOutcome::Unsupported { .. }
            ),
            "Astro {version:?} is outside the supported majors"
        );
    }
}

#[test]
fn mdx_peer_range_must_match_installed_astro() {
    let dir = fixture();
    set_mdx_pattern(dir.path());
    set_mdx_integration(dir.path());
    std::fs::create_dir_all(dir.path().join("node_modules/@astrojs/mdx")).unwrap();
    for (astro, accepted, rejected) in [
        (
            "5.18.2",
            [("4.3.14", "^5.0.0"), ("4.99.0", "^5.0.0")],
            [
                ("5.0.6", "^6.0.0"),
                ("6.0.3", "^6.4.0"),
                ("8.1.2", "^7.2.10"),
            ],
        ),
        (
            "6.3.7",
            [("5.0.6", "^6.0.0"), ("5.9.0", "^6.0.0")],
            [
                ("6.0.3", "^6.4.0"),
                ("4.3.14", "^5.0.0"),
                ("8.1.2", "^7.2.10"),
            ],
        ),
        (
            "6.4.8",
            [("5.0.6", "^6.0.0"), ("6.0.3", "^6.4.0")],
            [
                ("6.0.3", "^6.5.0"),
                ("4.3.14", "^5.0.0"),
                ("8.1.2", "^7.2.10"),
            ],
        ),
        (
            "7.4.4",
            [("7.0.8", "^7.0.0"), ("8.1.2", "^7.2.10")],
            [
                ("6.0.3", "^6.4.0"),
                ("4.3.14", "^5.0.0"),
                ("8.1.2", "^8.0.0"),
            ],
        ),
    ] {
        std::fs::write(
            dir.path().join("node_modules/astro/package.json"),
            format!(r#"{{"version":"{astro}"}}"#),
        )
        .unwrap();
        for (version, peer) in rejected {
            write_mdx(dir.path(), version, peer);
            assert!(
                matches!(
                    inspect_project(&request(dir.path())).unwrap(),
                    DiscoveryOutcome::Unsupported { .. }
                ),
                "Astro {astro} must reject MDX {version} ({peer})"
            );
        }
        for (version, peer) in accepted {
            write_mdx(dir.path(), version, peer);
            assert!(
                matches!(
                    inspect_project(&request(dir.path())).unwrap(),
                    DiscoveryOutcome::Ready(_)
                ),
                "Astro {astro} must accept MDX {version} ({peer})"
            );
        }
    }
}

#[test]
fn legacy_compatibility_flags_are_explicitly_unsupported() {
    let dir = fixture();
    for legacy in [
        "legacy:{collections:true}",
        "legacy:{collectionsBackwardsCompat:true}",
        "experimental:{legacy:true}",
    ] {
        std::fs::write(dir.path().join("astro.config.mjs"),format!("import {{defineConfig}} from 'astro/config'; export default defineConfig({{{legacy}}});")).unwrap();
        assert!(
            matches!(
                inspect_project(&request(dir.path())).unwrap(),
                DiscoveryOutcome::Unsupported { .. }
            ),
            "{legacy}"
        );
    }
}
