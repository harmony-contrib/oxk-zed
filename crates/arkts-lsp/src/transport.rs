use crate::SdkFileSystem;
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    io::Read,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tsrs_lsp::Reader;
use tsrs_lsproto::{BaseReader, Error, ErrorCode, Message};

pub(crate) struct SdkReader<R: Read> {
    input: BaseReader<R>,
    fs: Arc<SdkFileSystem>,
    pending: VecDeque<Vec<u8>>,
    state: Arc<Mutex<crate::middleware::HostState>>,
}

impl<R: Read> SdkReader<R> {
    pub fn new(
        input: R,
        fs: Arc<SdkFileSystem>,
        state: Arc<Mutex<crate::middleware::HostState>>,
    ) -> Self {
        Self {
            input: tsrs_lsproto::new_base_reader(input),
            fs,
            pending: VecDeque::new(),
            state,
        }
    }
    fn adapt(&mut self, message: &mut Value) -> Result<(), String> {
        // Volar accepted TS language-service preferences under typescript.preferences;
        // tsrs calls the same fields unstable. Preserve both forms, with native settings taking precedence.
        if message.get("method").is_none() {
            if let Some(items) = message["result"].as_array_mut() {
                for item in items {
                    legacy_preferences(item);
                }
            }
        }
        let env: Vec<_> = std::env::vars().collect();
        match message["method"].as_str() {
            Some("initialize") => {
                let params = &mut message["params"];
                let mut options =
                    arkts_sdk::normalize(params["initializationOptions"].clone(), &env);
                if !options.is_object() {
                    options = json!({});
                }
                let roots: Vec<_> = params["workspaceFolders"]
                    .as_array()
                    .map(|folders| {
                        folders
                            .iter()
                            .filter_map(|f| f["uri"].as_str())
                            .map(uri_path)
                            .collect()
                    })
                    .filter(|roots: &Vec<_>| !roots.is_empty())
                    .unwrap_or_else(|| {
                        params["rootUri"]
                            .as_str()
                            .map(uri_path)
                            .or_else(|| params["rootPath"].as_str().map(PathBuf::from))
                            .into_iter()
                            .collect()
                    });
                self.fs.configure(options.clone(), Some(roots.clone()))?;
                options = self.fs.configuration().0;
                self.state
                    .lock()
                    .unwrap()
                    .initialize(&options, &roots, params)?;
                // tsrs consumes preferences as a section map when the client has no configuration provider.
                if options.get("userPreferences").is_none() {
                    options["userPreferences"] = json!({"unstable": options.pointer("/typescript/preferences").cloned().unwrap_or_else(|| json!({}))});
                }
                params["initializationOptions"] = options;
            }
            Some("workspace/didChangeConfiguration") => {
                let (mut options, _) = self.fs.configuration();
                arkts_sdk::merge(
                    &mut options,
                    &arkts_sdk::normalize(message["params"]["settings"].clone(), &[]),
                );
                options = arkts_sdk::normalize(options, &env);
                let configs = self.fs.configure(options.clone(), None)?;
                options = self.fs.configuration().0;
                self.state.lock().unwrap().reconfigure(&options);
                message["params"]["settings"] = options;
                for section in ["typescript", "javascript"] {
                    legacy_preferences(&mut message["params"]["settings"][section]);
                }
                if !configs.is_empty() {
                    let changes: Vec<_> = configs.iter().map(|p| json!({"uri": tsrs_ls::lsconv::file_name_to_document_uri(&p.to_string_lossy()).0, "type":2})).collect();
                    self.pending.push_back(serde_json::to_vec(&json!({"jsonrpc":"2.0","method":"workspace/didChangeWatchedFiles","params":{"changes":changes}})).unwrap());
                }
            }
            Some("workspace/didChangeWorkspaceFolders") => {
                let (options, mut roots) = self.fs.configuration();
                let event = &message["params"]["event"];
                for item in event["removed"].as_array().into_iter().flatten() {
                    if let Some(uri) = item["uri"].as_str() {
                        roots.retain(|root| *root != uri_path(uri));
                    }
                }
                for item in event["added"].as_array().into_iter().flatten() {
                    if let Some(uri) = item["uri"].as_str() {
                        let root = uri_path(uri);
                        if !roots.contains(&root) {
                            roots.push(root);
                        }
                    }
                }
                self.fs.configure(options, Some(roots.clone()))?;
                self.state.lock().unwrap().set_roots(roots);
            }
            Some("textDocument/didOpen" | "textDocument/didClose") => {
                if let Some(uri) = message
                    .pointer("/params/textDocument/uri")
                    .and_then(Value::as_str)
                {
                    let configs = self
                        .fs
                        .set_open(uri_path(uri), message["method"] == "textDocument/didOpen");
                    if !configs.is_empty() {
                        let changes: Vec<_> = configs.iter().map(|path| json!({
                            "uri": tsrs_ls::lsconv::file_name_to_document_uri(path.to_str().unwrap()).0, "type":2
                        })).collect();
                        // The next dispatch invalidates cached virtual root membership before
                        // subsequent requests; imported files are still resolved by tsrs normally.
                        self.pending.push_back(serde_json::to_vec(&json!({"jsonrpc":"2.0",
                            "method":"workspace/didChangeWatchedFiles","params":{"changes":changes}
                        })).unwrap());
                    }
                }
            }
            _ => {}
        }
        let previous = if message["method"] == "textDocument/didChange" {
            message
                .pointer("/params/textDocument/uri")
                .and_then(Value::as_str)
                .and_then(|uri| self.state.lock().unwrap().document(uri))
        } else {
            None
        };
        self.state.lock().unwrap().observe(message)?;
        if let Some("textDocument/didOpen" | "textDocument/didChange") = message["method"].as_str()
        {
            if let Some(uri) = message
                .pointer("/params/textDocument/uri")
                .and_then(Value::as_str)
            {
                if let Some(document) = self.state.lock().unwrap().document(uri) {
                    if message["method"] == "textDocument/didOpen" {
                        message["params"]["textDocument"]["text"] = json!(document.tsrs_text());
                    } else if let Some(change) = previous
                        .as_deref()
                        .and_then(|previous| document.compiler_change(previous))
                    {
                        message["params"]["contentChanges"] = json!([change]);
                    }
                }
            }
        }
        Ok(())
    }
}

