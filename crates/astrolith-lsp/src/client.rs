//! An asynchronous LSP client speaking JSON-RPC over a language server's stdio.
//!
//! One [`Client`] owns one server process. Commands arrive on an unbounded
//! channel from the UI; decoded server messages leave on a separate event
//! channel, so a slow renderer never blocks the protocol loop. Requests from
//! providers await a one-shot reply; notifications (diagnostics, logs) are
//! pushed as events.

use std::{
    collections::HashMap,
    ffi::OsString,
    path::PathBuf,
    process::Stdio,
    sync::atomic::{AtomicI32, Ordering},
};

use lsp_types::{Diagnostic, InitializeResult, PublishDiagnosticsParams, ServerCapabilities, Uri};
use serde_json::{Value, json};
use std::str::FromStr;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, BufWriter},
    process::{Child, Command as ProcessCommand},
    sync::{mpsc, oneshot},
    task::AbortHandle,
};

use crate::{
    discovery::ServerSpec,
    framing::{self, FrameReader},
    language::LanguageId,
    position::{offset_to_position, position_to_offset},
};

#[derive(Debug)]
pub enum ClientError {
    /// The client task or its command channel is gone.
    Closed,
    /// The server process or its pipes failed.
    Io(String),
    /// The stream carried something this client could not interpret.
    Protocol(String),
    /// The server answered the request with an error.
    Server { code: i64, message: String },
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Closed => write!(formatter, "the language server client is closed"),
            Self::Io(message) => write!(formatter, "language server I/O failed: {message}"),
            Self::Protocol(message) => {
                write!(formatter, "language server protocol error: {message}")
            }
            Self::Server { code, message } => {
                write!(formatter, "language server error {code}: {message}")
            }
        }
    }
}

impl std::error::Error for ClientError {}

impl From<FrameError> for ClientError {
    fn from(error: FrameError) -> Self {
        match error {
            FrameError::Io(error) => Self::Io(error.to_string()),
            FrameError::Malformed(message) => Self::Protocol(message),
            FrameError::TooLarge(bytes) => {
                Self::Protocol(format!("message of {bytes} bytes is too large"))
            }
        }
    }
}

use crate::framing::FrameError;

/// Messages the UI sends to the server task.
enum Command {
    Open {
        uri: Uri,
        language_id: LanguageId,
        version: i32,
        text: String,
    },
    Change {
        uri: Uri,
        version: i32,
        text: String,
    },
    Close {
        uri: Uri,
    },
    Request {
        method: String,
        params: Value,
        response: oneshot::Sender<Result<Value, ClientError>>,
    },
    Shutdown {
        done: oneshot::Sender<()>,
    },
}

/// Server state changes the UI observes.
#[derive(Debug)]
pub enum Event {
    /// The initialize handshake completed.
    Ready(Box<ServerCapabilities>),
    /// `textDocument/publishDiagnostics` for an open document.
    Diagnostics {
        uri: Uri,
        version: Option<i32>,
        diagnostics: Vec<Diagnostic>,
    },
    /// A human-readable server message suitable for a log.
    Log { level: LogLevel, message: String },
    /// The client could not start or the handshake failed.
    Failed(String),
    /// The server process ended.
    Exited(Option<i32>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Error,
    Warning,
    Info,
    Log,
}

impl LogLevel {
    fn from_message_type(value: i64) -> Self {
        match value {
            1 => Self::Error,
            2 => Self::Warning,
            3 => Self::Info,
            _ => Self::Log,
        }
    }
}

enum Incoming {
    Message(Value),
    End(String),
}

enum Pending {
    Request(oneshot::Sender<Result<Value, ClientError>>),
    Shutdown(oneshot::Sender<()>),
}

/// Build a document URI from an absolute filesystem path.
pub fn uri_from_path(path: &std::path::Path) -> Result<Uri, ClientError> {
    let url = url::Url::from_file_path(path).map_err(|_| {
        ClientError::Protocol(format!("{:?} is not an absolute document path", path))
    })?;
    Uri::from_str(url.as_str())
        .map_err(|error| ClientError::Protocol(format!("invalid document URI: {error}")))
}

/// How to start one server process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpawnSpec {
    pub command: PathBuf,
    pub args: Vec<OsString>,
    pub root: PathBuf,
    pub language: LanguageId,
    pub server_id: &'static str,
}

