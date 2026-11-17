# OXK Symbols

`oxk-symbols.json` is a Zed file icon theme built from
[Symbols](https://github.com/miguelsolorio/vscode-symbols) by Miguel Solorio
(MIT). The SVG artwork in `symbols/` is vendored from upstream, except for
`custom/ets.svg`, which is the ArkTS icon added by this extension.

To use it, select **OXK Symbols** as the icon theme (`icon_theme` setting) and
keep `file_finder.file_icons` enabled (the default). Zed applies icon themes
globally, so this affects the project panel, tabs, and the language picker.

## Additions on top of upstream

- `.ets` maps to the ArkTS icon, and `.d.ets` uses the TypeScript declaration
  icon.
- `.json5` and the HarmonyOS project files (`module.json5`,
  `build-profile.json5`, `oh-package.json5`, `hvigorfile.ts`, ...) get
  TypeScript/JSON artwork.
- `ets`, `entry`, `AppScope`, `hvigor`, and `oh_modules` folders get distinct
  folder icons.
- Files that Zed only knows from its built-in icon theme (for example `vcs`,
  `storage`, or `terminal`) are aliased to the closest Symbols icon.

The ArkTS icon follows the Symbols design guide: a 24×24 canvas, the Symbols
palette (`#2563EB` blue-600 and `#14B8A6` teal-500), and the TS mark with a
HarmonyOS-style four-petal accent.

## Regenerate

The generator re-clones upstream at the pinned commit, copies the SVGs, and
rewrites the theme JSON:

```sh
node scripts/build-icon-theme.mjs
```

Pass `--source /path/to/vscode-symbols` to use an existing checkout, or
`--ref <commit>` to build from another revision. The script validates that
every referenced icon exists and that every association points at a defined
icon before writing the file.
