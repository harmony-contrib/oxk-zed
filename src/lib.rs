use std::{env, fs, path::Path};

use zed::{
    serde_json::{json, Map, Value},
    settings::LspSettings,
    LanguageServerId, Worktree,
};
use zed_extension_api as zed;

const ARKTS_SERVER_ID: &str = "arkts-language-server";
const FORMATTER_SERVER_ID: &str = "oxk-formatter";
const LINTER_SERVER_ID: &str = "oxk-linter";
const ARKTS_PACKAGE: &str = "@arkts/language-server";
const OXK_PACKAGE: &str = "@ohos-rs/oxk";
const ARKTS_SERVER_BIN: &str = "node_modules/@arkts/language-server/bin/ets-language-server.js";
const ARKTS_WRAPPER_BIN: &str = "server/arkts-lsp-wrapper.js";
const OXK_BIN: &str = "node_modules/@ohos-rs/oxk/bin/oxk.js";
const ARKTS_WRAPPER_SOURCE: &str = include_str!("../server/arkts-lsp-wrapper.js");

struct OxkExtension;

impl OxkExtension {
    fn current_dir_path(path: &str) -> zed::Result<String> {
        let current_dir = env::current_dir()
            .map_err(|error| format!("failed to resolve extension directory: {error}"))?;
        Ok(current_dir.join(path).to_string_lossy().to_string())
    }

    fn ensure_npm_package(
        language_server_id: &LanguageServerId,
        package_name: &str,
    ) -> zed::Result<()> {
        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::CheckingForUpdate,
        );

        let latest_version = match zed::npm_package_latest_version(package_name) {
            Ok(version) => version,
            Err(error) => {
                zed::set_language_server_installation_status(
                    language_server_id,
                    &zed::LanguageServerInstallationStatus::Failed(error.clone()),
                );
                return Err(format!(
                    "failed to fetch latest {package_name} version: {error}"
                ));
            }
        };

        let installed_version = zed::npm_package_installed_version(package_name)?;
        if installed_version.as_deref() != Some(latest_version.as_str()) {
            zed::set_language_server_installation_status(
                language_server_id,
                &zed::LanguageServerInstallationStatus::Downloading,
            );
            zed::npm_install_package(package_name, &latest_version)?;
        }

