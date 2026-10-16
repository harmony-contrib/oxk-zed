//! SDK configuration shared by Zed and native language-service hosts.
use serde_json::{json, Map, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};

fn string_from(object: &Map<String, Value>, keys: &[&str]) -> Option<Value> {
    keys.iter().find_map(|key| {
        object
            .get(*key)
            .filter(|v| v.as_str().is_some_and(|s| !s.is_empty()))
            .cloned()
    })
}

fn aliases(target: &mut Map<String, Value>, source: &Map<String, Value>) {
    for (name, keys) in [
        (
            "sdkPath",
            &[
                "sdkPath",
                "SDK_HOME",
                "sdkHome",
                "ohosSdkPath",
                "OHOS_SDK_HOME",
            ][..],
        ),
        (
            "hmsPath",
            &[
                "hmsPath",
                "HMS_HOME",
                "hmsHome",
                "hmsSdkPath",
                "HMS_SDK_HOME",
            ][..],
        ),
    ] {
        if !target.contains_key(name) {
            if let Some(value) = string_from(source, keys) {
                target.insert(name.into(), value);
            }
        }
    }
}

/// Preserves the extension's existing aliases and precedence: ets, arkts, ohos, top level, environment.
pub fn normalize(mut options: Value, environment: &[(String, String)]) -> Value {
    let Some(object) = options.as_object_mut() else {
        return options;
    };
    let mut ets = object
        .get("ets")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let current_ets = ets.clone();
    aliases(&mut ets, &current_ets);
    for key in ["arkts", "ohos"] {
        if let Some(source) = object.get(key).and_then(Value::as_object) {
            aliases(&mut ets, source);
        }
    }
    aliases(&mut ets, object);
    let env = environment
        .iter()
        .filter(|(k, _)| {
            matches!(
                k.as_str(),
                "SDK_HOME" | "OHOS_SDK_HOME" | "HMS_HOME" | "HMS_SDK_HOME"
            )
        })
        .map(|(k, v)| (k.clone(), Value::String(v.clone())))
        .collect();
    aliases(&mut ets, &env);
    if !ets.is_empty() {
        object.insert("ets".into(), Value::Object(ets));
    }
    if let Some(tsdk) = string_from(object, &["tsdk", "TSDK", "typescriptSdkPath"]) {
        let mut typescript = object
            .get("typescript")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        typescript.entry("tsdk").or_insert(tsdk);
        object.insert("typescript".into(), Value::Object(typescript));
    }
    options
}

/// Defaults read from the selected SDK, plus explicit initialization compiler options.
#[derive(Clone, Debug, Default)]
pub struct SdkConfig {
    pub compiler_options: Value,
    pub declarations: Vec<String>,
    pub sdk_root: Option<PathBuf>,
}

pub fn parse_config(text: &str) -> Result<Value, String> {
    serde_json::from_str(text)
        .or_else(|_| json5::from_str(text))
        .map_err(|e| e.to_string())
}

