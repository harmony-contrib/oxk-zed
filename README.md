# ArkTS for Zed

Zed extension for ArkTS (`.ets`) files.

## Features

- Syntax highlighting via `harmony-contrib/tree-sitter-arkts`
- ArkTS language features through `@arkts/language-server`
- Formatting through `@ohos-rs/oxk format --lsp` for ArkTS and the Oxfmt-compatible languages registered by the extension
- ArkTS lint diagnostics and code actions through `@ohos-rs/oxk lint --lsp`

## Setup

Install this directory as a Zed dev extension. The Rust extension installs `@arkts/language-server` for IDE features and `@ohos-rs/oxk` for formatting/linting when the corresponding LSP starts.

Use `arkts-language-server` for the main language server, `oxk-formatter` as the formatter, and keep `oxk-linter` enabled for diagnostics:

```json
{
  "lsp": {
    "arkts-language-server": {
      "settings": {
        "arkts": {
          "SDK_HOME": "/path/to/OpenHarmony/20",
          "HMS_HOME": "/path/to/HarmonyOS/hms"
        },
        "typescript": {
          "tsdk": "/path/to/typescript/lib"
        }
      }
    },
    "oxk-formatter": {
      "settings": {
        "fmt.configPath": ".oxfmtrc.jsonc"
      }
    },
    "oxk-linter": {
      "settings": {
        "run": "onType",
        "configPath": ".oxlintrc.jsonc",
        "tsConfigPath": "tsconfig.json",
        "fixKind": "safe_fix_or_suggestion"
      }
    }
  },
  "languages": {
    "ArkTS": {
      "language_servers": [
        "arkts-language-server",
        "oxk-linter",
        "oxk-formatter"
      ],
      "formatter": { "language_server": { "name": "oxk-formatter" } }
    }
  }
}
```

`arkts-language-server` settings are normalized before being passed to `@arkts/language-server`. The server itself requires `ets.sdkPath`, but the extension also accepts `arkts.SDK_HOME`, `arkts.sdkPath`, top-level `SDK_HOME`, `ohos.sdkPath`, and the shell environment variable `SDK_HOME`. `HMS_HOME`/`hmsPath` are normalized to `ets.hmsPath`.

`oxk-formatter` is registered for `ArkTS`, `JavaScript`, `JSX`, `TypeScript`, `TSX`, `Vue.js`, `JSON`, `JSON5`, `JSONC`, `HTML`, `CSS`, `SCSS`, `LESS`, `Angular`, `GraphQL`, `Handlebars`, `Markdown`, `MDX`, `YAML`, and `TOML`.

`fmt.configPath`, `configPath`, `tsConfigPath`, `run`, and `fixKind` are oxk/oxc language-server options. ArkTS lint rules are configured through `.oxlintrc.*`; rules are opt-in unless enabled by that config.

## LICENSE

[MIT](./LICENSE)
