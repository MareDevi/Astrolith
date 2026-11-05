//! The application's HTTP transport, installed once at startup.
//!
//! GPUI's default client refuses every request, so any document image with an
//! absolute `http`/`https` URL fails to load in the reading pane. This adapter
//! serves that requirement through the pinned `ureq` stack: GPUI asks for a
//! resource, the request runs on a service blocking thread, and the complete
//! response is handed back to the asset cache. Painting never performs I/O.

use std::{sync::Arc, time::Duration};

use futures::{AsyncReadExt as _, future::BoxFuture};
use gpui_kit::http_client::{
    AsyncBody, HttpClient, RedirectPolicy, Request, RequestTimeout, Response, Url,
    http::{HeaderValue, header::USER_AGENT},
};
use gpui_kit::{App, AppContext};
use tokio::runtime::Handle;

use crate::runtime::ServiceRuntime;

/// Total request deadline used when GPUI states no preference.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// Redirects followed for a request that allows them.
const FOLLOW_ALL_REDIRECTS: u32 = 10;

/// A blocking `ureq` request driven from the service runtime.
///
/// `send` resolves its response body completely before publishing it, which is
/// what GPUI's image loader consumes anyway; streaming would only add a second
/// buffering layer.
pub(crate) struct UreqHttpClient {
    agent: ureq::Agent,
    user_agent: HeaderValue,
    handle: Handle,
}

impl UreqHttpClient {
    fn new(handle: Handle) -> Self {
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(DEFAULT_TIMEOUT))
            .build()
            .into();
        Self {
            agent,
            user_agent: HeaderValue::from_static(concat!("Astrolith/", env!("CARGO_PKG_VERSION"))),
            handle,
        }
    }
}

/// Install the real client for the process.
///
/// Called once the service runtime exists, before any window renders a remote
/// image. The platform default is a client that rejects every request.
pub(crate) fn install(cx: &mut App) {
    let handle = cx.read_global(|runtime: &ServiceRuntime, _| runtime.handle().clone());
    cx.set_http_client(Arc::new(UreqHttpClient::new(handle)));
}

impl HttpClient for UreqHttpClient {
    fn send(
        &self,
        request: Request<AsyncBody>,
    ) -> BoxFuture<'static, anyhow::Result<Response<AsyncBody>>> {
        let agent = self.agent.clone();
        let user_agent = self.user_agent.clone();
        let handle = self.handle.clone();
        Box::pin(async move {
            let (mut parts, body) = request.into_parts();
            let timeout = parts
                .extensions
                .remove::<RequestTimeout>()
                .map_or(DEFAULT_TIMEOUT, |timeout| timeout.0);
            let redirects = match parts.extensions.remove::<RedirectPolicy>() {
                Some(RedirectPolicy::FollowAll) => FOLLOW_ALL_REDIRECTS,
                Some(RedirectPolicy::FollowLimit(limit)) => limit,
                Some(RedirectPolicy::NoFollow) | None => 0,
            };
            if !parts.headers.contains_key(USER_AGENT) {
                parts.headers.insert(USER_AGENT, user_agent);
            }
            let body = read_body(body).await?;
            let host = parts.uri.host().unwrap_or_default().to_owned();
            let request = Request::from_parts(parts, body);
            let join = handle.spawn_blocking(move || {
                let request = agent
                    .configure_request(request)
                    .max_redirects(redirects)
                    .timeout_global(Some(timeout))
                    .build();
                let response = agent.run(request)?;
                let (parts, body) = response.into_parts();
                let mut bytes = Vec::new();
                std::io::Read::read_to_end(&mut body.into_reader(), &mut bytes)?;
                Ok::<_, anyhow::Error>((parts, bytes))
            });
            let (parts, bytes) = join.await.map_err(anyhow::Error::from)??;
            tracing::debug!(
                host = %host,
                bytes = bytes.len(),
                "loaded a remote resource"
            );
            Ok(Response::from_parts(parts, AsyncBody::from(bytes)))
        })
    }

    fn user_agent(&self) -> Option<&HeaderValue> {
        Some(&self.user_agent)
    }

    fn proxy(&self) -> Option<&Url> {
        None
    }
}

/// GPUI may supply a streaming request body; `ureq` needs it upfront.
async fn read_body(mut body: AsyncBody) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    body.read_to_end(&mut bytes).await?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read as _, Write as _},
        net::TcpListener,
        thread,
    };

    use super::*;
    use gpui_kit::http_client::http;

    /// Answer one HTTP request with `body` and the given content type.
    fn serve_once(
        content_type: &'static str,
        body: Vec<u8>,
        path: &'static str,
    ) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a test server");
        let address = listener.local_addr().expect("a bound address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept one request");
            let mut request = [0u8; 1024];
            let read = stream.read(&mut request).expect("read the request line");
            assert!(String::from_utf8_lossy(&request[..read]).starts_with(&format!("GET {path}")));
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .expect("write response headers");
            stream.write_all(&body).expect("write the response body");
        });
        (format!("http://{address}{path}"), server)
    }

    fn client() -> (tokio::runtime::Runtime, UreqHttpClient) {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("start a test runtime");
        let client = UreqHttpClient::new(runtime.handle().clone());
        (runtime, client)
    }

    #[test]
    fn send_returns_the_remote_body_and_content_type() {
        let png: &[u8] = b"\x89PNG\r\n\x1a\npayload";
        let (url, server) = serve_once("image/png", png.to_vec(), "/image.png");
        let (runtime, client) = client();
        let response = runtime
            .block_on(client.get(&url, AsyncBody::empty(), true))
            .expect("the request succeeds");
        assert_eq!(response.status(), 200);
        assert_eq!(
            response
                .headers()
                .get(http::header::CONTENT_TYPE)
                .expect("a content type"),
            "image/png"
        );
        let body = runtime
            .block_on(async {
                let mut body = response.into_body();
                let mut bytes = Vec::new();
                body.read_to_end(&mut bytes).await.map(|_| bytes)
            })
            .expect("read the response body");
        assert_eq!(body, png);
        server.join().expect("the test server exits");
    }

    #[test]
    fn send_rejects_an_unreachable_server() {
        let (runtime, client) = client();
        let error = runtime
            .block_on(client.get("http://127.0.0.1:1/image.png", AsyncBody::empty(), true))
            .err()
            .expect("a refused connection is an error");
        assert!(!error.to_string().is_empty());
    }
}