impl From<ServerSpec> for SpawnSpec {
    fn from(spec: ServerSpec) -> Self {
        Self {
            command: spec.command,
            args: spec.args,
            root: spec.root,
            language: spec.language,
            server_id: spec.server_id,
        }
    }
}

/// Cloneable handle used by editor providers.
#[derive(Clone)]
pub struct ClientHandle {
    commands: mpsc::UnboundedSender<Command>,
    /// Document version shared by every provider for this open file.
    version: std::sync::Arc<AtomicI32>,
}

impl ClientHandle {
    pub fn open(&self, uri: Uri, language_id: LanguageId, text: String) {
        let version = self.version.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = self.commands.send(Command::Open {
            uri,
            language_id,
            version,
            text,
        });
    }

    pub fn change(&self, uri: Uri, text: String) {
        let version = self.version.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = self.commands.send(Command::Change { uri, version, text });
    }

    pub fn close(&self, uri: Uri) {
        let _ = self.commands.send(Command::Close { uri });
    }

    /// Ask the server for `method` and await its result.
    pub fn request(
        &self,
        method: &str,
        params: Value,
    ) -> impl Future<Output = Result<Value, ClientError>> + use<> {
        let commands = self.commands.clone();
        let method = method.to_owned();
        async move {
            let (response, receiver) = oneshot::channel();
            commands
                .send(Command::Request {
                    method,
                    params,
                    response,
                })
                .map_err(|_| ClientError::Closed)?;
            receiver.await.map_err(|_| ClientError::Closed)?
        }
    }

    /// Ask the server to exit; the actor replies once it has acknowledged.
    pub fn shutdown(&self) {
        let (done, _receiver) = oneshot::channel();
        let _ = self.commands.send(Command::Shutdown { done });
    }
}

/// A running language server and its event stream.
pub struct Client {
    handle: ClientHandle,
    events: mpsc::UnboundedReceiver<Event>,
    abort: AbortOnDrop,
}

impl Client {
    /// Start a server process from a discovered specification.
    pub async fn spawn_process(spec: SpawnSpec) -> Result<Self, ClientError> {
        let root_uri = uri_from_path(&spec.root)?;
        let mut command = ProcessCommand::new(&spec.command);
        command
            .args(&spec.args)
            .current_dir(&spec.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|error| {
            ClientError::Io(format!("could not start {:?}: {error}", spec.command))
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| ClientError::Io("the server has no stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ClientError::Io("the server has no stdout".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| ClientError::Io("the server has no stderr".into()))?;

        tracing::info!(
            server = spec.server_id,
            language = spec.language.as_str(),
            root = %spec.root.display(),
            command = %spec.command.display(),
            pid = child.id(),
            "starting language server"
        );
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(server = spec.server_id, "{line}");
            }
        });

        Ok(Self::start(stdin, stdout, Some(child), root_uri))
    }

