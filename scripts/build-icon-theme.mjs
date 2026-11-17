#!/usr/bin/env node
// Regenerate the bundled "OXK Symbols" Zed icon theme from
// https://github.com/miguelsolorio/vscode-symbols.
//
// Usage:
//   node scripts/build-icon-theme.mjs
//   node scripts/build-icon-theme.mjs --source /path/to/vscode-symbols
//
// The upstream SVGs and their MIT license are vendored into
// icon_themes/symbols/. Only the generated JSON and custom ArkTS icon need to
// be reviewed by hand.

import fs from "node:fs";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const UPSTREAM_REPOSITORY = "https://github.com/miguelsolorio/vscode-symbols.git";
// main as of 2026-11-04 ("Add common folder icons in mobile projects (#398)").
const UPSTREAM_REF = "a405d32cca082d838ad7739b6d0044dea9dbbb79";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT_THEME = path.join(ROOT, "icon_themes", "oxk-symbols.json");
const OUTPUT_ICONS = path.join(ROOT, "icon_themes", "symbols");
const CUSTOM_ICON = "./icon_themes/custom/ets.svg";

function parseArgs(argv) {
  const args = { source: null, ref: UPSTREAM_REF };
  for (let index = 0; index < argv.length; index += 1) {
    if (argv[index] === "--source") args.source = argv[index + 1];
    if (argv[index] === "--ref") args.ref = argv[index + 1];
  }
  return args;
}

function fetchSource(ref) {
  const cache = path.join(ROOT, ".cache", "vscode-symbols");
  const themeFile = path.join(cache, "src", "symbol-icon-theme.json");
  if (!fs.existsSync(themeFile)) {
    fs.rmSync(cache, { recursive: true, force: true });
    fs.mkdirSync(path.dirname(cache), { recursive: true });
    execFileSync("git", ["clone", "--no-checkout", UPSTREAM_REPOSITORY, cache], {
      stdio: "inherit",
    });
  }
  execFileSync("git", ["-C", cache, "fetch", "--depth", "1", "origin", ref], {
    stdio: "inherit",
  });
  execFileSync("git", ["-C", cache, "checkout", "--force", "FETCH_HEAD"], {
    stdio: "inherit",
  });
  return cache;
}

function copyIcons(source, folder) {
  const from = path.join(source, "src", "icons", folder);
  const to = path.join(OUTPUT_ICONS, folder);
  fs.rmSync(to, { recursive: true, force: true });
  fs.mkdirSync(to, { recursive: true });
  for (const name of fs.readdirSync(from)) {
    if (!name.endsWith(".svg")) continue;
    fs.copyFileSync(path.join(from, name), path.join(to, name));
  }
}

// Zed resolves paths relative to the extension root, not the theme JSON.
function iconPath(upstreamPath) {
  if (!upstreamPath.startsWith("./icons/")) {
    throw new Error(`Unexpected upstream icon path: ${upstreamPath}`);
  }
  return `./icon_themes/symbols/${upstreamPath.slice("./icons/".length)}`;
}

// Icon names that Zed's built-in icon theme maps suffixes to but Symbols calls
// something else. Re-pointing them keeps files that are only covered by Zed's
// default associations on the Symbols artwork.
const DEFAULT_KEY_ALIASES = {
  backup: "compressed",
  code: "code-gray",
  cpp: "cplus",
  default: "document",
  diff: "patch",
  document: "document",
  json: "brackets-yellow",
  log: "text",
  settings: "gear",
  storage: "database",
  template: "xml",
  terminal: "shell",
  typescript: "ts",
  vcs: "git",
};

// HarmonyOS additions. These reuse upstream artwork only.
const HARMONYOS_MAPPINGS = {
  fileSuffixes: {
    "d.ets": "dts",
    ets: "ets",
    json5: "brackets-yellow",
  },
  fileStems: {
    "build-profile.json5": "brackets-yellow",
    "hvigor-config.json5": "brackets-yellow",
    "hvigorfile.ts": "ts",
    "module.json5": "brackets-yellow",
    "oh-package-lock.json5": "lock",
    "oh-package.json5": "brackets-yellow",
  },
  folderNames: {
    AppScope: "folder-app",
    entry: "folder-app",
    ets: "folder-blue-code",
    hvigor: "folder-build",
    oh_modules: "folder-node-modules",
  },
};

