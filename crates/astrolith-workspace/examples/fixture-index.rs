use astrolith_astro::{Cancellation, DiscoveryOutcome, DiscoveryRequest, inspect_project};
use astrolith_core::ProjectId;
use astrolith_workspace::scan_collections;
use serde_json::json;
use std::{env, path::PathBuf, process::ExitCode};

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let root = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: fixture-index <astro-project-root>")?;
    let cancellation = Cancellation::default();
    let request = DiscoveryRequest::new(ProjectId::new("fixture")?, 1, root, cancellation.clone());
    let DiscoveryOutcome::Ready(manifest) = inspect_project(&request)? else {
        return Err("fixture discovery was not authoritative".into());
    };
    let rows = scan_collections(&manifest, &cancellation)?
        .into_iter()
        .map(|entry| {
            json!({
                "collection": entry.entry().id().collection().as_str(),
                "id": entry.entry().id().id(),
                "filePath": entry.entry().source().as_str(),
            })
        })
        .collect::<Vec<_>>();
    println!("{}", serde_json::to_string(&rows)?);
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("fixture-index: {error}");
            ExitCode::FAILURE
        }
    }
}