    /// Connect over an arbitrary duplex stream. Used by tests and adapters.
    pub async fn connect<S>(stream: S, root_uri: Uri) -> Result<Self, ClientError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (reader, writer) = tokio::io::split(stream);
        Ok(Self::start(writer, reader, None, root_uri))
    }

    fn start<R, W>(writer: W, reader: R, child: Option<Child>, root_uri: Uri) -> Self
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (commands, command_receiver) = mpsc::unbounded_channel();
        let (event_sender, events) = mpsc::unbounded_channel();
        let version = std::sync::Arc::new(AtomicI32::new(0));
        let handle = ClientHandle { commands, version };
        let actor = tokio::spawn(run(
            reader,
            writer,
            child,
            command_receiver,
            event_sender,
            root_uri,
        ));
        let abort = AbortOnDrop(actor.abort_handle());
        Self {
            handle,
            events,
            abort,
        }
    }

    pub fn handle(&self) -> ClientHandle {
        self.handle.clone()
    }

    /// Wait for the next server event.
    pub async fn next_event(&mut self) -> Option<Event> {
        self.events.recv().await
    }

    pub fn take_events(&mut self) -> mpsc::UnboundedReceiver<Event> {
        std::mem::replace(&mut self.events, mpsc::unbounded_channel().1)
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // `kill_on_drop` owns the process; aborting the actor stops protocol work
        // immediately instead of waiting for an unreachable server to answer.
        self.abort.0.abort();
    }
}

struct AbortOnDrop(AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct Connection<W> {
    writer: BufWriter<W>,
    next_id: i64,
    pending: HashMap<i64, Pending>,
    shutdown: bool,
}

impl<W: AsyncWrite + Unpin> Connection<W> {
    fn new(writer: W) -> Self {
        Self {
            writer: BufWriter::new(writer),
            next_id: 1,
            pending: HashMap::new(),
            shutdown: false,
        }
    }

    async fn send_notification(&mut self, method: &str, params: Value) -> Result<(), ClientError> {
        self.write(json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
        .await
    }

    async fn send_request(
        &mut self,
        method: &str,
        params: Value,
        pending: Pending,
    ) -> Result<(), ClientError> {
        let id = self.next_id;
        self.next_id += 1;
        self.pending.insert(id, pending);
        self.write(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))
        .await
    }

    async fn respond(
        &mut self,
        id: Value,
        result: Result<Value, (i64, String)>,
    ) -> Result<(), ClientError> {
        let message = match result {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err((code, message)) => {
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
            }
        };
        self.write(message).await
    }

    async fn write(&mut self, message: Value) -> Result<(), ClientError> {
        let frame = framing::encode(&message)?;
        self.writer
            .write_all(&frame)
            .await
            .map_err(|error| ClientError::Io(error.to_string()))?;
        self.writer
            .flush()
            .await
            .map_err(|error| ClientError::Io(error.to_string()))
    }

    /// Complete one pending entry from a response message.
    fn resolve(
        &mut self,
        id: i64,
        result: Result<Value, ClientError>,
    ) -> Option<oneshot::Sender<()>> {
        match self.pending.remove(&id) {
            Some(Pending::Request(sender)) => {
                let _ = sender.send(result);
                None
            }
            // The caller of shutdown only needs to know the server acknowledged.
            Some(Pending::Shutdown(done)) => Some(done),
            None => {
                tracing::debug!(id, "language server response without a pending request");
                None
            }
        }
    }
}

async fn run<R, W>(
    reader: R,
    writer: W,
    mut child: Option<Child>,
    mut commands: mpsc::UnboundedReceiver<Command>,
    events: mpsc::UnboundedSender<Event>,
    root_uri: Uri,
) where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (incoming_sender, mut incoming) = mpsc::unbounded_channel();
    tokio::spawn(read_loop(reader, incoming_sender));

    let mut connection = Connection::new(writer);
    match handshake(&mut connection, &mut incoming, &events, &root_uri).await {
        Ok(capabilities) => {
            let _ = events.send(Event::Ready(Box::new(capabilities)));
            serve(&mut connection, &mut incoming, &mut commands, &events).await;
        }
        Err(error) => {
            let _ = events.send(Event::Failed(error.to_string()));
        }
    }

    if let Some(mut child) = child.take() {
        // A server that acknowledged shutdown gets a moment to exit on its own;
        // anything else is killed, so a project switch never leaks a process.
        let grace = if connection.shutdown {
            std::time::Duration::from_secs(2)
        } else {
            std::time::Duration::from_millis(300)
        };
        let status = match tokio::time::timeout(grace, child.wait()).await {
            Ok(Ok(status)) => status.code(),
            Ok(Err(error)) => {
                tracing::warn!(%error, "could not wait for the language server");
                None
            }
            Err(_) => {
                let _ = child.kill().await;
                child.wait().await.ok().and_then(|status| status.code())
            }
        };
        let _ = events.send(Event::Exited(status));
    }
}

