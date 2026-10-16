use arkts_services::{Document, ResourceIndex};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
};
use tsrs_core::context::Context;
use tsrs_lsp::{Server, Writer};
use tsrs_lsproto::{Error, Message};

pub(crate) struct HostState {
    documents: BTreeMap<String, Arc<Document>>,
    requests: BTreeMap<String, PendingRequest>,
    roots: Vec<PathBuf>,
    resources: ResourceIndex,
    options: Value,
    chinese: bool,
    context: Context,
    shutdown_completed: bool,
}

struct PendingRequest {
    message: Value,
    document: Option<Arc<Document>>,
}

impl Default for HostState {
    fn default() -> Self {
        Self {
            documents: BTreeMap::new(),
            requests: BTreeMap::new(),
            roots: Vec::new(),
            resources: ResourceIndex::default(),
            options: json!({}),
            chinese: false,
            context: Context::background(),
            shutdown_completed: false,
        }
    }
}

impl HostState {
    pub fn initialize(
        &mut self,
        options: &Value,
        roots: &[PathBuf],
        params: &mut Value,
    ) -> Result<(), String> {
        self.chinese = params["locale"]
            .as_str()
            .is_some_and(|s| s.starts_with("zh"));
        // The old ArkTS server uses UTF-16. Prefer it when offered, so OXC additions and tsrs agree.
        // Every LSP client must support the default UTF-16 encoding, even if not listed.
        if params
            .pointer("/capabilities/general/positionEncodings")
            .is_some()
        {
            params["capabilities"]["general"]["positionEncodings"] = json!(["utf-16"]);
        }
        let capabilities: tsrs_lsproto::ClientCapabilities = tsrs_lsproto::unmarshal(
            &serde_json::to_vec(&params["capabilities"]).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        self.context = tsrs_lsproto::with_client_capabilities(
            &Context::background(),
            Arc::new(capabilities.resolve()),
        );
        self.roots = roots.to_vec();
        self.resources = ResourceIndex::load(roots, options);
        self.options = options.clone();
        Ok(())
    }
    pub fn reconfigure(&mut self, options: &Value) {
        self.resources = ResourceIndex::load(&self.roots, options);
        self.options = options.clone();
    }
    pub fn set_roots(&mut self, roots: Vec<PathBuf>) {
        self.roots = roots;
        self.resources = ResourceIndex::load(&self.roots, &self.options);
    }
    pub fn observe(&mut self, message: &Value) -> Result<(), String> {
        if message.get("method").is_some() && message.get("id").is_some() {
            let document = message
                .pointer("/params/textDocument/uri")
                .and_then(Value::as_str)
                .and_then(|uri| self.documents.get(uri))
                .cloned();
            self.requests.insert(
                message["id"].to_string(),
                PendingRequest {
                    message: message.clone(),
                    document,
                },
            );
        }
        let params = &message["params"];
        match message["method"].as_str() {
            Some("textDocument/didOpen") => {
                if let (Some(uri), Some(text)) = (
                    params["textDocument"]["uri"].as_str(),
                    params["textDocument"]["text"].as_str(),
                ) {
                    self.documents
                        .insert(uri.into(), Arc::new(Document::parse(text.into())));
                }
            }
            Some("textDocument/didChange") => {
                if let Some(uri) = params["textDocument"]["uri"].as_str() {
                    if let Some(document) = self.documents.get(uri) {
                        let mut text = document.text.clone();
                        if let Some(changes) = params["contentChanges"].as_array() {
                            for change in changes {
                                let Some(new) = change["text"].as_str() else {
                                    continue;
                                };
                                if change.get("range").is_none() {
                                    text = new.to_string();
                                } else {
                                    let start =
                                        arkts_services::offset_at(&text, &change["range"]["start"])
                                            .ok_or("invalid UTF-16 edit start")?;
                                    let end =
                                        arkts_services::offset_at(&text, &change["range"]["end"])
                                            .ok_or("invalid UTF-16 edit end")?;
                                    if start > end {
                                        return Err("edit start is after edit end".into());
                                    }
                                    text.replace_range(start..end, new);
                                }
                            }
                        }
                        self.documents
                            .insert(uri.into(), Arc::new(Document::parse(text)));
                    }
                }
            }
            Some("textDocument/didClose") => {
                if let Some(uri) = params["textDocument"]["uri"].as_str() {
                    self.documents.remove(uri);
                }
            }
            Some("workspace/didChangeWatchedFiles")
                if params["changes"].as_array().is_some_and(|changes| {
                    changes.iter().any(|change| {
                        let Some(uri) = change["uri"].as_str() else {
                            return false;
                        };
                        let path = tsrs_lsproto::DocumentUri(uri.into()).file_name();
                        Path::new(&path)
                            .components()
                            .any(|c| c.as_os_str() == "resources")
                            || path.ends_with("module.json5")
                            || path.ends_with("sysResource.js")
                    })
                }) =>
            {
                self.resources = ResourceIndex::load(&self.roots, &self.options);
            }
            _ => {}
        }
        Ok(())
    }
    pub fn compiler_text(&self, uri: &str) -> Option<String> {
        self.documents
            .get(uri)
            .map(|document| document.tsrs_text().into_owned())
    }
    pub fn shutdown_completed(&self) -> bool {
        self.shutdown_completed
    }
    fn extra_result(&self, request: &Value, document: &Document) -> Option<Value> {
        let uri = request.pointer("/params/textDocument/uri")?.as_str()?;
        let source = PathBuf::from(tsrs_lsproto::DocumentUri(uri.into()).file_name());
        let pos = &request["params"]["position"];
        match request["method"].as_str()? {
            "textDocument/hover" => document.binding_hover(pos, self.chinese).or_else(|| {
                self.resources
                    .media_hover(document, pos, &source, self.chinese)
            }),
            "textDocument/completion" => self.resources.completions(document, pos, &source),
            "textDocument/definition" => {
                self.resources.definitions(document, pos, &source, file_uri)
            }
            // Document links were exposed by the previous server; native actions are eagerly resolved.
            "textDocument/documentLink" => Some(document.document_links(&source, file_uri)),
            _ => None,
        }
    }
}

fn file_uri(path: &Path) -> String {
    tsrs_ls::lsconv::file_name_to_document_uri(&path.to_string_lossy()).0
}

pub(crate) struct SdkWriter<W: Write> {
    output: tsrs_lsproto::BaseWriter<W>,
    state: Arc<Mutex<HostState>>,
    server: Arc<OnceLock<Weak<Server>>>,
}
impl<W: Write> SdkWriter<W> {
    pub fn new(
        output: W,
        state: Arc<Mutex<HostState>>,
        server: Arc<OnceLock<Weak<Server>>>,
    ) -> Self {
        Self {
            output: tsrs_lsproto::new_base_writer(output),
            state,
            server,
        }
    }
    fn adapt(&self, message: &mut Value) {
        if let Some(method) = message["method"].as_str() {
            if method == "workspace/configuration" {
                // The client receives the same sections as tsrs; legacy preference aliases are adapted on reply.
            }
            if method == "textDocument/publishDiagnostics" {
                if let Some(uri) = message["params"]["uri"].as_str().map(ToString::to_string) {
                    self.add_diagnostics(&uri, &mut message["params"]["diagnostics"], None);
                }
            }
            return;
        }
        let request = self
            .state
            .lock()
            .unwrap()
            .requests
            .remove(&message["id"].to_string());
        let Some(pending) = request else {
            return;
        };
        let request = pending.message;
        // Native cancellation/validation errors must remain errors. Only methods
        // implemented by this host can replace tsrs's unknown-method response.
        if message.get("error").is_some()
            && !(matches!(
                request["method"].as_str(),
                Some("textDocument/documentLink" | "documentLink/resolve" | "codeAction/resolve")
            ) && matches!(
                message.pointer("/error/code").and_then(Value::as_i64),
                Some(-32601 | -32600)
            ))
        {
            return;
        }
        match request["method"].as_str() {
            Some("shutdown") if message.get("error").is_none() => {
                self.state.lock().unwrap().shutdown_completed = true;
            }
            Some("initialize") => {
                if let Some(capabilities) = message
                    .pointer_mut("/result/capabilities")
                    .and_then(Value::as_object_mut)
                {
                    capabilities.remove("documentFormattingProvider");
                    capabilities.remove("documentRangeFormattingProvider");
                    capabilities.insert(
                        "documentLinkProvider".into(),
                        json!({"resolveProvider":false}),
                    );
                    if let Some(chars) = capabilities
                        .get_mut("completionProvider")
                        .and_then(|v| v["triggerCharacters"].as_array_mut())
                    {
                        for c in [":", "$", "/"] {
                            if !chars.iter().any(|v| v == c) {
                                chars.push(json!(c));
                            }
                        }
                    }
                }
                message["result"]["serverInfo"]["name"] = json!("arkts-lsp (tsrs + oxc)");
            }
            Some("textDocument/diagnostic") => {
                if let Some(uri) = request
                    .pointer("/params/textDocument/uri")
                    .and_then(Value::as_str)
                {
                    if let Some(items) = message.pointer_mut("/result/items") {
                        self.add_diagnostics(uri, items, pending.document.clone());
                    }
                }
            }
            Some("codeAction/resolve" | "documentLink/resolve")
                if matches!(
                    message.pointer("/error/code").and_then(Value::as_i64),
                    Some(-32601 | -32600)
                ) =>
            {
                message.as_object_mut().unwrap().remove("error");
                message["result"] = request["params"].clone();
            }
            _ => {}
        }
        if let Some(document) = pending.document.as_deref() {
            if let Some(extra) = self.state.lock().unwrap().extra_result(&request, document) {
                message.as_object_mut().unwrap().remove("error");
                message["result"] = extra;
            }
            if request["method"] == "textDocument/codeAction" {
                if let Some(actions) = message["result"].as_array_mut() {
                    actions.retain(|action| !no_op_action(action, &request, document));
                }
            }
        }
        if request["method"] == "textDocument/hover" {
            if let Some(value) = message.pointer_mut("/result/contents/value") {
                if let Some(text) = value.as_str() {
                    *value = json!(text.replace("```typescript\n", "```ets\n"));
                }
            }
        }
    }

