//! Editor-facing language-server providers and diagnostic projection.
//!
//! The protocol client lives in `astrolith-lsp`; this module adapts it to the
//! pinned editor's provider traits. Every provider awaits the server on the
//! background executor, so rendering and typing stay independent of it.

use std::{ops::Range, rc::Rc};

use astrolith_lsp::{
    Client, ClientHandle, LanguageId, code_action_params, completion_params, convert_positions,
    definition_params, hover_params, methods, uri_from_path, utf16_position_to_char,
};
use gpui_kit::{
    App, AppContext as _, Entity, SharedString, Task, Window,
    component::highlighter::{Diagnostic, DiagnosticSet},
    component::input::{
        CodeActionProvider, CompletionProvider, DefinitionProvider, EditorState, HoverProvider,
        Rope,
    },
};
use lsp_types::{
    CodeAction, CodeActionOrCommand, CompletionContext, CompletionResponse, GotoDefinitionResponse,
    Hover, LocationLink, WorkspaceEdit,
};

/// One running server bound to one open build-mode document.
pub(crate) struct LspState {
    /// Keeps the process and protocol loop alive for as long as the session.
    pub _client: Client,
    pub handle: ClientHandle,
    pub uri: lsp_types::Uri,
    pub language: LanguageId,
}

impl LspState {
    pub fn open(&self, text: String) {
        self.handle.open(self.uri.clone(), self.language, text);
    }

    pub fn close(&self) {
        self.handle.close(self.uri.clone());
    }
}

/// Install completion, hover, definition and code-action providers on `editor`.
pub(crate) fn install(
    editor: &Entity<EditorState>,
    handle: ClientHandle,
    uri: lsp_types::Uri,
    cx: &mut App,
) {
    editor.update(cx, |editor, cx| {
        let provider = Rc::new(LspProvider {
            handle: handle.clone(),
            uri: uri.clone(),
        });
        let actions = Rc::new(CodeActions { handle, uri });
        let lsp = editor.lsp_mut();
        lsp.completion_provider = Some(provider.clone());
        lsp.hover_provider = Some(provider.clone());
        lsp.definition_provider = Some(provider);
        lsp.code_action_providers = vec![actions];
        editor.refresh(cx);
    });
}

/// Remove every provider and the diagnostics the server contributed.
pub(crate) fn uninstall(editor: &Entity<EditorState>, cx: &mut App) {
    editor.update(cx, |editor, cx| {
        let lsp = editor.lsp_mut();
        lsp.completion_provider = None;
        lsp.hover_provider = None;
        lsp.definition_provider = None;
        lsp.code_action_providers.clear();
        lsp.show_document = None;
        if let Some(diagnostics) = editor.diagnostics_mut() {
            diagnostics.clear();
        }
        editor.refresh(cx);
    });
}

/// Replace the editor's diagnostics with one server publish.
///
/// The server sends UTF-16 positions; the editor counts characters, so every
/// range is converted against the current document before it is published.
pub(crate) fn apply_diagnostics(
    editor: &Entity<EditorState>,
    diagnostics: &[lsp_types::Diagnostic],
    cx: &mut App,
) {
    let text = editor.read(cx).text().clone();
    let plain = text.to_string();
    let mut set = DiagnosticSet::new(&text);
    for diagnostic in diagnostics {
        let mut converted = diagnostic.clone();
        converted.range.start = utf16_position_to_char(&plain, converted.range.start);
        converted.range.end = utf16_position_to_char(&plain, converted.range.end);
        set.push(Diagnostic::from(converted));
    }
    editor.update(cx, |editor, cx| {
        if let Some(current) = editor.diagnostics_mut() {
            *current = set;
        }
        cx.notify();
    });
}

pub(crate) fn uri_for(path: &std::path::Path) -> Option<lsp_types::Uri> {
    uri_from_path(path).ok()
}

struct LspProvider {
    handle: ClientHandle,
    uri: lsp_types::Uri,
}

impl CompletionProvider for LspProvider {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        trigger: CompletionContext,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<CompletionResponse>> {
        let handle = self.handle.clone();
        let uri = self.uri.clone();
        let plain = text.to_string();
        let trigger_kind = match trigger.trigger_kind {
            lsp_types::CompletionTriggerKind::INVOKED => 1,
            lsp_types::CompletionTriggerKind::TRIGGER_CHARACTER => 2,
            lsp_types::CompletionTriggerKind::TRIGGER_FOR_INCOMPLETE_COMPLETIONS => 3,
            _ => 1,
        };
        let trigger_character = trigger.trigger_character.clone();
        cx.background_spawn(async move {
            let mut params = completion_params(&uri, &plain, offset, trigger_kind);
            if let Some(character) = trigger_character {
                params["context"]["triggerCharacter"] = serde_json::Value::String(character);
            }
            let mut value = handle
                .request(methods::COMPLETION, params)
                .await
                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
            convert_positions(&mut value, &plain);
            serde_json::from_value(value)
                .map_err(|error| anyhow::anyhow!("invalid completion response: {error}"))
        })
    }