async fn read_loop<R>(reader: R, sender: mpsc::UnboundedSender<Incoming>)
where
    R: AsyncRead + Unpin,
{
    let mut frames = FrameReader::new(reader);
    loop {
        match frames.read_message().await {
            Ok(Some(message)) => {
                if sender.send(Incoming::Message(message)).is_err() {
                    return;
                }
            }
            Ok(None) => {
                let _ = sender.send(Incoming::End("the server closed its output".into()));
                return;
            }
            Err(error) => {
                let _ = sender.send(Incoming::End(error.to_string()));
                return;
            }
        }
    }
}

fn initialize_params(root_uri: &Uri) -> Value {
    let name = root_uri
        .as_str()
        .split('/')
        .next_back()
        .filter(|segment| !segment.is_empty())
        .unwrap_or("project")
        .to_owned();
    json!({
        "processId": std::process::id(),
        "clientInfo": {"name": "Astrolith", "version": env!("CARGO_PKG_VERSION")},
        "rootUri": root_uri.as_str(),
        "workspaceFolders": [{"uri": root_uri.as_str(), "name": name}],
        "capabilities": {
            "general": {
                "positionEncodings": ["utf-16"]
            },
            "textDocument": {
                "synchronization": {
                    "dynamicRegistration": false,
                    "didSave": false,
                    "willSave": false,
                    "willSaveWaitUntil": false
                },
                "completion": {
                    "dynamicRegistration": false,
                    "contextSupport": true,
                    "completionItem": {
                        "snippetSupport": false,
                        "insertReplaceSupport": false,
                        "documentationFormat": ["markdown", "plaintext"],
                        "deprecatedSupport": true,
                        "preselectSupport": true
                    }
                },
                "hover": {
                    "dynamicRegistration": false,
                    "contentFormat": ["markdown", "plaintext"]
                },
                "definition": {
                    "dynamicRegistration": false,
                    "linkSupport": true
                },
                "codeAction": {
                    "dynamicRegistration": false,
                    "codeActionLiteralSupport": {
                        "codeActionKind": {
                            "valueSet": ["quickfix", "refactor", "refactor.extract", "refactor.inline", "refactor.rewrite", "source", "source.organizeImports", "source.fixAll"]
                        }
                    },
                    "isPreferredSupport": true
                },
                "publishDiagnostics": {
                    "relatedInformation": true,
                    "versionSupport": true,
                    "tagSupport": {"valueSet": [1, 2]}
                }
            },
            "workspace": {
                "workspaceFolders": true,
                "configuration": true,
                "applyEdit": true
            },
            "window": {
                "workDoneProgress": true,
                "showMessage": {},
                "showDocument": {"support": true}
            }
        },
        "initializationOptions": null,
        "trace": "off"
    })
}

async fn handshake(
    connection: &mut Connection<impl AsyncWrite + Unpin>,
    incoming: &mut mpsc::UnboundedReceiver<Incoming>,
    events: &mpsc::UnboundedSender<Event>,
    root_uri: &Uri,
) -> Result<ServerCapabilities, ClientError> {
    let (sender, mut receiver) = oneshot::channel();
    connection
        .send_request(
            "initialize",
            initialize_params(root_uri),
            Pending::Request(sender),
        )
        .await?;

    let result = loop {
        tokio::select! {
            response = &mut receiver => {
                break response.map_err(|_| ClientError::Closed)?;
            }
            message = incoming.recv() => {
                let Some(message) = message else {
                    return Err(ClientError::Closed);
                };
                match message {
                    Incoming::Message(message) => {
                        handle_server_message(connection, message, events).await?;
                    }
                    Incoming::End(reason) => {
                        return Err(ClientError::Io(reason));
                    }
                }
            }
        }
    };

    match result {
        Ok(value) => {
            let initialize: InitializeResult = serde_json::from_value(value).map_err(|error| {
                ClientError::Protocol(format!("invalid initialize result: {error}"))
            })?;
            connection
                .send_notification("initialized", json!({}))
                .await?;
            Ok(initialize.capabilities)
        }
        Err(error) => Err(error),
    }
}

