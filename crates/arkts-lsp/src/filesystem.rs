use arkts_sdk::SdkConfig;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::SystemTime,
};
use tsrs_vfs::{Entries, FileInfo, FS};

const SDK_BASE: &str = ".arkts-lsp-sdk.json";

#[derive(Default)]
struct Configuration {
    roots: Vec<PathBuf>,
    sdk: SdkConfig,
    options: Value,
}

/// Read-through filesystem that supplies SDK options and globals to project configs.
/// Writes always go to the host filesystem. Virtual tsconfig files are never written.
pub struct SdkFileSystem {
    inner: Arc<dyn FS>,
    cwd: PathBuf,
    configuration: RwLock<Configuration>,
}

impl SdkFileSystem {
    pub fn new(inner: Arc<dyn FS>, cwd: PathBuf) -> Self {
        Self {
            inner,
            cwd,
            configuration: RwLock::new(Configuration::default()),
        }
    }

    pub fn configure(
        &self,
        mut options: Value,
        roots: Option<Vec<PathBuf>>,
    ) -> Result<Vec<PathBuf>, String> {
        // Embedded hosts can supply a cwd different from the process working directory.
        for pointer in ["/ets/sdkPath", "/ets/hmsPath", "/typescript/tsdk"] {
            if let Some(value) = options.pointer_mut(pointer) {
                if let Some(path) = value.as_str().filter(|s| !s.is_empty()) {
                    if Path::new(path).is_relative() {
                        *value = json!(self.cwd.join(path));
                    }
                }
            }
        }
        let sdk = SdkConfig::load(&options, &self.cwd)?;
        let mut state = self.configuration.write().map_err(|e| e.to_string())?;
        if let Some(roots) = roots {
            state.roots = roots;
        }
        state.sdk = sdk;
        state.options = options;
        Ok(state
            .roots
            .iter()
            .flat_map(|r| [r.join("tsconfig.json"), r.join(SDK_BASE)])
            .collect())
    }

    pub(crate) fn configuration(&self) -> (Value, Vec<PathBuf>) {
        let state = self.configuration.read().unwrap();
        (state.options.clone(), state.roots.clone())
    }

    fn is_virtual(&self, path: &str, state: &Configuration) -> bool {
        (!state
            .sdk
            .compiler_options
            .as_object()
            .is_none_or(|o| o.is_empty())
            || !state.sdk.declarations.is_empty())
            && state.roots.iter().any(|root| {
                ["tsconfig.json", SDK_BASE]
                    .iter()
                    .any(|name| root.join(name) == Path::new(path))
            })
    }

    fn in_workspace(&self, path: &str, state: &Configuration) -> bool {
        state
            .roots
            .iter()
            .any(|root| Path::new(path).starts_with(root))
            && !state
                .sdk
                .sdk_root
                .as_ref()
                .is_some_and(|root| Path::new(path).starts_with(root))
    }

    fn adapt_project(
        &self,
        path: &str,
        text: &str,
        state: &Configuration,
    ) -> Result<String, String> {
        let parsed = arkts_sdk::parse_config(text)?;
        if parsed.get("extends").is_none()
            || (state
                .sdk
                .compiler_options
                .as_object()
                .is_none_or(|o| o.is_empty())
                && state.sdk.declarations.is_empty())
        {
            return state.sdk.adapt_tsconfig(text, &state.options);
        }
        // Let tsrs resolve inheritance. SDK defaults form the lowest-priority base;
        // placing those defaults directly in the child would override project bases.
        let mut sdk = state.sdk.clone();
        sdk.compiler_options = json!({});
        let mut adapted: Value = serde_json::from_str(&sdk.adapt_tsconfig(text, &state.options)?)
            .map_err(|e| e.to_string())?;
        if let Some(root) = state
            .roots
            .iter()
            .filter(|root| Path::new(path).starts_with(root))
            .max_by_key(|root| root.components().count())
        {
            let mut bases = vec![json!(root.join(SDK_BASE))];
            match adapted["extends"].clone() {
                Value::String(base) => bases.push(json!(base)),
                Value::Array(base) => bases.extend(base),
                _ => return Err("tsconfig extends must be a string or array".into()),
            }
            adapted["extends"] = json!(bases);
        }
        serde_json::to_string(&adapted).map_err(|e| e.to_string())
    }
}

