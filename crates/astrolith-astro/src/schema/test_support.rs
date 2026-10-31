//! Shared helpers for crate unit tests (compiled only under `cfg(test)`).

/// Serializes tests that write an executable script and then spawn it, to avoid
/// the Linux `ETXTBSY` race when concurrent tests fork while a script is open.
#[cfg(unix)]
pub(crate) static SPAWN_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