async fn serve(
    connection: &mut Connection<impl AsyncWrite + Unpin>,
    incoming: &mut mpsc::UnboundedReceiver<Incoming>,
    commands: &mut mpsc::UnboundedReceiver<Command>,
    events: &mpsc::UnboundedSender<Event>,
) {
    loop {
        tokio::select! {
            message = incoming.recv() => {
                let Some(message) = message else {
                    return;
                };
                match message {
                    Incoming::Message(message) => {
                        if let Err(error) = handle_server_message(connection, message, events).await {
                            let _ = events.send(Event::Log {
                                level: LogLevel::Error,
                                message: error.to_string(),
                            });
                            return;
                        }
                        if connection.shutdown {
                            return;
                        }
                    }
                    Incoming::End(reason) => {
                        let _ = events.send(Event::Log {
                            level: LogLevel::Warning,
                            message: format!("language server stopped: {reason}"),
                        });
                        return;
                    }
                }
            }
            command = commands.recv() => {
                let Some(command) = command else {
                    return;
                };
                if handle_command(connection, command).await.is_err() {
                    return;
                }
                if connection.shutdown {
                    return;
                }
            }
        }
    }
}

async fn handle_command(
    connection: &mut Connection<impl AsyncWrite + Unpin>,
    command: Command,
) -> Result<(), ClientError> {
    match command {
        Command::Open {
            uri,
            language_id,
            version,
            text,
        } => {
            connection
                .send_notification(
                    "textDocument/didOpen",
                    json!({
                        "textDocument": {
                            "uri": uri.as_str(),
                            "languageId": language_id.as_str(),
                            "version": version,
                            "text": text,
                        }
                    }),
                )
                .await
        }
        Command::Change { uri, version, text } => {
            connection
                .send_notification(
                    "textDocument/didChange",
                    json!({
                        "textDocument": {"uri": uri.as_str(), "version": version},
                        "contentChanges": [{"text": text}],
                    }),
                )
                .await
        }
        Command::Close { uri } => {
            connection
                .send_notification(
                    "textDocument/didClose",
                    json!({"textDocument": {"uri": uri.as_str()}}),
                )
                .await
        }
        Command::Request {
            method,
            params,
            response,
        } => {
            connection
                .send_request(&method, params, Pending::Request(response))
                .await
        }
        Command::Shutdown { done } => {
            connection
                .send_request("shutdown", Value::Null, Pending::Shutdown(done))
                .await
        }
    }
}