        zed::set_language_server_installation_status(
            language_server_id,
            &zed::LanguageServerInstallationStatus::None,
        );
        Ok(())
    }

    fn settings_for(
        language_server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> Option<LspSettings> {
        LspSettings::for_worktree(language_server_id.as_ref(), worktree).ok()
    }

    fn shell_env_for(worktree: &Worktree) -> Vec<(String, String)> {
        match zed::current_platform().0 {
            zed::Os::Mac | zed::Os::Linux => worktree.shell_env(),
            zed::Os::Windows => Vec::new(),
        }
    }

    fn binary_command_from_settings(
        language_server_id: &LanguageServerId,
        worktree: &Worktree,
        default_args: Vec<String>,
    ) -> Option<zed::Command> {
        let binary = Self::settings_for(language_server_id, worktree)?.binary?;
        let path = binary.path?;

        Some(zed::Command {
            command: path,
            args: binary.arguments.unwrap_or(default_args),
            env: Self::shell_env_for(worktree),
        })
    }

    fn ensure_arkts_wrapper() -> zed::Result<String> {
        let wrapper_path = Self::current_dir_path(ARKTS_WRAPPER_BIN)?;
        let path = Path::new(&wrapper_path);

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "failed to create ArkTS language server wrapper directory {}: {error}",
                    parent.display()
                )
            })?;
        }

        let should_write = fs::read_to_string(path)
            .map(|current| current != ARKTS_WRAPPER_SOURCE)
            .unwrap_or(true);

        if should_write {
            fs::write(path, ARKTS_WRAPPER_SOURCE).map_err(|error| {
                format!(
                    "failed to write ArkTS language server wrapper {}: {error}",
                    path.display()
                )
            })?;
        }

        Ok(wrapper_path)
    }

    fn arkts_language_server_command(
        language_server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> zed::Result<zed::Command> {
        if let Some(command) =
            Self::binary_command_from_settings(language_server_id, worktree, Vec::new())
        {
            return Ok(command);
        }

        Self::ensure_npm_package(language_server_id, ARKTS_PACKAGE)?;

        let wrapper_path = Self::ensure_arkts_wrapper()?;
        let server_path = Self::current_dir_path(ARKTS_SERVER_BIN)?;
        let mut env = Self::shell_env_for(worktree);
        env.push(("ARKTS_LANGUAGE_SERVER_PATH".to_string(), server_path));

        Ok(zed::Command {
            command: zed::node_binary_path()?,
            args: vec![wrapper_path],
            env,
        })
    }

    fn oxk_language_server_command(
        language_server_id: &LanguageServerId,
        worktree: &Worktree,
        subcommand: &str,
    ) -> zed::Result<zed::Command> {
        let default_args = vec![subcommand.to_string(), "--lsp".to_string()];
        if let Some(command) =
            Self::binary_command_from_settings(language_server_id, worktree, default_args.clone())
        {
            return Ok(command);
        }

        Self::ensure_npm_package(language_server_id, OXK_PACKAGE)?;

        Ok(zed::Command {
            command: zed::node_binary_path()?,
            args: {
                let mut args = vec![Self::current_dir_path(OXK_BIN)?];
                args.extend(default_args);
                args
            },
            env: Self::shell_env_for(worktree),
        })
    }

    fn value_from_map(object: &Map<String, Value>, keys: &[&str]) -> Option<Value> {
        keys.iter().find_map(|key| {
            object
                .get(*key)
                .filter(|value| value.as_str().is_some_and(|value| !value.is_empty()))
                .cloned()
        })
    }

    fn merge_arkts_sdk_aliases(target: &mut Map<String, Value>, source: &Map<String, Value>) {
        if !target.contains_key("sdkPath") {
            if let Some(value) = Self::value_from_map(
                source,
                &[
                    "sdkPath",
                    "SDK_HOME",
                    "sdkHome",
                    "ohosSdkPath",
                    "OHOS_SDK_HOME",
                ],
            ) {
                target.insert("sdkPath".to_string(), value);
            }
        }

        if !target.contains_key("hmsPath") {
            if let Some(value) = Self::value_from_map(
                source,
                &[
                    "hmsPath",
                    "HMS_HOME",
                    "hmsHome",
                    "hmsSdkPath",
                    "HMS_SDK_HOME",
                ],
            ) {
                target.insert("hmsPath".to_string(), value);
            }
        }
    }

    fn shell_env_aliases(worktree: &Worktree) -> Map<String, Value> {
        Self::shell_env_for(worktree)
            .into_iter()
            .filter_map(|(key, value)| match key.as_str() {
                "SDK_HOME" | "OHOS_SDK_HOME" | "HMS_HOME" | "HMS_SDK_HOME" => {
                    Some((key, Value::String(value)))
                }
                _ => None,
            })
            .collect()
    }

    fn normalize_arkts_initialization_options(mut options: Value, worktree: &Worktree) -> Value {
        let Some(object) = options.as_object_mut() else {
            return options;
        };

        let mut ets = object
            .get("ets")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();

        let current_ets = ets.clone();
        Self::merge_arkts_sdk_aliases(&mut ets, &current_ets);

        for key in ["arkts", "ohos"] {
            if let Some(source) = object.get(key).and_then(Value::as_object) {
                Self::merge_arkts_sdk_aliases(&mut ets, source);
            }
        }

        Self::merge_arkts_sdk_aliases(&mut ets, object);
        Self::merge_arkts_sdk_aliases(&mut ets, &Self::shell_env_aliases(worktree));

        if !ets.is_empty() {
            object.insert("ets".to_string(), Value::Object(ets));
        }

        if let Some(tsdk) = Self::value_from_map(object, &["tsdk", "TSDK", "typescriptSdkPath"]) {
            let mut typescript = object
                .get("typescript")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            typescript.entry("tsdk".to_string()).or_insert(tsdk);
            object.insert("typescript".to_string(), Value::Object(typescript));
        }

        options
    }
}

impl zed::Extension for OxkExtension {
    fn new() -> Self {
        Self
    }

    fn language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> zed::Result<zed::Command> {
        match language_server_id.as_ref() {
            ARKTS_SERVER_ID => Self::arkts_language_server_command(language_server_id, worktree),
            FORMATTER_SERVER_ID => {
                Self::oxk_language_server_command(language_server_id, worktree, "format")
            }
            LINTER_SERVER_ID => {
                Self::oxk_language_server_command(language_server_id, worktree, "lint")
            }
            unknown => return Err(format!("unknown language server: {unknown}")),
        }
    }

    fn language_server_initialization_options(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> zed::Result<Option<Value>> {
        let Some(settings) = Self::settings_for(language_server_id, worktree) else {
            return Ok(None);
        };

        if settings.initialization_options.is_some() {
            let initialization_options = settings.initialization_options.unwrap();
            if language_server_id.as_ref() == ARKTS_SERVER_ID {
                return Ok(Some(Self::normalize_arkts_initialization_options(
                    initialization_options,
                    worktree,
                )));
            }
            return Ok(Some(initialization_options));
        }

        if language_server_id.as_ref() == ARKTS_SERVER_ID {
            return Ok(settings
                .settings
                .map(|settings| Self::normalize_arkts_initialization_options(settings, worktree)));
        }

        Ok(settings
            .settings
            .map(|settings| json!({ "settings": settings })))
    }

    fn language_server_workspace_configuration(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> zed::Result<Option<Value>> {
        let Some(settings) = Self::settings_for(language_server_id, worktree) else {
            return Ok(None);
        };

        if language_server_id.as_ref() == ARKTS_SERVER_ID {
            return Ok(settings
                .settings
                .map(|settings| Self::normalize_arkts_initialization_options(settings, worktree)));
        }

        Ok(settings.settings)
    }
}

zed::register_extension!(OxkExtension);
