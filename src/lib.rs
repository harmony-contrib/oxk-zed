use std::env;

use zed::{
    serde_json::{json, Value},
    settings::LspSettings,
    LanguageServerId, Worktree,
};
use zed_extension_api as zed;

const ARKTS_SERVER_ID: &str = "arkts-language-server";
const FORMATTER_SERVER_ID: &str = "oxk-formatter";
const LINTER_SERVER_ID: &str = "oxk-linter";
const OXK_PACKAGE: &str = "@ohos-rs/oxk";
const OXK_BIN: &str = "node_modules/@ohos-rs/oxk/bin/oxk.js";
const LOCAL_SERVER: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/target/release/arkts-lsp");

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

    fn arkts_language_server_command(
        language_server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> zed::Result<zed::Command> {
        let args = vec!["--stdio".to_string()];
        if let Some(command) =
            Self::binary_command_from_settings(language_server_id, worktree, args.clone())
        {
            return Ok(command);
        }
        let env = Self::shell_env_for(worktree);
        let command = env.iter().find(|(key, _)| key == "ARKTS_LSP_PATH")
            .map(|(_, value)| value.clone())
            .or_else(|| worktree.which("arkts-lsp"))
            .or_else(|| worktree.which(LOCAL_SERVER))
            .ok_or_else(|| "arkts-lsp was not found. Build with scripts/build-local.sh, put arkts-lsp on PATH, or set lsp.arkts-language-server.binary.path.".to_string())?;
        Ok(zed::Command { command, args, env })
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

    fn normalize_arkts_initialization_options(options: Value, worktree: &Worktree) -> Value {
        arkts_sdk::normalize(options, &Self::shell_env_for(worktree))
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
            unknown => Err(format!("unknown language server: {unknown}")),
        }
    }

    fn language_server_initialization_options(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &Worktree,
    ) -> zed::Result<Option<Value>> {
        let settings = Self::settings_for(language_server_id, worktree).unwrap_or_default();

        if let Some(initialization_options) = settings.initialization_options {
            if language_server_id.as_ref() == ARKTS_SERVER_ID {
                return Ok(Some(Self::normalize_arkts_initialization_options(
                    initialization_options,
                    worktree,
                )));
            }
            return Ok(Some(initialization_options));
        }

        if language_server_id.as_ref() == ARKTS_SERVER_ID {
            return Ok(Some(Self::normalize_arkts_initialization_options(
                settings.settings.unwrap_or_else(|| json!({})),
                worktree,
            )));
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
        let settings = Self::settings_for(language_server_id, worktree).unwrap_or_default();

        if language_server_id.as_ref() == ARKTS_SERVER_ID {
            return Ok(Some(Self::normalize_arkts_initialization_options(
                settings.settings.unwrap_or_else(|| json!({})),
                worktree,
            )));
        }

        Ok(settings.settings)
    }
}

zed::register_extension!(OxkExtension);