async fn handle_server_message(
    connection: &mut Connection<impl AsyncWrite + Unpin>,
    message: Value,
    events: &mpsc::UnboundedSender<Event>,
) -> Result<(), ClientError> {
    let method = message.get("method").and_then(Value::as_str);
    if method.is_none() {
        let Some(id) = message.get("id").and_then(Value::as_i64) else {
            return Ok(());
        };
        let result = match (message.get("result"), message.get("error")) {
            (_, Some(error)) => Err(ClientError::Server {
                code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
                message: error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown server error")
                    .to_owned(),
            }),
            (Some(result), None) => Ok(result.clone()),
            (None, None) => Ok(Value::Null),
        };
        if let Some(done) = connection.resolve(id, result) {
            connection.send_notification("exit", Value::Null).await.ok();
            connection.shutdown = true;
            let _ = done.send(());
        }
        return Ok(());
    }

    let method = method.unwrap_or_default();
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let id = message.get("id").cloned();

    match (method, id) {
        ("textDocument/publishDiagnostics", _) => {
            match serde_json::from_value::<PublishDiagnosticsParams>(params) {
                Ok(diagnostics) => {
                    let _ = events.send(Event::Diagnostics {
                        uri: diagnostics.uri,
                        version: diagnostics.version,
                        diagnostics: diagnostics.diagnostics,
                    });
                }
                Err(error) => {
                    let _ = events.send(Event::Log {
                        level: LogLevel::Warning,
                        message: format!("ignored malformed diagnostics: {error}"),
                    });
                }
            }
            Ok(())
        }
        ("window/logMessage", _) | ("window/showMessage", _) => {
            let level = LogLevel::from_message_type(
                params.get("type").and_then(Value::as_i64).unwrap_or(4),
            );
            let message = params
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let _ = events.send(Event::Log { level, message });
            Ok(())
        }
        (method, Some(id)) => {
            // Server-initiated requests: answer the ones this client can honour
            // and reject the rest, so the server never waits forever.
            let result = match method {
                "workspace/configuration" => {
                    let count = params
                        .get("items")
                        .and_then(Value::as_array)
                        .map_or(0, Vec::len);
                    Ok(Value::Array(vec![Value::Null; count]))
                }
                "client/registerCapability"
                | "client/unregisterCapability"
                | "window/workDoneProgress/create"
                | "window/showMessageRequest" => Ok(Value::Null),
                "workspace/applyEdit" => Ok(json!({"applied": false})),
                _ => Err((-32601, format!("unsupported server request {method}"))),
            };
            connection.respond(id, result).await
        }
        ("$/progress", _) | ("telemetry/event", _) | ("$/logTrace", _) => Ok(()),
        _ => Ok(()),
    }
}

/// LSP request builders used by the editor providers.
pub fn completion_params(uri: &Uri, text: &str, offset: usize, trigger: u8) -> Value {
    let position = offset_to_position(text, offset);
    json!({
        "textDocument": {"uri": uri.as_str()},
        "position": {"line": position.line, "character": position.character},
        "context": {"triggerKind": trigger},
    })
}

pub fn hover_params(uri: &Uri, text: &str, offset: usize) -> Value {
    let position = offset_to_position(text, offset);
    json!({
        "textDocument": {"uri": uri.as_str()},
        "position": {"line": position.line, "character": position.character},
    })
}

pub fn definition_params(uri: &Uri, text: &str, offset: usize) -> Value {
    hover_params(uri, text, offset)
}

pub fn code_action_params(uri: &Uri, text: &str, range: std::ops::Range<usize>) -> Value {
    let start = offset_to_position(text, range.start);
    let end = offset_to_position(text, range.end);
    json!({
        "textDocument": {"uri": uri.as_str()},
        "range": {
            "start": {"line": start.line, "character": start.character},
            "end": {"line": end.line, "character": end.character},
        },
        "context": {"diagnostics": []},
    })
}

/// Offsets of an LSP range in this editor's bytes.
pub fn range_offsets(text: &str, range: lsp_types::Range) -> std::ops::Range<usize> {
    let start = position_to_offset(
        text,
        lsp_types::Position::new(range.start.line, range.start.character),
    );
    let end = position_to_offset(
        text,
        lsp_types::Position::new(range.end.line, range.end.character),
    );
    start..end.max(start)
}

/// Request method names kept in one place so providers and tests agree.
pub mod methods {
    use lsp_types::request::{
        CodeActionRequest, Completion, GotoDefinition, HoverRequest, Request as _, Shutdown,
    };

    pub const COMPLETION: &str = Completion::METHOD;
    pub const HOVER: &str = HoverRequest::METHOD;
    pub const DEFINITION: &str = GotoDefinition::METHOD;
    pub const CODE_ACTION: &str = CodeActionRequest::METHOD;
    pub const SHUTDOWN: &str = Shutdown::METHOD;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framing::encode;
    use tokio::io::AsyncReadExt;

