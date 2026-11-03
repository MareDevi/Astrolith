//! Opening the active entry in an already-running external preview.
//!
//! A [`PreviewSession`] never starts or stops a server: it only adds an entry
//! route to a base URL that is already [`ServerStatus::Ready`] and hands that
//! URL to a [`PreviewSurface`]. Reopening an entry therefore reuses the running
//! server instead of spawning another Astro process.

use std::sync::Arc;

use astrolith_core::EntryId;
use url::Url;

use crate::{
    config::RouteMapping,
    routes::{RouteError, resolve_entry_url},
    server::ServerStatus,
    surface::{PreviewSurface, SurfaceError},
};

/// Failure while opening a preview for an entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreviewError {
    /// The dev server is not ready, so there is no base URL to open.
    ServerNotReady {
        /// Current status label (e.g. `starting`, `stopped`).
        status: String,
    },
    /// The entry has no route mapping.
    Route(RouteError),
    /// The system browser could not be launched.
    Surface(SurfaceError),
}

impl std::fmt::Display for PreviewError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ServerNotReady { status } => {
                write!(formatter, "preview server is {status}, not ready")
            }
            Self::Route(error) => write!(formatter, "{error}"),
            Self::Surface(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for PreviewError {}

/// Opens entry URLs against an existing server through one surface.
pub struct PreviewSession {
    mapping: RouteMapping,
    surface: Arc<dyn PreviewSurface>,
}

impl std::fmt::Debug for PreviewSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("PreviewSession")
    }
}

impl PreviewSession {
    /// Create a session with `mapping` and `surface`.
    pub fn new(mapping: RouteMapping, surface: Arc<dyn PreviewSurface>) -> Self {
        Self { mapping, surface }
    }

    /// Replace the route mapping (e.g. after the project's collections change).
    pub fn set_mapping(&mut self, mapping: RouteMapping) {
        self.mapping = mapping;
    }

    /// The current mapping.
    pub fn mapping(&self) -> &RouteMapping {
        &self.mapping
    }

    /// Resolve `entry` against the ready server without presenting it.
    ///
    /// Copy-preview and the open path share this resolution so a copied URL is
    /// exactly the URL the browser receives.
    pub fn url_for_entry(
        &self,
        entry: &EntryId,
        status: &ServerStatus,
    ) -> Result<Url, PreviewError> {
        let base = status.url().ok_or_else(|| PreviewError::ServerNotReady {
            status: status.label().to_owned(),
        })?;
        resolve_entry_url(entry, &self.mapping, base).map_err(PreviewError::Route)
    }

    /// Resolve `entry` against the ready server and hand the URL to the surface.
    ///
    /// Returns the opened URL so a caller can display or re-copy it.
    pub fn open_entry(&self, entry: &EntryId, status: &ServerStatus) -> Result<Url, PreviewError> {
        let url = self.url_for_entry(entry, status)?;
        self.surface.open(&url).map_err(PreviewError::Surface)?;
        Ok(url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::ServerStatus;

    fn ready() -> ServerStatus {
        ServerStatus::Ready(Url::parse("http://127.0.0.1:4321/").unwrap())
    }

    fn entry() -> EntryId {
        EntryId::new("docs", "guides/nested").unwrap()
    }

    fn session() -> (PreviewSession, Arc<crate::surface::RecordingSurface>) {
        let surface = Arc::new(crate::surface::RecordingSurface::new());
        let mapping = RouteMapping::new().with_collection("docs", "docs");
        (PreviewSession::new(mapping, surface.clone()), surface)
    }

    #[test]
    fn opens_the_entry_route_on_the_ready_server() {
        let (session, surface) = session();
        let url = session.open_entry(&entry(), &ready()).unwrap();
        assert_eq!(url.path(), "/docs/guides/nested");
        assert_eq!(surface.opened(), vec![url.to_string()]);
    }

    #[test]
    fn resolves_a_url_without_opening_it() {
        let (session, surface) = session();
        let url = session.url_for_entry(&entry(), &ready()).unwrap();
        assert_eq!(url.path(), "/docs/guides/nested");
        assert!(
            surface.opened().is_empty(),
            "resolving a URL must not launch a browser"
        );
    }

    #[test]
    fn reuses_the_running_server_on_each_open() {
        let (session, surface) = session();
        session.open_entry(&entry(), &ready()).unwrap();
        session.open_entry(&entry(), &ready()).unwrap();
        assert_eq!(surface.opened().len(), 2);
        assert!(
            surface
                .opened()
                .iter()
                .all(|url| url.starts_with("http://127.0.0.1:4321/")),
            "every open must reuse the same server base URL"
        );
    }

    #[test]
    fn refuses_to_open_before_the_server_is_ready() {
        let (session, surface) = session();
        for status in [
            ServerStatus::Stopped,
            ServerStatus::Starting,
            ServerStatus::Stopping,
            ServerStatus::Failed("boom".into()),
        ] {
            let error = session.open_entry(&entry(), &status).unwrap_err();
            assert!(matches!(error, PreviewError::ServerNotReady { .. }));
        }
        assert!(surface.opened().is_empty());
    }

    #[test]
    fn unmapped_entry_is_a_route_error_and_opens_nothing() {
        let (session, surface) = session();
        let other = EntryId::new("notes", "x").unwrap();
        assert!(matches!(
            session.open_entry(&other, &ready()),
            Err(PreviewError::Route(RouteError::UnmappedEntry { .. }))
        ));
        assert!(surface.opened().is_empty());
    }

    #[test]
    fn surface_failure_is_propagated() {
        let surface = Arc::new(crate::surface::RecordingSurface::failing(
            SurfaceError::LaunchFailed {
                command: "xdg-open".into(),
                message: "missing".into(),
            },
        ));
        let mapping = RouteMapping::new().with_collection("docs", "docs");
        let session = PreviewSession::new(mapping, surface);
        assert!(matches!(
            session.open_entry(&entry(), &ready()),
            Err(PreviewError::Surface(SurfaceError::LaunchFailed { .. }))
        ));
    }

    #[test]
    fn mapping_can_be_replaced_for_a_new_project() {
        let (mut session, surface) = session();
        session.set_mapping(RouteMapping::new().with_collection("docs", "reference"));
        let url = session.open_entry(&entry(), &ready()).unwrap();
        assert_eq!(url.path(), "/reference/guides/nested");
        assert_eq!(surface.opened().len(), 1);
    }
}
