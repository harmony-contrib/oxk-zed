use arkts_services::{Document, Json5Document, ResourceIndex};
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
    json5_documents: BTreeMap<String, Arc<Json5Document>>,
    requests: BTreeMap<String, PendingRequest>,
    roots: Vec<PathBuf>,
    resources: ResourceIndex,
    options: Value,
    chinese: bool,
    context: Context,
    shutdown_completed: bool,
    arkts_diagnostics: BTreeMap<String, CachedDiagnostics>,
}

// Owned protocol values only. A cache entry must never keep a Project/Program/checker alive.
struct CachedDiagnostics {
    project_id: String,
    program_version: u64,
    items: Arc<[Value]>,
}

struct PendingRequest {
    message: Value,
    document: Option<Arc<Document>>,
    json5: Option<Arc<Json5Document>>,
}

impl Default for HostState {
    fn default() -> Self {
        Self {
            documents: BTreeMap::new(),
            json5_documents: BTreeMap::new(),
            requests: BTreeMap::new(),
            roots: Vec::new(),
            resources: ResourceIndex::default(),
            options: json!({}),
            chinese: false,
            context: Context::background(),
            shutdown_completed: false,
            arkts_diagnostics: BTreeMap::new(),
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
        let method = message["method"].as_str();
        let needs_document = matches!(
            method,
            Some(
                "textDocument/hover"
                    | "textDocument/completion"
                    | "textDocument/definition"
                    | "textDocument/documentLink"
                    | "textDocument/codeAction"
                    | "textDocument/diagnostic"
            )
        );
        // Pass-through responses need no host state. Holding their document here would
        // retain every intermediate edit until the corresponding tsrs response arrives.
        if message.get("id").is_some()
            && (needs_document
                || matches!(
                    method,
                    Some("initialize" | "shutdown" | "codeAction/resolve" | "documentLink/resolve")
                ))
        {
            let uri = message
                .pointer("/params/textDocument/uri")
                .and_then(Value::as_str);
            let document = needs_document
                .then(|| uri.and_then(|uri| self.documents.get(uri)).cloned())
                .flatten();
            let json5 = needs_document
                .then(|| uri.and_then(|uri| self.json5_documents.get(uri)).cloned())
                .flatten();
            self.requests.insert(
                message["id"].to_string(),
                PendingRequest {
                    message: message.clone(),
                    document,
                    json5,
                },
            );
        }
        let params = &message["params"];
        match message["method"].as_str() {
            Some("$/cancelRequest") => {
                let id = params["id"].to_string();
                // tsrs still sends the protocol response; canceled requests no longer need
                // host additions or their old document. Keep session lifecycle bookkeeping.
                if self.requests.get(&id).is_some_and(|request| {
                    !matches!(
                        request.message["method"].as_str(),
                        Some("initialize" | "shutdown")
                    )
                }) {
                    self.requests.remove(&id);
                }
            }
            Some("textDocument/didOpen") => {
                if let (Some(uri), Some(text)) = (
                    params["textDocument"]["uri"].as_str(),
                    params["textDocument"]["text"].as_str(),
                ) {
                    let path = PathBuf::from(tsrs_lsproto::DocumentUri(uri.into()).file_name());
                    if is_arkts_config(&path) {
                        self.documents.remove(uri);
                        self.json5_documents
                            .insert(uri.into(), Arc::new(Json5Document::parse(text.into())));
                    } else {
                        self.json5_documents.remove(uri);
                        self.documents
                            .insert(uri.into(), Arc::new(Document::parse(text.into())));
                    }
                }
            }
            Some("textDocument/didChange") => {
                if let Some(uri) = params["textDocument"]["uri"].as_str() {
                    let is_json5 = self.json5_documents.contains_key(uri);
                    let text = self
                        .documents
                        .get(uri)
                        .map(|document| document.text.clone())
                        .or_else(|| {
                            self.json5_documents
                                .get(uri)
                                .map(|document| document.text.clone())
                        });
                    if let Some(mut text) = text {
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
                        if is_json5 {
                            self.json5_documents
                                .insert(uri.into(), Arc::new(Json5Document::parse(text)));
                        } else {
                            self.documents
                                .insert(uri.into(), Arc::new(Document::parse(text)));
                        }
                    }
                }
            }
            Some("textDocument/didClose") => {
                if let Some(uri) = params["textDocument"]["uri"].as_str() {
                    self.documents.remove(uri);
                    self.json5_documents.remove(uri);
                    self.arkts_diagnostics.remove(uri);
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
    pub fn document(&self, uri: &str) -> Option<Arc<Document>> {
        self.documents.get(uri).cloned()
    }
    pub fn shutdown_completed(&self) -> bool {
        self.shutdown_completed
    }
    fn extra_result(
        &self,
        request: &Value,
        document: Option<&Document>,
        json5: Option<&Json5Document>,
    ) -> Option<Value> {
        let uri = request.pointer("/params/textDocument/uri")?.as_str()?;
        let source = PathBuf::from(tsrs_lsproto::DocumentUri(uri.into()).file_name());
        let pos = &request["params"]["position"];
        if let Some(json5) = json5 {
            return match request["method"].as_str()? {
                "textDocument/hover" => {
                    self.resources
                        .json5_hover(json5, pos, &source, self.chinese)
                }
                "textDocument/completion" => self.resources.json5_completions(json5, pos, &source),
                "textDocument/definition" => self
                    .resources
                    .json5_definitions(json5, pos, &source, file_uri),
                "textDocument/documentLink" => Some(
                    self.resources
                        .json5_document_links(json5, &source, file_uri),
                ),
                _ => None,
            };
        }
        let document = document?;
        match request["method"].as_str()? {
            "textDocument/hover" => document.binding_hover(pos, self.chinese).or_else(|| {
                self.resources
                    .system_hover(document, pos, self.chinese)
                    .or_else(|| {
                        self.resources
                            .media_hover(document, pos, &source, self.chinese)
                    })
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

fn is_arkts_config(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            matches!(
                name,
                "module.json5" | "module.json" | "app.json5" | "app.json"
            )
        })
}

fn file_uri(path: &Path) -> String {
    let name = path.to_string_lossy();
    // The URI converter splits on `/`; normalize the Windows separator first.
    #[cfg(windows)]
    let name = name.replace('\\', "/");
    tsrs_ls::lsconv::file_name_to_document_uri(&name).0
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
                    self.add_diagnostics(&uri, &mut message["params"]["diagnostics"], None, None);
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
        let json5_request = pending.json5.is_some()
            && matches!(
                request["method"].as_str(),
                Some(
                    "textDocument/hover"
                        | "textDocument/completion"
                        | "textDocument/definition"
                        | "textDocument/documentLink"
                        | "textDocument/diagnostic"
                )
            );
        if message.get("error").is_some()
            && !json5_request
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
                    if pending.json5.is_some()
                        && !matches!(message.pointer("/result/items"), Some(Value::Array(_)))
                    {
                        // tsrs does not model JSON5 files; resource diagnostics are still valid.
                        message.as_object_mut().unwrap().remove("error");
                        message["result"] = json!({"items": [], "kind": "full"});
                    }
                    if let Some(items) = message.pointer_mut("/result/items") {
                        self.add_diagnostics(
                            uri,
                            items,
                            pending.document.clone(),
                            pending.json5.clone(),
                        );
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
        if pending.document.is_some() || pending.json5.is_some() {
            if let Some(extra) = self.state.lock().unwrap().extra_result(
                &request,
                pending.document.as_deref(),
                pending.json5.as_deref(),
            ) {
                message.as_object_mut().unwrap().remove("error");
                message["result"] = extra;
            }
            if let (Some(document), "textDocument/codeAction") = (
                pending.document.as_deref(),
                request["method"].as_str().unwrap_or_default(),
            ) {
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

    fn add_diagnostics(
        &self,
        uri: &str,
        items: &mut Value,
        snapshot: Option<Arc<Document>>,
        json5_snapshot: Option<Arc<Json5Document>>,
    ) {
        let (document, json5, ctx, extra) = {
            let state = self.state.lock().unwrap();
            let json5 = json5_snapshot.or_else(|| state.json5_documents.get(uri).cloned());
            let document = if json5.is_some() {
                None
            } else {
                snapshot.or_else(|| state.documents.get(uri).cloned())
            };
            if json5.is_none() && document.is_none() {
                return;
            }
            let file = PathBuf::from(tsrs_lsproto::DocumentUri(uri.into()).file_name());
            let extra = if let Some(json5) = &json5 {
                state.resources.json5_diagnostics(json5, &file)
            } else if let Some(document) = &document {
                state.resources.diagnostics(document, &file)
            } else {
                Vec::new()
            };
            (document, json5, state.context.clone(), extra)
        };
        let Some(items) = items.as_array_mut() else {
            return;
        };
        items.extend(extra);
        if json5.is_none() {
            if let (Some(server), Some(document)) =
                (self.server.get().and_then(Weak::upgrade), document.as_ref())
            {
                if let Some(extra) = self.arkts_diagnostics(server.session(), &ctx, uri, document) {
                    items.extend(extra.iter().cloned());
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

    fn arkts_diagnostics(
        &self,
        session: &tsrs_project::Session,
        ctx: &Context,
        uri: &str,
        document: &Arc<Document>,
    ) -> Option<Arc<[Value]>> {
        if uri.ends_with(".d.ets") || !self.state.lock().unwrap().documents.contains_key(uri) {
            return None;
        }
        let uri_value = tsrs_lsproto::DocumentUri(uri.into());
        // Output adaptation must never flush pending changes or load another project. The native
        // request already selected/built its Program. Only inspect an existing, matching one.
        let project = session.snapshot().get_default_project(&uri_value)?;
        if project.is_dirty() {
            return None;
        }
        let program = project.get_program()?;
        let file = program.get_source_file(&uri_value.file_name())?;
        if file.text() != document.tsrs_text().as_ref() {
            return None;
        }
        let project_id = project.id().0;
        let program_version = project.program_last_update;
        if let Some(cached) = self.state.lock().unwrap().arkts_diagnostics.get(uri) {
            if cached.project_id == project_id && cached.program_version == program_version {
                return Some(Arc::clone(&cached.items));
            }
        }

        // Extra linter diagnostics are allocated in a Program-owned compiler pool. Re-running
        // that pass for unchanged Programs would accumulate identical diagnostics in its arena.
        // Cache owned results by the native Program generation, including dependency/config edits.
        // The lease still protects results from the normal project checker used by this pass.
        let ctx = tsrs_core::context::with_checker_lifetime(
            ctx,
            tsrs_core::context::CheckerLifetime::Diagnostics,
        );
        let ctx = tsrs_project::with_checker_data_lease(&ctx);
        let _lease = tsrs_project::enter_checker_data_lease(&ctx);
        let converters = tsrs_ls::lsconv::new_converters(
            tsrs_lsproto::PositionEncodingKind::UTF16,
            move |name| {
                program
                    .get_source_file(name)
                    .map(|file| tsrs_ls::lsconv::compute_lsp_line_starts(file.text()))
            },
        );
        let mut items = Vec::new();
        for diagnostic in program.get_arkts_linter_diagnostics(&ctx, Some(file)) {
            let diagnostic = tsrs_ls::lsconv::diagnostic_to_lsp_push(&ctx, &converters, diagnostic);
            let text = tsrs_lsproto::marshal(&diagnostic).ok()?;
            let mut value: Value = serde_json::from_str(&text).ok()?;
            value["source"] = json!("arkts-linter");
            items.push(value);
        }
        let items: Arc<[Value]> = items.into();
        let mut state = self.state.lock().unwrap();
        // Reader and writer are concurrent: don't reinsert after didClose. A newer edit may
        // already be queued while this Program still serves old requests. Cache its result too;
        // the Program generation invalidates it once the edit is applied. Requiring the latest
        // host document here would make queued old requests repeatedly allocate the same result.
        if state.documents.contains_key(uri) {
            state.arkts_diagnostics.insert(
                uri.into(),
                CachedDiagnostics {
                    project_id,
                    program_version,
                    items: Arc::clone(&items),
                },
            );
        }
        Some(items)
        // project remains alive until after the lease and all arena pointers have been dropped.
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
    fn json5_config_documents_are_parsed_updated_and_closed() {
        let mut state = HostState::default();
        let uri = "file:///tmp/entry/src/main/module.json5";
        state
            .observe(&json!({"method":"textDocument/didOpen","params":{
                "textDocument":{"uri":uri,"text":"{module:{icon:\"$media:icon\"}}"}
            }}))
            .unwrap();
        assert!(state.document(uri).is_none());
        assert_eq!(
            state.json5_documents.get(uri).unwrap().strings[0].value,
            "$media:icon"
        );

        state
            .observe(&json!({"method":"textDocument/didChange","params":{
                "textDocument":{"uri":uri,"version":2},
                "contentChanges":[{"text":"{module:{icon:'$media:other'}}"}]
            }}))
            .unwrap();
        assert_eq!(
            state.json5_documents.get(uri).unwrap().strings[0].value,
            "$media:other"
        );

        let request = json!({"method":"textDocument/documentLink","params":{
            "textDocument":{"uri":uri}
        }});
        let extra = state.extra_result(
            &request,
            None,
            state
                .json5_documents
                .get(uri)
                .map(|document| document.as_ref()),
        );
        assert!(extra.is_some());

        state
            .observe(&json!({"method":"textDocument/didClose","params":{
                "textDocument":{"uri":uri}
            }}))
            .unwrap();
        assert!(state.json5_documents.get(uri).is_none());
    }

    struct DiagnosticFixture {
        directory: tempfile::TempDir,
        fs: Arc<crate::SdkFileSystem>,
        session: Arc<tsrs_project::Session>,
        writer: SdkWriter<std::io::Cursor<Vec<u8>>>,
        ctx: Context,
    }

    impl DiagnosticFixture {
        fn new() -> Self {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path().canonicalize().unwrap();
            let fs = Arc::new(crate::SdkFileSystem::new(
                Arc::new(tsrs_vfs::osvfs::fs()),
                root.clone(),
            ));
            fs.configure(
                json!({"compilerOptions":{
                    "noLib":true,"noEmit":true,"needDoArkTsLinter":true,"moduleResolution":"node"
                }}),
                Some(vec![root.clone()]),
            )
            .unwrap();
            let ctx = Context::background();
            let session = tsrs_project::new_session(&tsrs_project::SessionInit {
                background_ctx: ctx.clone(),
                options: Arc::new(tsrs_project::SessionOptions {
                    current_directory: root.to_str().unwrap().into(),
                    position_encoding: tsrs_lsproto::PositionEncodingKind::UTF16,
                    ..Default::default()
                }),
                fs: fs.clone(),
                client: None,
                logger: None,
                npm_executor: None,
                spawner: None,
                content_mapper_logger: None,
                parse_cache: None,
                content_mapped_parse_cache: None,
            });
            let writer = SdkWriter::new(
                std::io::Cursor::new(Vec::new()),
                Arc::new(Mutex::new(HostState::default())),
                Arc::new(OnceLock::new()),
            );
            Self {
                directory,
                fs,
                session,
                writer,
                ctx,
            }
        }
        fn path(&self, name: &str) -> PathBuf {
            self.directory.path().canonicalize().unwrap().join(name)
        }
        fn uri(&self, name: &str) -> tsrs_lsproto::DocumentUri {
            tsrs_ls::lsconv::file_name_to_document_uri(self.path(name).to_str().unwrap())
        }
        fn write(&self, name: &str, text: &str) {
            let path = self.path(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        fn changed(&self, paths: Vec<PathBuf>) {
            let events: Vec<_> = paths
                .iter()
                .map(|path| tsrs_lsproto::FileEvent {
                    uri: tsrs_ls::lsconv::file_name_to_document_uri(path.to_str().unwrap()),
                    type_: tsrs_lsproto::FileChangeType::CHANGED,
                })
                .collect();
            self.session.did_change_watched_files(&self.ctx, &events);
        }
        fn open(&self, name: &str, text: &str) {
            let uri = self.uri(name);
            let configs = self.fs.set_open(self.path(name), true);
            self.writer
                .state
                .lock()
                .unwrap()
                .observe(&json!({"method":"textDocument/didOpen",
                "params":{"textDocument":{"uri":uri.0,"text":text}}}))
                .unwrap();
            self.session.did_open_file(
                &self.ctx,
                uri,
                1,
                text.into(),
                tsrs_lsproto::LanguageKind("ets"),
            );
            self.changed(configs);
        }
        fn load(&self, name: &str) -> tsrs_project::dirty::Shared<tsrs_project::Project> {
            let uri = self.uri(name);
            self.session.get_language_service(&self.ctx, &uri).unwrap();
            self.session.snapshot().get_default_project(&uri).unwrap()
        }
        fn diagnostics(&self, name: &str) -> Option<Arc<[Value]>> {
            let uri = self.uri(name);
            let document = self.writer.state.lock().unwrap().document(&uri.0).unwrap();
            self.writer
                .arkts_diagnostics(&self.session, &self.ctx, &uri.0, &document)
        }
    }

    impl Drop for DiagnosticFixture {
        fn drop(&mut self) {
            self.session.close();
            self.session.wait_for_background_tasks();
        }
    }

    #[test]
    fn virtual_project_loads_open_files_and_imports_without_workspace_scan() {
        let fixture = DiagnosticFixture::new();
        let main = "import { value } from './oh_modules/pkg/index'; export const result = value;";
        fixture.write("entry/main.ets", main);
        fixture.write("entry/oh_modules/pkg/index.ts", "export const value = 1;");
        let unrelated = [
            "unopened.ets",
            "entry/build/generated.ets",
            "features/widget/build/generated.ts",
            "entry/oh_modules/unused/index.ts",
            "entry/node_modules/unused/index.ts",
            "entry/.hvigor/generated.ets",
        ];
        for name in unrelated {
            fixture.write(name, "export const unused = 1;");
        }
        fixture.open("entry/main.ets", main);
        {
            let project = fixture.load("entry/main.ets");
            let program = project.get_program().unwrap();
            assert!(
                program
                    .get_source_file(
                        fixture
                            .path("entry/oh_modules/pkg/index.ts")
                            .to_str()
                            .unwrap()
                    )
                    .is_some(),
                "explicit imports must remain resolvable"
            );
            for name in unrelated {
                assert!(
                    program
                        .get_source_file(fixture.path(name).to_str().unwrap())
                        .is_none(),
                    "unrequested file {name} was scanned into the Program"
                );
            }
        }
        fixture.open("unopened.ets", "export const unused = 1;");
        assert!(fixture
            .load("entry/main.ets")
            .get_program()
            .unwrap()
            .get_source_file(fixture.path("unopened.ets").to_str().unwrap())
            .is_some());
        let configs = fixture.fs.set_open(fixture.path("unopened.ets"), false);
        fixture
            .session
            .did_close_file(&fixture.ctx, fixture.uri("unopened.ets"));
        fixture.changed(configs);
        assert!(
            fixture
                .load("entry/main.ets")
                .get_program()
                .unwrap()
                .get_source_file(fixture.path("unopened.ets").to_str().unwrap())
                .is_none(),
            "closing a root must remove it unless another file imports it"
        );
    }

    #[test]
    fn writer_reuses_linter_results_and_invalidates_on_dependency_change() {
        let fixture = DiagnosticFixture::new();
        let main = "import { value } from './dep'; export const result: any = value;";
        fixture.write("main.ets", main);
        fixture.write("dep.ts", "export const value = 1;");
        fixture.open("main.ets", main);
        let project = fixture.load("main.ets");
        let old_project = Arc::downgrade(project.arc());
        drop(project);
        let snapshot_id = fixture.session.snapshot().id();
        let first = fixture.diagnostics("main.ets").unwrap();
        assert!(!first.is_empty(), "fixture must exercise linter allocation");
        for _ in 0..50 {
            assert!(Arc::ptr_eq(
                &first,
                &fixture.diagnostics("main.ets").unwrap()
            ));
        }
        assert_eq!(fixture.session.snapshot().id(), snapshot_id);
        fixture.write("dep.ts", "export const value = 'changed';");
        fixture.changed(vec![fixture.path("dep.ts")]);
        fixture.load("main.ets");
        fixture.session.wait_for_background_tasks();
        assert!(
            old_project.upgrade().is_none(),
            "owned diagnostic cache retained an old Project"
        );
        let second = fixture.diagnostics("main.ets").unwrap();
        assert!(
            !Arc::ptr_eq(&first, &second),
            "dependency edits must invalidate cached linter results"
        );
        let uri = fixture.uri("main.ets");
        fixture
            .writer
            .state
            .lock()
            .unwrap()
            .observe(&json!({"method":"textDocument/didClose",
            "params":{"textDocument":{"uri":uri.0}}}))
            .unwrap();
        assert!(fixture
            .writer
            .state
            .lock()
            .unwrap()
            .arkts_diagnostics
            .is_empty());
    }

    #[test]
    fn writer_does_not_flush_pending_edits_or_load_missing_projects() {
        let fixture = DiagnosticFixture::new();
        let main = "export const result: any = 1;";
        let uri = fixture.uri("main.ets");
        let document = Arc::new(Document::parse(main.into()));
        fixture
            .writer
            .state
            .lock()
            .unwrap()
            .observe(&json!({"method":"textDocument/didOpen",
            "params":{"textDocument":{"uri":uri.0,"text":main}}}))
            .unwrap();
        let snapshot_id = fixture.session.snapshot().id();
        assert!(fixture
            .writer
            .arkts_diagnostics(&fixture.session, &fixture.ctx, &uri.0, &document)
            .is_none());
        assert_eq!(fixture.session.snapshot().id(), snapshot_id);
        fixture.write("main.ets", main);
        fixture.open("main.ets", main);
        fixture.load("main.ets");
        let first = fixture.diagnostics("main.ets").unwrap();
        let snapshot_id = fixture.session.snapshot().id();
        fixture.session.did_change_file(
            &fixture.ctx,
            uri.clone(),
            2,
            tsrs_lsproto::unmarshal(br#"[{"text":"export const result: number = 2;"}]"#).unwrap(),
        );
        assert!(Arc::ptr_eq(
            &first,
            &fixture.diagnostics("main.ets").unwrap()
        ));
        assert_eq!(
            fixture.session.snapshot().id(),
            snapshot_id,
            "writer flushed pending edits"
        );
        fixture.load("main.ets");
        assert!(
            fixture.diagnostics("main.ets").is_none(),
            "different source versions must not be mixed"
        );
    }

    #[test]
    fn queued_old_responses_share_results_and_closed_documents_are_skipped() {
        let fixture = DiagnosticFixture::new();
        let main = "export const result: any = 1;";
        fixture.write("main.ets", main);
        fixture.open("main.ets", main);
        fixture.load("main.ets");
        let uri = fixture.uri("main.ets");
        let old = fixture
            .writer
            .state
            .lock()
            .unwrap()
            .document(&uri.0)
            .unwrap();
        // The reader can run ahead of dispatch/writer; no native Program update yet.
        fixture.writer.state.lock().unwrap().observe(&json!({"method":"textDocument/didChange",
            "params":{"textDocument":{"uri":uri.0},"contentChanges":[{"text":"export const result = 2;"}]}})).unwrap();
        let first = fixture
            .writer
            .arkts_diagnostics(&fixture.session, &fixture.ctx, &uri.0, &old)
            .unwrap();
        assert!(!first.is_empty());
        for _ in 0..50 {
            let next = fixture
                .writer
                .arkts_diagnostics(&fixture.session, &fixture.ctx, &uri.0, &old)
                .unwrap();
            assert!(
                Arc::ptr_eq(&first, &next),
                "queued responses repeatedly allocated linter diagnostics"
            );
        }
        fixture
            .writer
            .state
            .lock()
            .unwrap()
            .observe(&json!({"method":"textDocument/didClose",
            "params":{"textDocument":{"uri":uri.0}}}))
            .unwrap();
        assert!(fixture
            .writer
            .arkts_diagnostics(&fixture.session, &fixture.ctx, &uri.0, &old)
            .is_none());
    }

    #[test]
    fn canceled_request_releases_its_document_before_response() {
        let mut state = HostState::default();
        state
            .observe(&json!({"method":"textDocument/didOpen","params":{
                "textDocument":{"uri":"file:///a.ets","text":"let value = 1;"}
            }}))
            .unwrap();
        let old = Arc::downgrade(&state.documents["file:///a.ets"]);
        state
            .observe(
                &json!({"id":"hover-1","method":"textDocument/hover","params":{
                    "textDocument":{"uri":"file:///a.ets"},"position":{"line":0,"character":4}
                }}),
            )
            .unwrap();
        state
            .observe(&json!({"method":"textDocument/didChange","params":{
                "textDocument":{"uri":"file:///a.ets"},"contentChanges":[{"text":"let value = 2;"}]
            }}))
            .unwrap();
        assert!(old.upgrade().is_some());
        state
            .observe(&json!({"method":"$/cancelRequest","params":{"id":"hover-1"}}))
            .unwrap();
        assert!(
            old.upgrade().is_none(),
            "canceled hover retained the old document"
        );
    }

    #[test]
    fn passthrough_requests_do_not_retain_document_versions() {
        let mut state = HostState::default();
        state
            .observe(&json!({"method":"textDocument/didOpen","params":{
                "textDocument":{"uri":"file:///a.ets","text":"let value = 1;"}
            }}))
            .unwrap();
        let old = Arc::downgrade(&state.documents["file:///a.ets"]);
        for method in [
            "textDocument/semanticTokens/full",
            "textDocument/references",
            "textDocument/inlayHint",
        ] {
            state
                .observe(&json!({"id":method,"method":method,"params":{
                    "textDocument":{"uri":"file:///a.ets"}
                }}))
                .unwrap();
        }
        state
            .observe(&json!({"method":"textDocument/didClose","params":{
                "textDocument":{"uri":"file:///a.ets"}
            }}))
            .unwrap();
        assert!(
            old.upgrade().is_none(),
            "passthrough requests retained a closed document"
        );
        assert!(state.requests.is_empty());
    }

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