impl FS for SdkFileSystem {
    fn use_case_sensitive_file_names(&self) -> bool {
        self.inner.use_case_sensitive_file_names()
    }
    fn file_exists(&self, path: &str) -> bool {
        self.inner.file_exists(path) || self.is_virtual(path, &self.configuration.read().unwrap())
    }
    fn read_file(&self, path: &str) -> Option<String> {
        let state = self.configuration.read().unwrap();
        if Path::new(path).file_name().is_some_and(|s| s == SDK_BASE)
            && self.is_virtual(path, &state)
        {
            return Some(json!({"compilerOptions":state.sdk.compiler_options}).to_string());
        }
        let original = self.inner.read_file(path);
        if Path::new(path)
            .file_name()
            .is_some_and(|s| s == "tsconfig.json" || s == "jsconfig.json")
            && self.in_workspace(path, &state)
        {
            if let Some(text) = original.as_deref() {
                // A malformed project config must retain tsrs's own diagnostics.
                return Some(
                    self.adapt_project(path, text, &state)
                        .unwrap_or_else(|_| text.to_string()),
                );
            }
            if self.is_virtual(path, &state) {
                return state.sdk.adapt_tsconfig(&json!({"include":["**/*"],"exclude":["node_modules", "oh_modules", ".git", "build"]}).to_string(), &state.options).ok();
            }
        }
        original.map(|text| {
            if path.ends_with(".ets") && text.contains("$$this") {
                arkts_services::Document::parse(text)
                    .tsrs_text()
                    .into_owned()
            } else {
                text
            }
        })
    }
    fn get_accessible_entries(&self, path: &str) -> Entries {
        let mut entries = self.inner.get_accessible_entries(path);
        if self.is_virtual(
            &Path::new(path).join("tsconfig.json").to_string_lossy(),
            &self.configuration.read().unwrap(),
        ) && !entries.files.iter().any(|name| name == "tsconfig.json")
        {
            entries.files.push("tsconfig.json".into());
        }
        entries
    }
    fn stat(&self, path: &str) -> Option<FileInfo> {
        self.inner.stat(path).or_else(|| {
            self.is_virtual(path, &self.configuration.read().unwrap())
                .then(|| FileInfo {
                    name: Path::new(path)
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    size: 0,
                    mode: tsrs_vfs::FileMode::Perm,
                    mod_time: None,
                })
        })
    }
    fn directory_exists(&self, path: &str) -> bool {
        self.inner.directory_exists(path)
    }
    fn realpath(&self, path: &str) -> String {
        self.inner.realpath(path)
    }
    fn write_file(&self, path: &str, data: &str) -> Result<(), String> {
        self.inner.write_file(path, data)
    }
    fn append_file(&self, path: &str, data: &str) -> Result<(), String> {
        self.inner.append_file(path, data)
    }
    fn remove(&self, path: &str) -> Result<(), String> {
        self.inner.remove(path)
    }
    fn chtimes(&self, path: &str, a: SystemTime, m: SystemTime) -> Result<(), String> {
        self.inner.chtimes(path, a, m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn virtual_config_is_visible_and_never_written() {
        let dir = tempfile::tempdir().unwrap();
        let fs = SdkFileSystem::new(Arc::new(tsrs_vfs::osvfs::fs()), dir.path().into());
        fs.configure(
            json!({"compilerOptions":{"strict":true}}),
            Some(vec![dir.path().into()]),
        )
        .unwrap();
        let path = dir.path().join("tsconfig.json");
        let name = path.to_str().unwrap();
        assert!(fs.file_exists(name));
        assert_eq!(
            serde_json::from_str::<Value>(&fs.read_file(name).unwrap()).unwrap()["compilerOptions"]
                ["strict"],
            true
        );
        assert!(fs
            .get_accessible_entries(dir.path().to_str().unwrap())
            .files
            .contains(&"tsconfig.json".into()));
        assert!(!path.exists());
        fs.configure(json!({}), None).unwrap();
        assert!(!fs.file_exists(name));
    }
    #[test]
    fn project_inheritance_is_above_sdk_defaults() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("tsconfig.json"),
            r#"{"extends":"./base.json","compilerOptions":{"strict":false}}"#,
        )
        .unwrap();
        let fs = SdkFileSystem::new(Arc::new(tsrs_vfs::osvfs::fs()), dir.path().into());
        fs.configure(
            json!({"compilerOptions":{"skipLibCheck":true}}),
            Some(vec![dir.path().into()]),
        )
        .unwrap();
        let config: Value = serde_json::from_str(
            &fs.read_file(dir.path().join("tsconfig.json").to_str().unwrap())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(config["extends"][1], "./base.json");
        assert_eq!(config["compilerOptions"]["strict"], false);
        let base = config["extends"][0].as_str().unwrap();
        assert!(fs.file_exists(base));
        assert!(!Path::new(base).exists());
        assert_eq!(
            serde_json::from_str::<Value>(&fs.read_file(base).unwrap()).unwrap()["compilerOptions"]
                ["skipLibCheck"],
            true
        );
    }
    #[test]
    fn no_sdk_retains_project_inheritance_and_bad_configs() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("tsconfig.json");
        std::fs::write(&config, r#"{"extends":"./base.json"}"#).unwrap();
        let fs = SdkFileSystem::new(Arc::new(tsrs_vfs::osvfs::fs()), dir.path().into());
        fs.configure(json!({}), Some(vec![dir.path().into()]))
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&fs.read_file(config.to_str().unwrap()).unwrap())
                .unwrap()["extends"],
            "./base.json"
        );
        std::fs::write(&config, "null").unwrap();
        assert_eq!(fs.read_file(config.to_str().unwrap()).unwrap(), "null");
    }
}