    fn add_diagnostics(&self, uri: &str, items: &mut Value, snapshot: Option<Arc<Document>>) {
        let (document, ctx, extra) = {
            let state = self.state.lock().unwrap();
            let Some(document) = snapshot.or_else(|| state.documents.get(uri).cloned()) else {
                return;
            };
            let file = PathBuf::from(tsrs_lsproto::DocumentUri(uri.into()).file_name());
            let extra = state.resources.diagnostics(&document, &file);
            (document, state.context.clone(), extra)
        };
        let Some(items) = items.as_array_mut() else {
            return;
        };
        items.extend(extra);
        // Reuse the session's current Program and checker pools, including their region lifetimes.
        // Keep the LanguageService lease alive through conversion; no arena pointers escape this scope.
        if let Some(server) = self.server.get().and_then(Weak::upgrade) {
            if let Ok(service) = server
                .session()
                .get_language_service(&ctx, &tsrs_lsproto::DocumentUri(uri.into()))
            {
                let program = service.get_program();
                if let Some(file) =
                    program.get_source_file(&tsrs_lsproto::DocumentUri(uri.into()).file_name())
                {
                    if file.text() == document.tsrs_text().as_ref() && !uri.ends_with(".d.ets") {
                        let converters = tsrs_ls::lsconv::new_converters(
                            tsrs_lsproto::PositionEncodingKind::UTF16,
                            move |name| {
                                program
                                    .get_source_file(name)
                                    .map(|f| tsrs_ls::lsconv::compute_lsp_line_starts(f.text()))
                            },
                        );
                        for diagnostic in program.get_arkts_linter_diagnostics(&ctx, Some(file)) {
                            let diagnostic = tsrs_ls::lsconv::diagnostic_to_lsp_pull(
                                &ctx,
                                &converters,
                                diagnostic,
                                false,
                            );
                            if let Ok(text) = tsrs_lsproto::marshal(&diagnostic) {
                                if let Ok(mut value) = serde_json::from_str::<Value>(&text) {
                                    value["source"] = json!("arkts-linter");
                                    items.push(value);
                                }
                            }
                        }
                    }
                }
            }
        }
        // Retain suggestion tags and stable order as in the previous server's diagnostics.
        for item in items.iter_mut() {
            if item["code"] == 6133 && item.get("tags").is_none() {
                item["tags"] = json!([1]);
            }
        }
    }
}

fn no_op_action(action: &Value, request: &Value, document: &Document) -> bool {
    let Some(changes) = action.pointer("/edit/changes").and_then(Value::as_object) else {
        return false;
    };
    if changes.len() != 1 {
        return false;
    }
    let Some(uri) = request
        .pointer("/params/textDocument/uri")
        .and_then(Value::as_str)
    else {
        return false;
    };
    let Some(edits) = changes.get(uri).and_then(Value::as_array) else {
        return false;
    };
    !edits.is_empty()
        && edits.iter().all(|edit| {
            let Some(start) = arkts_services::offset_at(&document.text, &edit["range"]["start"])
            else {
                return false;
            };
            let Some(end) = arkts_services::offset_at(&document.text, &edit["range"]["end"]) else {
                return false;
            };
            document.text.get(start..end) == edit["newText"].as_str()
        })
}

impl<W: Write + Send> Writer for SdkWriter<W> {
    fn write(&mut self, message: &Message) -> Result<(), Error> {
        let encoded = tsrs_lsproto::marshal(message).map_err(Error::new)?;
        let mut value: Value =
            serde_json::from_str(&encoded).map_err(|e| Error::new(e.to_string()))?;
        self.adapt(&mut value);
        let bytes = serde_json::to_vec(&value).map_err(|e| Error::new(e.to_string()))?;
        self.output.write(&bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canceled_requests_and_initialization_errors_are_not_rewritten() {
        let state = Arc::new(Mutex::new(HostState::default()));
        state.lock().unwrap().observe(&json!({"method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///a.ets","text":"const x = $r('app.string.');"}}})).unwrap();
        let writer = SdkWriter::new(
            std::io::Cursor::new(Vec::<u8>::new()),
            Arc::clone(&state),
            Arc::new(OnceLock::new()),
        );
        for method in [
            "initialize",
            "textDocument/diagnostic",
            "textDocument/completion",
            "textDocument/hover",
        ] {
            state.lock().unwrap().observe(&json!({"id":1,"method":method,"params":{"textDocument":{"uri":"file:///a.ets"},"position":{"line":0,"character":24}}})).unwrap();
            let mut response = json!({"jsonrpc":"2.0","id":1,"error":{"code":-32800,"message":"Request cancelled"}});
            let expected = response.clone();
            writer.adapt(&mut response);
            assert_eq!(response, expected);
        }
    }
    #[test]
    fn unchanged_diagnostic_report_retains_its_shape() {
        let state = Arc::new(Mutex::new(HostState::default()));
        state.lock().unwrap().observe(&json!({"id":1,"method":"textDocument/diagnostic","params":{"textDocument":{"uri":"file:///a.ets"}}})).unwrap();
        let writer = SdkWriter::new(
            std::io::Cursor::new(Vec::<u8>::new()),
            state,
            Arc::new(OnceLock::new()),
        );
        let mut response =
            json!({"jsonrpc":"2.0","id":1,"result":{"kind":"unchanged","resultId":"1"}});
        let expected = response.clone();
        writer.adapt(&mut response);
        assert_eq!(response, expected);
    }
}