    fn is_completion_trigger(&self, _offset: usize, new_text: &str, _cx: &mut App) -> bool {
        new_text.chars().next_back().is_some_and(|character| {
            character.is_alphanumeric()
                || matches!(
                    character,
                    '_' | '.' | '$' | '@' | '/' | '-' | '"' | '\'' | '<'
                )
        })
    }
}

impl HoverProvider for LspProvider {
    fn hover(
        &self,
        text: &Rope,
        offset: usize,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<Option<Hover>>> {
        let handle = self.handle.clone();
        let uri = self.uri.clone();
        let plain = text.to_string();
        cx.background_spawn(async move {
            let value = handle
                .request(methods::HOVER, hover_params(&uri, &plain, offset))
                .await
                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
            if value.is_null() {
                return Ok(None);
            }
            let mut value = value;
            convert_positions(&mut value, &plain);
            serde_json::from_value(value)
                .map(Some)
                .map_err(|error| anyhow::anyhow!("invalid hover response: {error}"))
        })
    }
}

impl DefinitionProvider for LspProvider {
    fn definitions(
        &self,
        text: &Rope,
        offset: usize,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<Vec<LocationLink>>> {
        let handle = self.handle.clone();
        let uri = self.uri.clone();
        let plain = text.to_string();
        cx.background_spawn(async move {
            let mut value = handle
                .request(methods::DEFINITION, definition_params(&uri, &plain, offset))
                .await
                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
            convert_positions(&mut value, &plain);
            let response: GotoDefinitionResponse = serde_json::from_value(value)
                .map_err(|error| anyhow::anyhow!("invalid definition response: {error}"))?;
            Ok(match response {
                GotoDefinitionResponse::Scalar(location) => vec![LocationLink {
                    origin_selection_range: None,
                    target_uri: location.uri,
                    target_range: location.range,
                    target_selection_range: location.range,
                }],
                GotoDefinitionResponse::Array(locations) => locations
                    .into_iter()
                    .map(|location| LocationLink {
                        origin_selection_range: None,
                        target_uri: location.uri,
                        target_range: location.range,
                        target_selection_range: location.range,
                    })
                    .collect(),
                GotoDefinitionResponse::Link(links) => links,
            })
        })
    }
}

struct CodeActions {
    handle: ClientHandle,
    uri: lsp_types::Uri,
}

impl CodeActionProvider for CodeActions {
    fn id(&self) -> SharedString {
        "astrolith.lsp".into()
    }

    fn code_actions(
        &self,
        state: Entity<EditorState>,
        range: Range<usize>,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<Vec<CodeAction>>> {
        let handle = self.handle.clone();
        let uri = self.uri.clone();
        let plain = state.read(cx).text().to_string();
        cx.background_spawn(async move {
            let mut value = handle
                .request(
                    methods::CODE_ACTION,
                    code_action_params(&uri, &plain, range),
                )
                .await
                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
            convert_positions(&mut value, &plain);
            let actions: Vec<CodeActionOrCommand> = serde_json::from_value(value)
                .map_err(|error| anyhow::anyhow!("invalid code action response: {error}"))?;
            Ok(actions
                .into_iter()
                .filter_map(|action| match action {
                    CodeActionOrCommand::CodeAction(action) => Some(action),
                    // Commands are server-side work; this client has no
                    // workspace/executeCommand flow, so it does not offer them.
                    CodeActionOrCommand::Command(_) => None,
                })
                .collect())
        })
    }

    fn perform_code_action(
        &self,
        state: Entity<EditorState>,
        action: CodeAction,
        _push_to_history: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<()>> {
        let Some(edit) = action.edit else {
            return Task::ready(Ok(()));
        };
        apply_workspace_edit(&state, &self.uri, edit, window, cx);
        Task::ready(Ok(()))
    }
}

/// Apply the edits that target the open document.
///
/// Cross-file workspace edits are deliberately skipped: opening and editing a
/// different retained session is the workspace's responsibility, not a
/// provider's, and half-applying them would be worse than not offering them.
fn apply_workspace_edit(
    state: &Entity<EditorState>,
    uri: &lsp_types::Uri,
    edit: WorkspaceEdit,
    window: &mut Window,
    cx: &mut App,
) {
    if let Some(changes) = edit.changes {
        for (target, edits) in changes {
            if target.as_str() != uri.as_str() {
                tracing::info!(document = %target.as_str(), "skipping a cross-file code action edit");
                continue;
            }
            state.update(cx, |editor, cx| {
                editor.apply_lsp_edits(&edits, window, cx);
            });
        }
    }
    if edit.document_changes.is_some() {
        tracing::info!("skipping a documentChanges code action; only plain edits are applied");
    }
}
