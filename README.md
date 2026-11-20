# ArkTS for Zed

ArkTS (`.ets`) support for Zed, powered by a native language server built on **tsrs** and **OXC**:

- Syntax highlighting from [`harmony-contrib/tree-sitter-arkts`](https://github.com/harmony-contrib/tree-sitter-arkts).
- Completion, hover, navigation, references, rename, symbols, signature help, highlights, folding, inlay hints, semantic tokens, code actions, and type diagnostics from one Rust process.
- ArkUI declarations and `$r` resource completion, hover, go-to-definition and diagnostics from an installed OpenHarmony or HarmonyOS SDK, including `sys.*` entries from both `ets-loader` and HMS `sysResource.js` tables.
- JSON5 configuration resources: `$type:name` completion, diagnostics and definitions in `module.json5`/`app.json5`, path completion and links, and `requestPermissions` completion/hover.
- Formatting and linting through `@ohos-rs/oxk` (`oxk-formatter` and `oxk-linter`).
- An optional **OXK Symbols** file icon theme with an ArkTS `.ets` icon.

## Install

The extension is installed as a Zed dev extension from this checkout.

### Prerequisites

- Rust 1.100.0 with the `wasm32-wasip2` target. `rust-toolchain.toml` pins both; tsrs requires Rust 1.100 or newer.
- An OpenHarmony or HarmonyOS SDK for ArkUI globals and system resources. Plain TypeScript/ETS editing works without one.
- `bash` for `scripts/build-local.sh` (macOS/Linux). Zed's bundled Node runs the formatter and linter launchers.

### Build

```sh
scripts/build-local.sh
cargo run --example validate-extension -- extension.wasm
```

This produces `target/release/arkts-lsp` (native language server) and `extension.wasm` (Zed extension).

### Install in Zed

1. Run **zed: install dev extension** from the command palette.
2. Select this repository.
3. Point the language server at the native binary, preferably with an absolute path:

```json
{
  "lsp": {
    "arkts-language-server": {
      "binary": {
        "path": "/absolute/path/to/oxk-zed/target/release/arkts-lsp",
        "arguments": ["--stdio"]
      },
      "settings": {
        "ets": {
          "sdkPath": "/path/to/OpenHarmony/20",
          "hmsPath": "/path/to/HarmonyOS/hms"
        }
      }
    }
  }
}
```

`binary.path` can be omitted when `arkts-lsp` is on `PATH`, `ARKTS_LSP_PATH` is set, or the binary remains at `target/release/arkts-lsp` in the checkout used to build the extension. There is no automatic native binary download, so use an explicit path if the checkout moves.

### Enable formatting and linting

`oxk-formatter` and `oxk-linter` are registered for ArkTS. Enable them for the language:

```json
{
  "languages": {
    "ArkTS": {
      "language_servers": ["arkts-language-server", "oxk-linter", "oxk-formatter"],
      "formatter": { "language_server": { "name": "oxk-formatter" } }
    }
  },
  "lsp": {
    "oxk-formatter": {
      "settings": { "fmt.configPath": ".oxfmtrc.jsonc" }
    },
    "oxk-linter": {
      "settings": {
        "run": "onType",
        "configPath": ".oxlintrc.jsonc",
        "tsConfigPath": "tsconfig.json",
        "fixKind": "safe_fix_or_suggestion"
      }
    }
  }
}
```

Zed downloads `@ohos-rs/oxk` on demand. Both servers accept `binary.path` / `binary.arguments` overrides.

### Optional: icon theme

Select the bundled theme to get ArkTS file icons in the project panel, tabs, and language picker:

```json
{
  "icon_theme": "OXK Symbols"
}
```

Keep `file_finder.file_icons` enabled (the default). Zed derives language-picker icons from the active icon theme and the language's file suffix, so `.ets` shows the ArkTS icon only while this theme is selected. See [File icon theme](#file-icon-theme) for details.

## Configuration

The main server accepts `ets.sdkPath` and previous aliases: `arkts.SDK_HOME`, `arkts.sdkPath`, top-level `SDK_HOME`, `ohos.sdkPath`, and shell `SDK_HOME`/`OHOS_SDK_HOME`. HMS aliases map to `ets.hmsPath`. Precedence is `ets`, `arkts`, `ohos`, top level, then environment.

The SDK adapter loads ets-loader's ArkUI grammar, component declarations, and SDK module paths. It adds configuration in memory, leaving project files untouched. Project compiler options and `extends` override SDK defaults; explicit initialization `compilerOptions` override project settings. A workspace without `tsconfig.json` receives a virtual config.

`typescript.tsdk` selects declaration files, not a JavaScript compiler implementation. With an ArkTS SDK, only ECMAScript/decorator declarations are loaded from the TSDK to avoid DOM globals conflicting with ArkUI. tsrs remains the engine. Legacy `typescript.preferences` fields are accepted alongside native `unstable` preferences.

The host adds:

- SDK ArkTS restriction diagnostics, reusing the current Program and checker leases.
- OXC-parsed `$r` completion, definition, missing-resource diagnostics, and media previews.
- `module.json5` / `app.json5` `$type:name` completion, diagnostics and definitions, path completion and document links, plus `requestPermissions` completion and hover.
- `$$this` documentation and checking through a position-preserving compiler view.
- Existing-file document links with eager targets and optional resolve support.
- UTF-16 edits, resource watcher refresh, and SDK configuration refresh.

Resource completion uses an explicit replacement, including inside an existing name. OXC identifies `$$this` expressions before normalization, preserving strings and comments. Whole-document and range formatting are delegated to `oxk-formatter`.

Current boundaries: resource scope follows module directories and shared `AppScope`; complex build-profile product/target filtering and dependency HAR resources under `oh_modules` are not implemented. Reverse navigation from `resources/element/*.json` back to `$r`/`module.json5` call sites is not implemented. Diagnostics come from tsrs rather than DevEco's TypeScript build, so wording can differ.

## File icon theme

**OXK Symbols** is a Zed icon theme built from [Symbols](https://github.com/miguelsolorio/vscode-symbols) by Miguel Solorio (MIT). It includes:

- An ArkTS `.ets` icon plus `.d.ets` declaration files.
- HarmonyOS associations: `.json5`, `module.json5`, `build-profile.json5`, `oh-package.json5`, `hvigorfile.ts`, and `ets` / `entry` / `AppScope` / `hvigor` / `oh_modules` folders.
- The rest of the Symbols file and folder artwork, with Zed built-in icon keys aliased to the closest Symbols icon.

The icon is only used while **OXK Symbols** is the selected `icon_theme`; Zed has no per-language icon registration. Regenerate the theme with `node scripts/build-icon-theme.mjs`; see [icon_themes/README.md](icon_themes/README.md).

## Reuse the crates

| Crate | Responsibility | Dependencies |
| --- | --- | --- |
| `arkts-sdk` | SDK aliases, grammar/declarations, config merging | Filesystem and JSON |
| `arkts-services` | OXC documents, resources, binding docs, positions | Local OXC; no tsrs or editor |
| `arkts-lsp` | tsrs session, SDK filesystem, ArkTS additions | Native Rust; no Zed or Node |
| `oxk-zed` | Zed WASM adapter and server discovery | Zed extension API |

Other applications can use the same git dependencies. The OXC/tsrs crates are not published to crates.io; they are pulled from the `ohos` branches of the ohos-rs forks and pinned in `Cargo.lock`:

| Dependency | Source | Revision |
| --- | --- | --- |
| OXC | `https://github.com/ohos-rs/oxc` (`ohos`) | `1526b2c932d211c25d85dd172018889d0aeda776` |
| tsrs | `https://github.com/ohos-rs/tsrs` (`ohos`) | `fd5328b12436fdc044724fcfe1cb8896351cd46a` |

`cargo update` refreshes both to the latest commits on `ohos`.

```rust,no_run
use std::{path::Path, sync::Arc};

fn main() -> Result<(), String> {
    arkts_lsp::run(
        std::io::stdin(),
        std::io::stdout(),
        std::io::stderr(),
        Path::new("/absolute/workspace"),
        Arc::new(arkts_lsp::osvfs::fs()),
    )
}
```

`run` accepts host-owned streams and a filesystem implementing `arkts_lsp::FS` (re-exported from tsrs). EOF/shutdown/exit end the session. SDK discovery and resource indexing currently read the local filesystem, so entirely in-memory SDK/resource trees are not supported. `SdkFileSystem` is independently usable. Parsing helpers return owned data; arena pointers do not escape parsing scopes.

## Development

```sh
cargo test -p arkts-sdk -p arkts-services -p arkts-lsp
cargo clippy -p oxk-zed -p arkts-sdk -p arkts-services -p arkts-lsp --no-deps -- -D warnings
python3 tools/verify_lsp.py --sdk /absolute/sdk
```

The comparison harness can run against the legacy server installed under ignored `target/baseline`:

```sh
npm install --prefix target/baseline --no-audit --no-fund @arkts/language-server@1.3.10
python3 tools/verify_lsp.py --server legacy --sdk /absolute/sdk
python3 tools/lsp_compare.py --suite --sdk /absolute/sdk
python3 tools/summarize_compare.py
```

See [reports/lsp-comparison.md](reports/lsp-comparison.md) for the current comparison. Raw responses and memory samples are written under ignored `reports/raw`.

## License

[MIT](LICENSE). The bundled icon artwork is from [Symbols](https://github.com/miguelsolorio/vscode-symbols), also MIT; see [icon_themes/symbols/LICENSE](icon_themes/symbols/LICENSE).