function buildTheme(source) {
  const manifest = JSON.parse(
    fs.readFileSync(path.join(source, "src", "symbol-icon-theme.json"), "utf8"),
  );
  const definitions = manifest.iconDefinitions ?? {};
  const fileDefinitions = Object.fromEntries(
    Object.entries(definitions).filter(([key]) => !key.startsWith("folder")),
  );

  const fileIcons = {};
  for (const [key, definition] of Object.entries(fileDefinitions)) {
    if (!definition.iconPath?.endsWith(".svg")) continue;
    fileIcons[key] = { path: iconPath(definition.iconPath) };
  }

  for (const [alias, target] of Object.entries(DEFAULT_KEY_ALIASES)) {
    if (!fileIcons[alias] && fileIcons[target]) {
      fileIcons[alias] = fileIcons[target];
    }
  }
  fileIcons["default"] = fileIcons["document"] ?? { path: CUSTOM_ICON };
  fileIcons.ets = { path: CUSTOM_ICON };

  const fileSuffixes = {};
  for (const [suffix, key] of Object.entries(manifest.fileExtensions ?? {})) {
    if (!fileIcons[key]) continue;
    fileSuffixes[suffix] = key;
  }
  Object.assign(fileSuffixes, HARMONYOS_MAPPINGS.fileSuffixes);

  const fileStems = {};
  for (const [name, key] of Object.entries(manifest.fileNames ?? {})) {
    if (!fileIcons[key]) continue;
    for (const variant of new Set([name, name.toLowerCase(), name.toUpperCase()])) {
      fileStems[variant] = key;
    }
  }
  Object.assign(fileStems, HARMONYOS_MAPPINGS.fileStems);

  const folderDefinitions = Object.fromEntries(
    Object.entries(definitions).filter(([key]) => key.startsWith("folder")),
  );
  const folderPath = (key) => {
    const definition = folderDefinitions[key];
    return definition?.iconPath ? iconPath(definition.iconPath) : undefined;
  };

  const namedDirectoryIcons = {};
  for (const [name, key] of Object.entries(manifest.folderNames ?? {})) {
    const collapsed = folderPath(key);
    if (!collapsed) continue;
    const expanded = folderPath(manifest.folderNamesExpanded?.[name] ?? key) ?? collapsed;
    const variations = new Set([name, `_${name}`, `__${name}__`]);
    if (!name.startsWith(".")) variations.add(`.${name}`);
    for (const variation of variations) {
      namedDirectoryIcons[variation] ??= { collapsed, expanded };
    }
  }
  for (const [name, key] of Object.entries(HARMONYOS_MAPPINGS.folderNames)) {
    const icon = folderPath(key);
    if (icon) namedDirectoryIcons[name] = { collapsed: icon, expanded: icon };
  }

  return {
    $schema: "https://zed.dev/schema/icon_themes/v0.3.0.json",
    name: "OXK Symbols",
    author: "richerfu. Icons from Symbols by Miguel Solorio (MIT).",
    themes: [
      {
        name: "OXK Symbols",
        appearance: "dark",
        file_icons: fileIcons,
        directory_icons: {
          collapsed: iconPath("./icons/folders/folder.svg"),
          expanded: iconPath("./icons/folders/folder-open.svg"),
        },
        named_directory_icons: namedDirectoryIcons,
        file_suffixes: fileSuffixes,
        file_stems: fileStems,
      },
    ],
  };
}

function validate(theme) {
  const [variant] = theme.themes;
  const missing = [];
  const checkPath = (description, relative) => {
    if (!relative?.startsWith("./icon_themes/")) {
      missing.push(`${description}: unexpected path ${relative}`);
      return;
    }
    const absolute = path.join(ROOT, relative.slice(2));
    if (!fs.existsSync(absolute)) missing.push(`${description}: missing ${relative}`);
  };

  for (const [key, definition] of Object.entries(variant.file_icons)) {
    checkPath(`file_icons.${key}`, definition.path);
  }
  for (const [key, definition] of Object.entries(variant.named_directory_icons)) {
    checkPath(`named_directory_icons.${key}.collapsed`, definition.collapsed);
    checkPath(`named_directory_icons.${key}.expanded`, definition.expanded);
  }
  checkPath("directory_icons.collapsed", variant.directory_icons.collapsed);
  checkPath("directory_icons.expanded", variant.directory_icons.expanded);

  for (const [suffix, key] of Object.entries(variant.file_suffixes)) {
    if (!variant.file_icons[key]) missing.push(`file_suffixes.${suffix}: unknown icon ${key}`);
  }
  for (const [stem, key] of Object.entries(variant.file_stems)) {
    if (!variant.file_icons[key]) missing.push(`file_stems.${stem}: unknown icon ${key}`);
  }

  if (missing.length > 0) {
    throw new Error(`Invalid icon theme:\n${missing.slice(0, 40).join("\n")}`);
  }
}

const args = parseArgs(process.argv.slice(2));
const source = args.source ? path.resolve(args.source) : fetchSource(args.ref);
const sourceTheme = path.join(source, "src", "symbol-icon-theme.json");
if (!fs.existsSync(sourceTheme)) {
  throw new Error(`Not a vscode-symbols checkout: ${source}`);
}

copyIcons(source, "files");
copyIcons(source, "folders");

const theme = buildTheme(source);
validate(theme);
fs.mkdirSync(path.dirname(OUTPUT_THEME), { recursive: true });
fs.writeFileSync(OUTPUT_THEME, `${JSON.stringify(theme, null, 2)}\n`);

const [variant] = theme.themes;
console.log(
  `Wrote ${path.relative(ROOT, OUTPUT_THEME)}: ` +
    `${Object.keys(variant.file_icons).length} icons, ` +
    `${Object.keys(variant.file_suffixes).length} suffixes, ` +
    `${Object.keys(variant.file_stems).length} stems, ` +
    `${Object.keys(variant.named_directory_icons).length} folder names.`,
);
