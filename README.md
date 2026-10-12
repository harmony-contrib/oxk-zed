# ArkTS for Zed

Native ArkTS (`.ets`) services powered by local **tsrs** and **OXC**. The main server runs as one Rust process; the extension no longer installs or launches `@arkts/language-server` or its Node IPC wrapper.

The Zed server IDs remain `arkts-language-server`, `oxk-formatter`, and `oxk-linter`, preserving existing language settings. Highlighting still uses `harmony-contrib/tree-sitter-arkts`. Formatting and the separate OXK lint server retain their existing `@ohos-rs/oxk` integration and binary overrides.

## Build and install locally

The native crates depend on local source trees:

| Dependency | Path relative to this repository | Revision tested |
| --- | --- | --- |
| OXC | `../oxc` | `230deff13c3d0baddb9560ba895ee2b6a3fc4b7e` |
| tsrs | `../../ohos-rs/tsrs` | `7308ab7f29516b3330d9d0dad4945b571b54772c` |

Rust 1.99.0 and the `wasm32-wasip2` target are specified in `rust-toolchain.toml`. With those checkouts available:

```sh
scripts/build-local.sh
cargo run --example validate-extension -- extension.wasm
```

This builds `target/release/arkts-lsp` and `extension.wasm`. Install this directory with Zed's **Install Dev Extension** command. The workspace's default member is the root WASM extension, matching Zed's extension build. The native server is built separately.

The main binary is selected in this order:

1. `lsp.arkts-language-server.binary.path`, with optional `binary.arguments`.
2. The worktree's `ARKTS_LSP_PATH` environment variable.
3. `arkts-lsp` on the worktree's PATH.
4. `target/release/arkts-lsp` in the checkout used to build the extension.

Use an explicit path when moving a compiled extension to another machine. The final fallback is for local development; there is no automatic native binary download.

```json
{
  "lsp": {
    "arkts-language-server": {
      "binary": {
        "path": "/absolute/path/to/oxk-zed/target/release/arkts-lsp",
        "arguments": ["--stdio"]
      },
      "settings": {
        "arkts": {
          "SDK_HOME": "/path/to/OpenHarmony/20",
          "HMS_HOME": "/path/to/HarmonyOS/hms"
        },
        "typescript": {
          "inlayHints": { "parameterNames": { "enabled": "all" } }
        }
      }
    },
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
  },
  "languages": {
    "ArkTS": {
      "language_servers": ["arkts-language-server", "oxk-linter", "oxk-formatter"],
      "formatter": { "language_server": { "name": "oxk-formatter" } }
    }
  }
}
```

Omit `HMS_HOME` without an HMS SDK. Node/npm are needed for the default OXK formatter and lint launchers, not the native main server. Those launchers also accept binary overrides.

## SDK and language behavior

The main server accepts `ets.sdkPath` and previous aliases: `arkts.SDK_HOME`, `arkts.sdkPath`, top-level `SDK_HOME`, `ohos.sdkPath`, and shell `SDK_HOME`/`OHOS_SDK_HOME`. HMS aliases map to `ets.hmsPath`. Precedence is `ets`, `arkts`, `ohos`, top level, then environment.

The SDK adapter loads ets-loader's ArkUI grammar, component declarations, and SDK module paths. It adds configuration in memory, leaving project files untouched. Project compiler options and `extends` override SDK defaults; explicit initialization `compilerOptions` override project settings. A workspace without `tsconfig.json` receives a virtual config. Ordinary TypeScript/ETS features work without an SDK; ArkUI globals and system resources require one.

`typescript.tsdk` selects declaration files, not a JavaScript compiler implementation. With an ArkTS SDK, only ECMAScript/decorator declarations are loaded from the TSDK to avoid DOM globals conflicting with ArkUI. tsrs remains the engine. Legacy `typescript.preferences` fields are accepted alongside native `unstable` preferences.

tsrs provides completion/resolve, hover/JSDoc, navigation, references, rename, symbols, signature help, highlights, folding, selection, inlay hints, semantic tokens, call hierarchy, file-rename import edits, code actions, and type diagnostics. The host adds:

- SDK ArkTS restriction diagnostics, reusing the current Program and checker leases.
- OXC-parsed `$r` completion, definition, missing-resource diagnostics, and media previews.
- `$$this` documentation and checking through a position-preserving compiler view.
- Existing-file document links with eager targets and optional resolve support.
- UTF-16 edits, resource watcher refresh, and SDK configuration refresh.

Resource completion uses an explicit replacement, including inside an existing name. OXC identifies `$$this` expressions before normalization, preserving strings and comments. Main-server whole-document and range formatting are delegated to the configured OXK formatter.

Current boundaries: resource scope follows module directories and shared `AppScope`; complex build-profile product/target filtering and dependency HAR resources under `oh_modules` are not implemented. This extension registers ArkTS, not JSON permission-manifest editing. Diagnostic text can differ from the old localized TypeScript implementation. See the [comparison report](reports/lsp-comparison.md).

## Reuse the crates

| Crate | Responsibility | Dependencies |
| --- | --- | --- |
| `arkts-sdk` | SDK aliases, grammar/declarations, config merging | Filesystem and JSON |
| `arkts-services` | OXC documents, resources, binding docs, positions | Local OXC; no tsrs or editor |
| `arkts-lsp` | tsrs session, SDK filesystem, ArkTS additions | Native Rust; no Zed or Node |
| `oxk-zed` | Zed WASM adapter and server discovery | Zed extension API |

Other applications can use path dependencies. The local OXC/tsrs checkouts must remain available; these crates are not published to crates.io.

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

## Verify and compare

```sh
cargo test -p arkts-sdk -p arkts-services -p arkts-lsp
cargo clippy -p oxk-zed -p arkts-sdk -p arkts-services -p arkts-lsp --no-deps -- -D warnings
python3 tools/verify_lsp.py --sdk /absolute/sdk
```

The baseline is installed only under ignored `target/baseline`:

```sh
npm install --prefix target/baseline --no-audit --no-fund @arkts/language-server@1.3.10
python3 tools/verify_lsp.py --server legacy --sdk /absolute/sdk
python3 tools/lsp_compare.py --suite --sdk /absolute/sdk
python3 tools/lsp_compare.py --files 100 --edits 2000 --run 9 --sdk /absolute/sdk
python3 tools/summarize_compare.py
```

The macOS benchmark uses `/usr/bin/time -l` and 50 ms process-tree RSS sampling. Three runs alternate launch order for 102 and 1002 source files. Each includes feature requests, 100 warm hovers, and 200 edits followed by a hover barrier. Raw responses, notifications, latency, CPU counters, and memory samples are saved under ignored `reports/raw`. Zed, highlighting, and the separate formatter/linter are outside this measurement.

`oxk-formatter` remains registered for ArkTS, JavaScript/JSX, TypeScript/TSX, Vue.js, JSON/JSON5/JSONC, HTML, CSS/SCSS/LESS, Angular, GraphQL, Handlebars, Markdown/MDX, YAML, and TOML. OXK lint rules and fixes use `.oxlintrc.*` and the existing settings.

## License

[MIT](LICENSE)