fn legacy_preferences(settings: &mut Value) {
    if let Some(mut preferences) = settings.get("preferences").cloned() {
        if let Some(native) = settings.get("unstable") {
            arkts_sdk::merge(&mut preferences, native);
        }
        settings["unstable"] = preferences;
    }
}

fn uri_path(uri: &str) -> PathBuf {
    tsrs_lsproto::DocumentUri(uri.into()).file_name().into()
}

impl<R: Read + Send> Reader for SdkReader<R> {
    fn read(&mut self) -> (Option<Message>, Option<Error>) {
        let bytes = match self
            .pending
            .pop_front()
            .map(Ok)
            .unwrap_or_else(|| self.input.read())
        {
            Ok(bytes) => bytes,
            Err(error) => return (None, Some(error)),
        };
        let adapted = (|| {
            let mut message: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            self.adapt(&mut message)?;
            serde_json::to_vec(&message).map_err(|e| e.to_string())
        })();
        match adapted {
            Ok(bytes) => match tsrs_lsproto::unmarshal(&bytes) {
                Ok(message) => (Some(message), None),
                Err(error) => (None, Some(Error::from(error))),
            },
            Err(error) => (
                // Preserve the request ID so tsrs sends a correlated InvalidParams response.
                tsrs_lsproto::unmarshal(&bytes).ok(),
                Some(Error::wrap_code(
                    ErrorCode::InvalidParams,
                    Error::new(error),
                )),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reader() -> SdkReader<std::io::Cursor<Vec<u8>>> {
        SdkReader::new(
            std::io::Cursor::new(Vec::new()),
            Arc::new(SdkFileSystem::new(
                Arc::new(tsrs_vfs::osvfs::fs()),
                PathBuf::from("/"),
            )),
            Arc::new(Mutex::new(crate::middleware::HostState::default())),
        )
    }

    #[test]
    fn incremental_edits_do_not_queue_full_document_copies() {
        for binding in ["", "Text($$this.value);\n"] {
            let mut reader = reader();
            let text = format!("const value = 0;\n//{}\n{binding}", "x".repeat(1024 * 1024));
            reader
                .adapt(&mut json!({"method":"textDocument/didOpen","params":{
                    "textDocument":{"uri":"file:///large.ets","text":text}
                }}))
                .unwrap();
            let mut queued_bytes = 0;
            for version in 2..=17 {
                let mut edit = json!({"method":"textDocument/didChange","params":{
                    "textDocument":{"uri":"file:///large.ets","version":version},
                    "contentChanges":[{"range":{"start":{"line":0,"character":14},
                        "end":{"line":0,"character":15}},"text":(version % 10).to_string()}]
                }});
                reader.adapt(&mut edit).unwrap();
                queued_bytes += serde_json::to_vec(&edit).unwrap().len();
            }
            assert!(
                queued_bytes < 16 * 1024,
                "16 one-character edits queued {queued_bytes} bytes"
            );
            eprintln!(
                "binding={}: 16 edits queued {queued_bytes} bytes",
                !binding.is_empty()
            );
        }
    }

    #[test]
    fn sequential_edits_normalize_the_final_binding_view() {
        let mut reader = reader();
        reader
            .adapt(&mut json!({"method":"textDocument/didOpen","params":{
                "textDocument":{"uri":"file:///a.ets","text":"Text(this.value);"}
            }}))
            .unwrap();
        let mut change = json!({"method":"textDocument/didChange","params":{
            "textDocument":{"uri":"file:///a.ets","version":2},"contentChanges":[
                {"range":{"start":{"line":0,"character":5},"end":{"line":0,"character":5}},"text":"$$"},
                {"range":{"start":{"line":0,"character":12},"end":{"line":0,"character":17}},"text":"other"}
            ]
        }});
        reader.adapt(&mut change).unwrap();
        let mut compiler = "Text(this.value);".to_string();
        for edit in change["params"]["contentChanges"].as_array().unwrap() {
            let start = arkts_services::offset_at(&compiler, &edit["range"]["start"]).unwrap();
            let end = arkts_services::offset_at(&compiler, &edit["range"]["end"]).unwrap();
            compiler.replace_range(start..end, edit["text"].as_str().unwrap());
        }
        assert_eq!(compiler, "Text(  this.other);");
        assert_eq!(
            reader
                .state
                .lock()
                .unwrap()
                .document("file:///a.ets")
                .unwrap()
                .text,
            "Text($$this.other);"
        );
    }

    #[test]
    fn invalid_sdk_preserves_initialize_request_id() {
        let bytes = serde_json::to_vec(
            &json!({"jsonrpc":"2.0","id":123,"method":"initialize","params":{
                "capabilities":{}, "initializationOptions":{"ets":{"sdkPath":"/does-not-exist"}}
            }}),
        )
        .unwrap();
        let frame = [
            format!("Content-Length: {}\r\n\r\n", bytes.len()).into_bytes(),
            bytes,
        ]
        .concat();
        let fs = Arc::new(SdkFileSystem::new(
            Arc::new(tsrs_vfs::osvfs::fs()),
            PathBuf::from("/"),
        ));
        let mut reader = SdkReader::new(
            std::io::Cursor::new(frame),
            fs,
            Arc::new(Mutex::new(crate::middleware::HostState::default())),
        );
        let (message, error) = reader.read();
        assert!(error.unwrap().is_code(ErrorCode::InvalidParams));
        let value: Value =
            serde_json::from_str(&tsrs_lsproto::marshal(&message.unwrap()).unwrap()).unwrap();
        assert_eq!(value["id"], 123);
    }
}