fn read_json(path: &Path) -> Result<Value, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse_config(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn absolute(root: &Path, value: &str) -> PathBuf {
    let p = Path::new(value);
    if p.is_absolute() {
        p.into()
    } else {
        root.join(p)
    }
}

fn declarations(root: &Path, result: &mut Vec<String>) -> Result<(), String> {
    if !root.is_dir() {
        return Ok(());
    }
    let mut entries = fs::read_dir(root)
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        let path = entry.path();
        // Do not follow directory symlinks: SDK links can point back to an ancestor.
        if kind.is_dir() {
            declarations(&path, result)?;
        } else if path.is_file()
            && path
                .to_str()
                .is_some_and(|s| s.ends_with(".d.ts") || s.ends_with(".d.ets"))
        {
            result.push(path.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

/// Recursive objects use the higher-priority value; arrays/scalars are replaced.
pub fn merge(base: &mut Value, override_: &Value) {
    if let (Some(base), Some(high)) = (base.as_object_mut(), override_.as_object()) {
        for (key, value) in high {
            if let Some(current) = base.get_mut(key) {
                merge(current, value);
            } else {
                base.insert(key.clone(), value.clone());
            }
        }
    } else {
        *base = override_.clone();
    }
}

impl SdkConfig {
    /// Loads the same ets-loader grammar, component globals and SDK module roots as the previous server.
    pub fn load(options: &Value, cwd: &Path) -> Result<Self, String> {
        let mut config = Self {
            compiler_options: json!({}),
            ..Self::default()
        };
        if let Some(sdk) = options
            .pointer("/ets/sdkPath")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            let sdk = absolute(cwd, sdk);
            let ets = if sdk.file_name().is_some_and(|s| s == "ets") {
                sdk.clone()
            } else {
                sdk.join("ets")
            };
            let loader = ets.join("build-tools/ets-loader");
            let sdk_config = read_json(&loader.join("tsconfig.json"))?;
            config.compiler_options = sdk_config
                .get("compilerOptions")
                .cloned()
                .unwrap_or_else(|| json!({}));
            // Build-only output and TypeScript 4.9 options do not apply to the native editor service.
            for key in [
                "importsNotUsedAsValues",
                "sourceMap",
                "outDir",
                "rootDir",
                "declaration",
                "types",
                "typeRoots",
            ] {
                config
                    .compiler_options
                    .as_object_mut()
                    .ok_or("SDK compilerOptions must be an object")?
                    .remove(key);
            }
            let path = |p: PathBuf| p.to_string_lossy().replace('\\', "/");
            merge(
                &mut config.compiler_options,
                &json!({
                    "etsLoaderPath": path(loader.clone()), "target": "esnext", "module": "esnext",
                    "moduleResolution": "node", "moduleDetection": "force", "noEmit": true,
                    "strict": true, "strictPropertyInitialization": false, "experimentalDecorators": true,
                    "skipLibCheck": true, "etsAnnotationsEnable": true, "packageManagerType": "ohpm", "needDoArkTsLinter": true,
                    "compatibleSdkVersion": 20, "compatibleSdkVersionStage": "beta2",
                    "alwaysStrict": true, "mixCompile": true, "tsImportSendableEnable": true,
                    "useUnknownInCatchVariables": false, "baseUrl": path(ets.clone()),
                    "paths": {"*": [path(ets.join("api/*")), path(ets.join("kits/*")), path(ets.join("arkts/*"))],
                              "@internal/full/*": [path(ets.join("api/@internal/full/*"))]},
                    "ohSdkConfigs": [{"apiPaths": [path(ets.join("api")), path(ets.join("kits")), path(ets.join("arkts"))]}]
                }),
            );
            if config
                .compiler_options
                .pointer("/ets/syntaxComponents")
                .is_none()
            {
                config.compiler_options["ets"]["syntaxComponents"] = json!({"paramsUICallback": ["ForEach", "LazyForEach"],
                    "attrUICallback": [{"name": "Repeat", "attributes": ["each", "template"]}]});
            }
            declarations(&ets.join("component"), &mut config.declarations)?;
            declarations(&loader.join("declarations"), &mut config.declarations)?;
            config.sdk_root = Some(sdk);
        }
        if let Some(hms) = options
            .pointer("/ets/hmsPath")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            let hms = absolute(cwd, hms);
            if !hms.is_dir() {
                return Err(format!("HMS SDK is not a directory: {}", hms.display()));
            }
            let ets = if hms.file_name().is_some_and(|s| s == "ets") {
                hms
            } else {
                hms.join("ets")
            };
            for sub in ["api", "kits"] {
                let dir = ets.join(sub);
                if !dir.is_dir() {
                    continue;
                }
                for entry in fs::read_dir(&dir).map_err(|e| e.to_string())? {
                    let p = entry.map_err(|e| e.to_string())?.path();
                    if !p.is_file() {
                        continue;
                    }
                    let name = p.file_name().unwrap().to_string_lossy();
                    if let Some(name) = name
                        .strip_suffix(".d.ts")
                        .or_else(|| name.strip_suffix(".d.ets"))
                    {
                        if config.compiler_options.get("paths").is_none() {
                            config.compiler_options["paths"] = json!({});
                        }
                        config.compiler_options["paths"][name] = json!([p.to_string_lossy()]);
                    }
                }
            }
        }
        if let Some(tsdk) = options
            .pointer("/typescript/tsdk")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            let tsdk = absolute(cwd, tsdk);
            let libs = if tsdk.join("lib").is_dir() {
                tsdk.join("lib")
            } else {
                tsdk
            };
            let before = config.declarations.len();
            declarations(&libs, &mut config.declarations)?;
            if config.sdk_root.is_some() {
                // Loading every TSDK declaration includes DOM and WebWorker globals,
                // whose Text/Image/console symbols conflict with ArkUI's SDK globals.
                let standard: Vec<_> = config
                    .declarations
                    .drain(before..)
                    .filter(|path| {
                        Path::new(path)
                            .file_name()
                            .and_then(|name| name.to_str())
                            .is_some_and(|name| {
                                (name.starts_with("lib.es") && !name.ends_with(".full.d.ts"))
                                    || name.starts_with("lib.decorators")
                            })
                    })
                    .collect();
                config.declarations.extend(standard);
            }
            if config.declarations.len() > before {
                config.compiler_options["noLib"] = json!(true);
            }
        }
        if let Some(overrides) = options.get("compilerOptions") {
            if !overrides.is_object() {
                return Err("compilerOptions must be an object".into());
            }
            merge(&mut config.compiler_options, overrides);
        }
        config.declarations.sort();
        config.declarations.dedup();
        Ok(config)
    }

    /// Adapts a project config without changing it on disk. Project options override SDK defaults;
    /// initialization compilerOptions remain the highest priority.
    pub fn adapt_tsconfig(&self, text: &str, explicit: &Value) -> Result<String, String> {
        let mut config = parse_config(text)?;
        if !config.is_object() {
            return Err("tsconfig must be an object".into());
        }
        let mut compiler = self.compiler_options.clone();
        if let Some(project) = config.get("compilerOptions") {
            merge(&mut compiler, project);
        }
        if let Some(overrides) = explicit.get("compilerOptions") {
            merge(&mut compiler, overrides);
        }
        config["compilerOptions"] = compiler;
        if !self.declarations.is_empty() {
            // Adding files changes TS's default include; preserve that default explicitly.
            if config.get("files").is_none()
                && config.get("include").is_none()
                && config.get("extends").is_none()
            {
                config["include"] = json!(["**/*"]);
            }
            let files = config
                .as_object_mut()
                .ok_or("tsconfig must be an object")?
                .entry("files")
                .or_insert_with(|| json!([]));
            let files = files
                .as_array_mut()
                .ok_or("tsconfig files must be an array")?;
            for path in &self.declarations {
                if !files.iter().any(|f| f.as_str() == Some(path)) {
                    files.push(json!(path));
                }
            }
        }
        serde_json::to_string(&config).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aliases_preserve_precedence_and_unknown_options() {
        let options = normalize(
            json!({"ets":{"SDK_HOME":"/first"},"arkts":{"sdkPath":"/second", "HMS_HOME":"/hms"},
            "tsdk":"/ts", "userPreferences":{"quotePreference":"single"}}),
            &[("SDK_HOME".into(), "/env".into())],
        );
        assert_eq!(options["ets"]["sdkPath"], "/first");
        assert_eq!(options["ets"]["hmsPath"], "/hms");
        assert_eq!(options["typescript"]["tsdk"], "/ts");
        assert_eq!(options["userPreferences"]["quotePreference"], "single");
        assert_eq!(normalize(Value::Null, &[]), Value::Null);
    }
    #[test]
    fn sdk_config_and_jsonc_project_are_adapted_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("ets/build-tools/ets-loader")).unwrap();
        fs::create_dir_all(dir.path().join("ets/component")).unwrap();
        fs::write(
            dir.path().join("ets/build-tools/ets-loader/tsconfig.json"),
            r#"{"compilerOptions":{"ets":{"components":["Text"]}}}"#,
        )
        .unwrap();
        fs::write(
            dir.path().join("ets/component/text.d.ets"),
            "declare function Text(): void;",
        )
        .unwrap();
        let options = json!({"ets":{"sdkPath":dir.path()}, "compilerOptions":{"strict":false}});
        let sdk = SdkConfig::load(&options, dir.path()).unwrap();
        let adapted: Value = serde_json::from_str(&sdk.adapt_tsconfig("{ // comment\n compilerOptions: {strict: true, paths: {'local/*': ['./src/*']}}, }", &options).unwrap()).unwrap();
        assert_eq!(adapted["compilerOptions"]["strict"], false);
        assert_eq!(adapted["compilerOptions"]["paths"]["local/*"][0], "./src/*");
        assert_eq!(adapted["compilerOptions"]["ets"]["components"][0], "Text");
        assert_eq!(adapted["files"].as_array().unwrap().len(), 1);
        assert_eq!(adapted["include"][0], "**/*");
        assert!(!dir.path().join("tsconfig.json").exists());
    }
    #[test]
    fn bad_sdk_is_an_error() {
        assert!(SdkConfig::load(
            &json!({"ets":{"sdkPath":"/does-not-exist"}}),
            Path::new("/")
        )
        .is_err());
        assert!(SdkConfig::load(&json!({"compilerOptions":false}), Path::new("/")).is_err());
        assert!(SdkConfig::default()
            .adapt_tsconfig("null", &json!({}))
            .is_err());
    }
}