    fn uri(value: &str) -> Uri {
        Uri::from_str(value).unwrap()
    }

    async fn read_frame(stream: &mut (impl AsyncRead + Unpin), buffer: &mut Vec<u8>) -> Value {
        loop {
            if let Some((header_end, header_len)) = find_headers(buffer) {
                let headers = String::from_utf8_lossy(&buffer[..header_end]).into_owned();
                let length = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("Content-Length: "))
                    .unwrap()
                    .trim()
                    .parse::<usize>()
                    .unwrap();
                let start = header_end + header_len;
                if buffer.len() >= start + length {
                    let body = buffer[start..start + length].to_vec();
                    buffer.drain(..start + length);
                    return serde_json::from_slice(&body).unwrap();
                }
            }
            let mut chunk = [0u8; 4096];
            let read = stream.read(&mut chunk).await.unwrap();
            assert!(read > 0, "server stream ended early");
            buffer.extend_from_slice(&chunk[..read]);
        }
    }

    fn find_headers(buffer: &[u8]) -> Option<(usize, usize)> {
        buffer
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|index| (index, 4))
    }

    async fn write_frame(stream: &mut (impl AsyncWrite + Unpin), value: Value) {
        stream.write_all(&encode(&value).unwrap()).await.unwrap();
        stream.flush().await.unwrap();
    }

    #[tokio::test]
    async fn performs_the_handshake_and_pushes_diagnostics() {
        let (client_stream, mut server_stream) = tokio::io::duplex(64 * 1024);
        let root = uri("file:///tmp/astrolith-project");
        let mut client = Client::connect(client_stream, root.clone()).await.unwrap();
        let handle = client.handle();
        let server = tokio::spawn(async move {
            let mut buffer = Vec::new();
            let initialize = read_frame(&mut server_stream, &mut buffer).await;
            assert_eq!(initialize["method"], "initialize");
            assert_eq!(initialize["params"]["rootUri"], root.as_str());
            write_frame(
                &mut server_stream,
                json!({
                    "jsonrpc": "2.0",
                    "id": initialize["id"],
                    "result": {"capabilities": {"hoverProvider": true}},
                }),
            )
            .await;
            let initialized = read_frame(&mut server_stream, &mut buffer).await;
            assert_eq!(initialized["method"], "initialized");

            let open = read_frame(&mut server_stream, &mut buffer).await;
            assert_eq!(open["method"], "textDocument/didOpen");
            assert_eq!(open["params"]["textDocument"]["languageId"], "astro");
            assert_eq!(open["params"]["textDocument"]["text"], "<h1>Hi</h1>");
            write_frame(
                &mut server_stream,
                json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/publishDiagnostics",
                    "params": {
                        "uri": "file:///tmp/astrolith-project/src/pages/index.astro",
                        "version": 1,
                        "diagnostics": [{
                            "range": {
                                "start": {"line": 0, "character": 0},
                                "end": {"line": 0, "character": 4}
                            },
                            "severity": 1,
                            "message": "Unclosed tag",
                        }],
                    },
                }),
            )
            .await;
            let shutdown = read_frame(&mut server_stream, &mut buffer).await;
            assert_eq!(shutdown["method"], "shutdown");
            write_frame(
                &mut server_stream,
                json!({"jsonrpc": "2.0", "id": shutdown["id"], "result": null}),
            )
            .await;
            let exit = read_frame(&mut server_stream, &mut buffer).await;
            assert_eq!(exit["method"], "exit");
        });

        let ready = client.next_event().await.unwrap();
        assert!(
            matches!(ready, Event::Ready(capabilities) if capabilities.hover_provider == Some(lsp_types::HoverProviderCapability::Simple(true)))
        );

        handle.open(
            uri("file:///tmp/astrolith-project/src/pages/index.astro"),
            LanguageId::Astro,
            "<h1>Hi</h1>".into(),
        );
        let diagnostics = client.next_event().await.unwrap();
        match diagnostics {
            Event::Diagnostics {
                version,
                diagnostics,
                ..
            } => {
                assert_eq!(version, Some(1));
                assert_eq!(diagnostics.len(), 1);
                assert_eq!(diagnostics[0].message, "Unclosed tag");
            }
            other => panic!("unexpected event {other:?}"),
        }
        handle.shutdown();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn answers_server_requests_and_forwards_hover_results() {
        let (client_stream, mut server_stream) = tokio::io::duplex(64 * 1024);
        let root = uri("file:///tmp/astrolith-project");
        let mut client = Client::connect(client_stream, root).await.unwrap();
        let handle = client.handle();
        let server = tokio::spawn(async move {
            let mut buffer = Vec::new();
            let initialize = read_frame(&mut server_stream, &mut buffer).await;
            write_frame(
                &mut server_stream,
                json!({"jsonrpc": "2.0", "id": initialize["id"], "result": {"capabilities": {}}}),
            )
            .await;
            read_frame(&mut server_stream, &mut buffer).await; // initialized
            write_frame(
                &mut server_stream,
                json!({
                    "jsonrpc": "2.0",
                    "id": 99,
                    "method": "workspace/configuration",
                    "params": {"items": [{}, {}]},
                }),
            )
            .await;
            let response = read_frame(&mut server_stream, &mut buffer).await;
            assert_eq!(response["id"], 99);
            assert_eq!(response["result"], json!([null, null]));
            let hover = read_frame(&mut server_stream, &mut buffer).await;
            assert_eq!(hover["method"], "textDocument/hover");
            write_frame(
                &mut server_stream,
                json!({
                    "jsonrpc": "2.0",
                    "id": hover["id"],
                    "result": {"contents": {"kind": "plaintext", "value": "docs"}},
                }),
            )
            .await;
        });

        assert!(matches!(
            client.next_event().await.unwrap(),
            Event::Ready(_)
        ));
        let value = handle
            .request(
                methods::HOVER,
                hover_params(&uri("file:///tmp/a.astro"), "x", 0),
            )
            .await
            .unwrap();
        assert_eq!(value["contents"]["value"], "docs");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn reports_a_server_error_reply() {
        let (client_stream, mut server_stream) = tokio::io::duplex(64 * 1024);
        let root = uri("file:///tmp/p");
        let mut client = Client::connect(client_stream, root).await.unwrap();
        let handle = client.handle();
        let server = tokio::spawn(async move {
            let mut buffer = Vec::new();
            let initialize = read_frame(&mut server_stream, &mut buffer).await;
            write_frame(
                &mut server_stream,
                json!({"jsonrpc": "2.0", "id": initialize["id"], "result": {"capabilities": {}}}),
            )
            .await;
            read_frame(&mut server_stream, &mut buffer).await;
            let request = read_frame(&mut server_stream, &mut buffer).await;
            write_frame(
                &mut server_stream,
                json!({
                    "jsonrpc": "2.0",
                    "id": request["id"],
                    "error": {"code": -32601, "message": "not supported"},
                }),
            )
            .await;
        });
        assert!(matches!(
            client.next_event().await.unwrap(),
            Event::Ready(_)
        ));
        let error = handle
            .request(methods::DEFINITION, json!({}))
            .await
            .unwrap_err();
        assert!(matches!(error, ClientError::Server { code: -32601, .. }));
        server.await.unwrap();
    }

    #[test]
    fn builds_positions_from_utf16_offsets() {
        let document = uri("file:///tmp/a.astro");
        let params = hover_params(&document, "let 🎉x = 1", "let 🎉".len());
        assert_eq!(params["position"]["character"], 6);
        assert_eq!(
            range_offsets(
                "let 🎉x",
                lsp_types::Range::new(
                    lsp_types::Position::new(0, 4),
                    lsp_types::Position::new(0, 6),
                )
            ),
            4.."let 🎉".len()
        );
    }
}
