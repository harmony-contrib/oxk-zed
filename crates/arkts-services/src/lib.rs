//! Editor-independent ArkTS additions, parsed with the local OXC ArkTS parser.
use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, AssignmentExpression, CallExpression, Expression, ObjectPropertyKind,
    StaticMemberExpression, StringLiteral,
};
use oxc_ast_visit::{walk, Visit};
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType, Span};
use serde_json::{json, Value};
use std::{
    borrow::Cow,
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct ResourceCall {
    pub value: String,
    pub span: Span,
}
#[derive(Clone, Debug, Default)]
pub struct Document {
    pub text: String,
    pub resource_calls: Vec<ResourceCall>,
    pub bindings: Vec<Span>,
    pub strings: Vec<ResourceCall>,
}

#[derive(Default)]
struct Calls {
    resources: Vec<ResourceCall>,
    bindings: Vec<Span>,
    strings: Vec<ResourceCall>,
}
impl<'a> Visit<'a> for Calls {
    fn visit_string_literal(&mut self, value: &StringLiteral<'a>) {
        self.strings.push(ResourceCall {
            value: value.value.as_str().unwrap_or_default().to_string(),
            span: value.span,
        });
    }
    fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
        if matches!(&call.callee, Expression::Identifier(id) if id.name == "$r") {
            if let Some(Argument::StringLiteral(value)) = call.arguments.first() {
                self.resources.push(ResourceCall {
                    value: value.value.as_str().unwrap_or_default().to_string(),
                    span: value.span,
                });
            }
        }
        walk::walk_call_expression(self, call);
    }
    fn visit_static_member_expression(&mut self, member: &StaticMemberExpression<'a>) {
        if let Expression::Identifier(id) = &member.object {
            if id.name == "$$this" {
                self.bindings.push(id.span);
            }
        }
        walk::walk_static_member_expression(self, member);
    }
}

impl Document {
    pub fn parse(text: String) -> Self {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, &text, SourceType::ets()).parse();
        let mut calls = Calls::default();
        calls.visit_program(&parsed.program);
        Self {
            text,
            resource_calls: calls.resources,
            bindings: calls.bindings,
            strings: calls.strings,
        }
    }
    pub fn binding_hover(&self, position: &Value, chinese: bool) -> Option<Value> {
        let offset = offset_at(&self.text, position)?;
        let span = self.bindings.iter().find(|span| contains(**span, offset))?;
        let text = if chinese {
            "$$运算符为系统组件提供TS变量的引用, 使得TS变量和系统组件的内部状态保持同步。详见: https://developer.huawei.com/consumer/cn/doc/harmonyos-guides/arkts-two-way-sync"
        } else {
            "$$ operator provides a reference to the TS variable for system components, keeping the TS variable synchronized with the internal state of the system component. See: https://developer.huawei.com/consumer/cn/doc/harmonyos-guides/arkts-two-way-sync"
        };
        Some(json!({"contents":{"kind":"markdown","value":text},"range":range(&self.text, *span)}))
    }
    /// tsrs checks two-way binding targets as ordinary `this` expressions. Replacing
    /// the two ASCII dollar signs with spaces preserves every byte and UTF-16 offset.
    /// OXC identifies expressions first, so strings and comments remain intact.
    pub fn tsrs_text(&self) -> Cow<'_, str> {
        if self.bindings.is_empty() {
            return Cow::Borrowed(&self.text);
        }
        let mut text = self.text.clone();
        for span in &self.bindings {
            text.replace_range(span.start as usize..span.start as usize + 2, "  ");
        }
        Cow::Owned(text)
    }

    /// Adapt edits only when either version contains a normalized binding. Ordinary edits
    /// can pass through unchanged; binding edits replace just the changed compiler span,
    /// so a slow consumer does not queue a full document for each keystroke.
    pub fn compiler_change(&self, previous: &Self) -> Option<Value> {
        if self.bindings.is_empty() && previous.bindings.is_empty() {
            return None;
        }
        let before = previous.tsrs_text();
        let after = self.tsrs_text();
        let mut start = before
            .bytes()
            .zip(after.bytes())
            .take_while(|(a, b)| a == b)
            .count();
        while !before.is_char_boundary(start) {
            start -= 1;
        }
        // An LSP position must not fall between CR and LF.
        if start > 0
            && before.as_bytes()[start - 1] == b'\r'
            && (before.as_bytes().get(start) == Some(&b'\n')
                || after.as_bytes().get(start) == Some(&b'\n'))
        {
            start -= 1;
        }
        let mut suffix = before[start..]
            .bytes()
            .rev()
            .zip(after[start..].bytes().rev())
            .take_while(|(a, b)| a == b)
            .count();
        while !before.is_char_boundary(before.len() - suffix)
            || !after.is_char_boundary(after.len() - suffix)
        {
            suffix -= 1;
        }
        if suffix > 0
            && [&*before, &*after].iter().any(|text| {
                let end = text.len() - suffix;
                end > 0 && text.as_bytes()[end - 1] == b'\r' && text.as_bytes()[end] == b'\n'
            })
        {
            suffix -= 1;
        }
        Some(json!({
            "range": {
                "start": position_at(&before, start),
                "end": position_at(&before, before.len() - suffix)
            },
            "text": &after[start..after.len() - suffix]
        }))
    }
    pub fn resource_at(&self, position: &Value) -> Option<&ResourceCall> {
        let offset = offset_at(&self.text, position)?;
        self.resource_calls
            .iter()
            .find(|call| contains(call.span, offset))
    }
    pub fn document_links(&self, source: &Path, uri: impl Fn(&Path) -> String) -> Value {
        let directory = source
            .ancestors()
            .find(|p| p.join("module.json5").is_file())
            .or_else(|| source.parent());
        let mut links = Vec::new();
        if let Some(dir) = directory {
            for s in &self.strings {
                if s.value.is_empty() || s.value == "." {
                    continue;
                }
                let path = [Some(dir), source.parent()]
                    .into_iter()
                    .flatten()
                    .map(|base| base.join(&s.value))
                    .find(|path| path.exists());
                if let Some(path) = path {
                    links.push(json!({"range":range(&self.text,s.span),"target":uri(&path)}));
                }
            }
        }
        json!(links)
    }
}

/// Entries of one `sysResource.js` assignment. Spans point at the property keys so
/// definitions can open the exact line instead of the top of the generated file.
#[derive(Default)]
struct SystemResources(Vec<(String, Span, String)>);
impl<'a> Visit<'a> for SystemResources {
    fn visit_assignment_expression(&mut self, assignment: &AssignmentExpression<'a>) {
        if let Expression::ObjectExpression(object) = &assignment.right {
            for property in &object.properties {
                if let ObjectPropertyKind::ObjectProperty(property) = property {
                    if let (Some(kind), Expression::ObjectExpression(values)) =
                        (property.key.static_name(), &property.value)
                    {
                        let Some(kind) = kind.as_str() else { continue };
                        for value in &values.properties {
                            if let ObjectPropertyKind::ObjectProperty(value) = value {
                                if let Some(name) = value.key.static_name() {
                                    if let Some(name) = name.as_str() {
                                        let id = match &value.value {
                                            Expression::NumericLiteral(number) => {
                                                number.raw.as_ref().map_or_else(
                                                    || number.value.to_string(),
                                                    ToString::to_string,
                                                )
                                            }
                                            _ => String::new(),
                                        };
                                        self.0.push((
                                            format!("sys.{kind}.{name}"),
                                            value.key.span(),
                                            id,
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        walk::walk_assignment_expression(self, assignment);
    }
}

fn contains(span: Span, offset: usize) -> bool {
    span.start as usize <= offset && offset <= span.end as usize
}

pub fn position_at(text: &str, offset: usize) -> Value {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    let before = &text[..offset];
    json!({"line":before.bytes().filter(|b| *b == b'\n').count(), "character":before.rsplit('\n').next().unwrap_or("").encode_utf16().count()})
}

pub fn range(text: &str, span: Span) -> Value {
    json!({"start":position_at(text, span.start as usize), "end":position_at(text, span.end as usize)})
}

/// Returns a UTF-8 byte offset from an LSP UTF-16 position, including surrogate pairs.
pub fn offset_at(text: &str, position: &Value) -> Option<usize> {
    let line = position["line"].as_u64()? as usize;
    let column = position["character"].as_u64()? as usize;
    let mut start = 0;
    for _ in 0..line {
        start += text.get(start..)?.find('\n')? + 1;
    }
    let mut utf16 = 0;
    for (offset, ch) in text.get(start..)?.char_indices() {
        if utf16 == column {
            return Some(start + offset);
        }
        if ch == '\n' || ch == '\r' {
            return None;
        }
        utf16 += ch.len_utf16();
    }
    (utf16 == column).then_some(text.len())
}

#[derive(Clone, Debug)]
pub struct Resource {
    pub label: String,
    pub path: PathBuf,
    pub selection: Value,
    pub qualifiers: String,
}

#[derive(Clone, Debug)]
struct SystemResource {
    file: usize,
    id: String,
    selection: Value,
}

#[derive(Default)]
pub struct ResourceIndex {
    resources: Vec<Resource>,
    system: BTreeMap<String, SystemResource>,
    /// Concrete previewer values per resource id and qualifier (`base`, `dark`, locales).
    system_values: BTreeMap<String, BTreeMap<String, String>>,
    system_paths: Vec<PathBuf>,
    roots: Vec<PathBuf>,
}

/// The SDK ships generated resource tables. Parse them with OXC instead of executing
/// SDK JavaScript, keeping name spans for definitions and the id for hover.
fn load_system_file(path: &Path, index: &mut ResourceIndex) {
    let Ok(text) = fs::read_to_string(path) else {
        return;
    };
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, &text, SourceType::cjs()).parse();
    let mut resources = SystemResources::default();
    resources.visit_program(&parsed.program);
    let file = index.system_paths.len();
    index.system_paths.push(path.to_path_buf());
    for (label, span, id) in resources.0 {
        index.system.entry(label).or_insert(SystemResource {
            file,
            id,
            selection: range(&text, span),
        });
    }
}

/// `sysResource.js` only maps names to ids. The previewer ships the concrete values
/// per qualifier directory; index them for hover and for `$type:<id>` resolution.
fn load_system_values(path: &Path, index: &mut ResourceIndex) {
    let Ok(text) = fs::read_to_string(path) else {
        return;
    };
    let mut qualifier = String::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("id:") {
            let Some((id, rest)) = rest.split_once(", '") else {
                continue;
            };
            let Some((value, name)) = rest.rsplit_once("' '") else {
                continue;
            };
            let Some(name) = name.strip_suffix('\'') else {
                continue;
            };
            if name.is_empty() {
                continue;
            }
            index
                .system_values
                .entry(id.to_string())
                .or_default()
                .insert(qualifier.clone(), value.to_string());
        } else if !line.starts_with("keyconfig") && !line.contains('\'') {
            // Values may span lines; continuation lines contain quotes and stay in qualifier.
            qualifier = line.to_string();
        }
    }
}

fn walk_files(path: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() {
            if !matches!(
                entry.file_name().to_str(),
                Some("node_modules" | "oh_modules" | ".git" | "build" | "target")
            ) {
                walk_files(&path, files);
            }
        } else if kind.is_file() {
            files.push(path);
        }
    }
}

impl ResourceIndex {
    pub fn load(roots: &[PathBuf], options: &Value) -> Self {
        let mut index = Self {
            roots: roots.to_vec(),
            ..Self::default()
        };
        if let Some(sdk) = options.pointer("/ets/sdkPath").and_then(Value::as_str) {
            let sdk = Path::new(sdk);
            let ends_with_ets = sdk.file_name().is_some_and(|n| n == "ets");
            let ets = if ends_with_ets {
                sdk.to_path_buf()
            } else {
                sdk.join("ets")
            };
            let root = if ends_with_ets {
                sdk.parent()
                    .filter(|parent| !parent.as_os_str().is_empty())
                    .unwrap_or(sdk)
            } else {
                sdk
            };
            load_system_file(
                &ets.join("build-tools")
                    .join("ets-loader")
                    .join("sysResource.js"),
                &mut index,
            );
            load_system_values(
                &root
                    .join("previewer")
                    .join("common")
                    .join("resources")
                    .join("entry")
                    .join("resources.txt"),
                &mut index,
            );
        }
        // HMS resources share the `sys.` namespace but live outside ets-loader.
        if let Some(hms) = options.pointer("/ets/hmsPath").and_then(Value::as_str) {
            let hms = Path::new(hms);
            let ets = if hms.file_name().is_some_and(|n| n == "ets") {
                hms.to_path_buf()
            } else {
                hms.join("ets")
            };
            load_system_file(&ets.join("sysResource.js"), &mut index);
        }
        for root in roots {
            let mut files = Vec::new();
            walk_files(root, &mut files);
            for path in files {
                let Some(parent) = path.parent() else {
                    continue;
                };
                let Some(kind) = parent.file_name().and_then(|s| s.to_str()) else {
                    continue;
                };
                let Some(group) = parent.parent() else {
                    continue;
                };
                if !group
                    .parent()
                    .is_some_and(|p| p.file_name().is_some_and(|s| s == "resources"))
                {
                    continue;
                }
                let qualifiers = group
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                if kind == "element" && path.extension().is_some_and(|s| s == "json") {
                    let Ok(text) = fs::read_to_string(&path) else {
                        continue;
                    };
                    let Ok(value) = arkts_sdk::parse_config(&text) else {
                        continue;
                    };
                    if let Some(types) = value.as_object() {
                        for (kind, items) in types {
                            if let Some(items) = items.as_array() {
                                for item in items {
                                    let Some(name) = item["name"].as_str() else {
                                        continue;
                                    };
                                    let needle = serde_json::to_string(name).unwrap();
                                    let start = text.find(&needle).unwrap_or(0);
                                    index.resources.push(Resource {
                                        label: format!("app.{kind}.{name}"),
                                        path: path.clone(),
                                        selection: range(
                                            &text,
                                            Span::new(start as u32, (start + needle.len()) as u32),
                                        ),
                                        qualifiers: qualifiers.clone(),
                                    });
                                }
                            }
                        }
                    }
                } else if matches!(kind, "media" | "profile") {
                    if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                        index.resources.push(Resource { label: format!("app.{kind}.{name}"), path:path.clone(), selection:json!({"start":{"line":0,"character":0},"end":{"line":0,"character":0}}), qualifiers });
                    }
                }
            }
        }
        index
    }

    fn in_scope(&self, resource: &Resource, source: &Path) -> bool {
        // A module's resources belong to its sources. Shared AppScope resources belong to every module.
        if resource
            .path
            .components()
            .any(|c| c.as_os_str() == "AppScope")
        {
            return true;
        }
        if let Some(module) = source.ancestors().find(|p| {
            p.join("src/main/module.json5").is_file() || p.join("src/main/resources").is_dir()
        }) {
            return resource.path.starts_with(module);
        }
        self.roots
            .iter()
            .any(|root| source.starts_with(root) && resource.path.starts_with(root))
    }

    pub fn completions(
        &self,
        document: &Document,
        position: &Value,
        source: &Path,
    ) -> Option<Value> {
        let call = document.resource_at(position)?;
        let start = call.span.start as usize + 1;
        let end = call.span.end as usize - 1;
        let cursor = offset_at(&document.text, position)?.clamp(start, end);
        let prefix = document.text.get(start..cursor)?;
        let mut labels: BTreeMap<_, ()> =
            self.system.keys().cloned().map(|key| (key, ())).collect();
        for resource in self.resources.iter().filter(|r| self.in_scope(r, source)) {
            labels.insert(resource.label.clone(), ());
        }
        let items: Vec<_> = labels
            .keys()
            .filter(|label| label.starts_with(prefix))
            .map(|label| {
                json!({"label":label,"kind":12,"detail":label,
                    "textEdit":{"range":{"start":position_at(&document.text,start),"end":position_at(&document.text,end)},"newText":label}})
            })
            .collect();
        Some(json!({"isIncomplete":false,"items":items}))
    }

    pub fn definitions(
        &self,
        document: &Document,
        position: &Value,
        source: &Path,
        uri: impl Fn(&Path) -> String,
    ) -> Option<Value> {
        let call = document.resource_at(position)?;
        let origin = range(&document.text, call.span);
        if call.value.starts_with("sys.") {
            // Match the reference server, but jump to the exact property when it exists.
            let definition = if let Some(resource) = self.system.get(&call.value) {
                let path = &self.system_paths[resource.file];
                json!({"originSelectionRange":origin,"targetUri":uri(path),
                    "targetRange":resource.selection,"targetSelectionRange":resource.selection})
            } else if let Some(path) = self.system_paths.first() {
                let top = json!({"start":{"line":0,"character":0},"end":{"line":0,"character":0}});
                json!({"originSelectionRange":origin,"targetUri":uri(path),
                    "targetRange":top,"targetSelectionRange":top})
            } else {
                return Some(json!([]));
            };
            return Some(json!([definition]));
        }
        let definitions: Vec<_> = self.resources.iter().filter(|r| r.label == call.value && self.in_scope(r, source)).map(|r|
            json!({"originSelectionRange":origin,"targetUri":uri(&r.path),"targetRange":r.selection,"targetSelectionRange":r.selection})).collect();
        Some(json!(definitions))
    }

    pub fn system_hover(
        &self,
        document: &Document,
        position: &Value,
        chinese: bool,
    ) -> Option<Value> {
        let call = document.resource_at(position)?;
        let resource = self.system.get(&call.value)?;
        let path = self.system_paths.get(resource.file)?;
        let kind = call
            .value
            .strip_prefix("sys.")
            .and_then(|value| value.split_once('.'))
            .map(|(kind, _)| kind)
            .unwrap_or_default();
        let labels = if chinese {
            [
                "属性",
                "值",
                "资源类型",
                "资源 ID",
                "默认值",
                "深色值",
                "SDK 文件",
            ]
        } else {
            [
                "Properties",
                "Value",
                "Resource type",
                "Resource id",
                "Value (base)",
                "Value (dark)",
                "SDK file",
            ]
        };
        let mut rows = vec![
            (labels[2], kind.to_string()),
            (labels[3], resource.id.clone()),
        ];
        let mut has_value = false;
        for (qualifier, label) in [("base", labels[4]), ("dark", labels[5])] {
            if let Some(value) = self.system_value(&resource.id, qualifier) {
                rows.push((label, value));
                has_value = true;
            }
        }
        if !has_value {
            if let Some((qualifier, value)) = self
                .system_values
                .get(&resource.id)
                .and_then(|values| values.iter().next())
            {
                rows.push((labels[1], format!("{value} ({qualifier})")));
            }
        }
        rows.push((labels[6], path.display().to_string()));
        let mut value = format!(
            "**{}**\n\n| {} | {} |\n|------|----|\n",
            call.value, labels[0], labels[1]
        );
        for (label, cell) in rows {
            value.push_str(&format!(
                "| {label} | `{}` |\n",
                cell.replace('`', "'")
                    .replace('|', "\\|")
                    .replace('\n', " ")
            ));
        }
        Some(
            json!({"contents":{"kind":"markdown","value":value},"range":range(&document.text,call.span)}),
        )
    }

    /// Resolves `$color:<id>` style aliases within the same qualifier.
    fn system_value(&self, id: &str, qualifier: &str) -> Option<String> {
        let values = self.system_values.get(id)?;
        let mut value = values.get(qualifier)?.clone();
        for _ in 0..8 {
            let Some(reference) = value
                .strip_prefix('$')
                .and_then(|rest| rest.split_once(':'))
                .map(|(_, id)| id)
            else {
                break;
            };
            let Some(next) = self
                .system_values
                .get(reference)
                .and_then(|values| values.get(qualifier))
            else {
                break;
            };
            if *next == value {
                break;
            }
            value = next.clone();
        }
        Some(value)
    }

    pub fn media_hover(
        &self,
        document: &Document,
        position: &Value,
        source: &Path,
        chinese: bool,
    ) -> Option<Value> {
        let call = document.resource_at(position)?;
        if !call.value.starts_with("app.media.") {
            return None;
        }
        let mut value = String::new();
        for r in self
            .resources
            .iter()
            .filter(|r| r.label == call.value && self.in_scope(r, source))
        {
            let name = r.path.file_name()?.to_string_lossy();
            let size = fs::metadata(&r.path).ok()?.len();
            let labels = if chinese {
                ["属性", "值", "文件名", "文件大小", "资源组限定词"]
            } else {
                [
                    "Properties",
                    "Value",
                    "File Name",
                    "File Size",
                    "Resource qualifiers",
                ]
            };
            value.push_str(&format!("**🖼️ [{name}]({})**\n\n| {} | {} |\n|------|----|\n| {} | `{name}` |\n| {} | `{size}` |\n| {} | `{}` |\n\n![{name}]({})\n", r.path.display(),labels[0],labels[1],labels[2],labels[3],labels[4],r.qualifiers,r.path.display()));
        }
        (!value.is_empty()).then(|| json!({"contents":{"kind":"markdown","value":value},"range":range(&document.text,call.span)}))
    }

    pub fn diagnostics(&self, document: &Document, source: &Path) -> Vec<Value> {
        let mut result = Vec::new();
        for call in &document.resource_calls {
            let (code, message) = if call.value.is_empty() {
                (
                    "RESOURCE_NOT_PROVIDED",
                    "Resource value is empty".to_string(),
                )
            } else if call.value.starts_with("sys") {
                if self.system.is_empty() || self.system.contains_key(&call.value) {
                    continue;
                }
                (
                    "SYS_RESOURCE_NOT_FOUND",
                    format!(
                        "Resource {} not found in current scope. Indexed system resources: {}",
                        call.value,
                        self.system_paths
                            .iter()
                            .map(|p| p.display().to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                )
            } else if call.value.starts_with("app") {
                let scope: Vec<_> = self
                    .resources
                    .iter()
                    .filter(|r| self.in_scope(r, source))
                    .collect();
                if scope.is_empty() || scope.iter().any(|r| r.label == call.value) {
                    continue;
                }
                let mut dirs: Vec<_> = scope
                    .iter()
                    .filter_map(|r| r.path.parent().and_then(Path::parent))
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect();
                dirs.sort();
                dirs.dedup();
                (
                    "APP_RESOURCE_NOT_FOUND",
                    format!(
                        "Resource {} not found in current scope. Indexed application resources: {}",
                        call.value,
                        dirs.join(", ")
                    ),
                )
            } else {
                (
                    "INVALID_RESOURCE_SCOPE",
                    format!(
                        "Invalid resource scope: {}, expected starts with `sys` or `app`",
                        call.value
                    ),
                )
            };
            result.push(json!({"range":range(&document.text,call.span),"severity":1,"source":"ets","code":code,"message":message,
                "codeDescription":{"href":"https://developer.huawei.com/consumer/cn/doc/harmonyos-guides/resource-categories-and-access#资源访问"}}));
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiler_changes_preserve_binding_transitions_unicode_and_line_endings() {
        let cases = [
            ("Text($$this.value);", "Text($$this.other);"),
            ("Text($$this.value);", "Text(this.value);"),
            ("Text(this.value);", "Text($$this.value);"),
            ("Text($$this.value);", "// Text($$this.value);"),
            ("// Text($$this.value);", "Text($$this.value);"),
            ("'🦀'; Text($$this.value);", "'🦁'; Text($$this.value);"),
            ("'🦀';\r\nText($$this.value);", "'🦀';\nText($$this.value);"),
            ("'🦀';\nText($$this.value);", "'🦀';\r\nText($$this.value);"),
            ("Text($$this.value);\r\n", "Text($$this.value);\r\n\r\n"),
            ("Text($$this.value);", "Text($$this.value);"),
            ("Text($$this.value);", ""),
            ("", "Text($$this.value);"),
        ];
        for (before, after) in cases {
            let previous = Document::parse(before.into());
            let current = Document::parse(after.into());
            let change = current.compiler_change(&previous).unwrap();
            let mut actual = previous.tsrs_text().into_owned();
            let start = offset_at(&actual, &change["range"]["start"]).unwrap();
            let end = offset_at(&actual, &change["range"]["end"]).unwrap();
            actual.replace_range(start..end, change["text"].as_str().unwrap());
            assert_eq!(actual, current.tsrs_text(), "{before:?} -> {after:?}");
        }
        let plain = Document::parse("const value = 1;".into());
        assert!(plain.compiler_change(&plain).is_none());
    }

    #[test]
    fn syntax_excludes_comments_and_property_calls_and_handles_unicode() {
        let doc = Document::parse("// $r('fake')\nconst emoji = '🦀'; const a = $r('app.string.name'); obj.$r('ignored'); Text($$this.value);".into());
        assert_eq!(doc.resource_calls.len(), 1);
        assert_eq!(doc.resource_calls[0].value, "app.string.name");
        assert_eq!(doc.bindings.len(), 1);
        let compiler = doc.tsrs_text();
        assert_eq!(compiler.len(), doc.text.len());
        assert!(compiler.contains("Text(  this.value)"));
        assert!(compiler.contains("// $r('fake')"));
        let p = position_at(&doc.text, doc.bindings[0].start as usize);
        assert_eq!(
            offset_at(&doc.text, &p),
            Some(doc.bindings[0].start as usize)
        );
        assert!(doc.binding_hover(&p, true).unwrap()["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("使得"));
    }
    #[test]
    fn binding_normalization_preserves_literals_and_utf16_positions() {
        let doc = Document::parse(
            "const s = '$$this.value'; // $$this.value\r\nText($$this.value);".into(),
        );
        assert_eq!(doc.bindings.len(), 1);
        assert!(doc
            .tsrs_text()
            .starts_with("const s = '$$this.value'; // $$this.value\r\n"));
        let text = "🦀é\r\n文";
        assert_eq!(offset_at(text, &json!({"line":0,"character":1})), None);
        assert_eq!(offset_at(text, &json!({"line":0,"character":3})), Some(6));
        assert_eq!(
            offset_at(text, &json!({"line":1,"character":1})),
            Some(text.len())
        );
    }
    #[test]
    fn resources_are_scoped_to_module_and_include_qualifiers() {
        let root = tempfile::tempdir().unwrap();
        for module in ["entry", "other"] {
            let dir = root
                .path()
                .join(module)
                .join("src/main/resources/base/element");
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join("string.json"),
                json!({"string":[{"name":module,"value":module}]}).to_string(),
            )
            .unwrap();
        }
        let index = ResourceIndex::load(&[root.path().into()], &json!({}));
        let doc = Document::parse("const r = $r('app.string.');".into());
        let p = position_at(&doc.text, doc.resource_calls[0].span.start as usize + 3);
        let list = index
            .completions(&doc, &p, &root.path().join("entry/src/main/ets/Index.ets"))
            .unwrap();
        assert_eq!(list["items"].as_array().unwrap().len(), 1);
        assert_eq!(list["items"][0]["label"], "app.string.entry");
    }

    #[test]
    fn system_resources_complete_define_and_hover_from_sdk_and_hms() {
        let root = tempfile::tempdir().unwrap();
        let loader = root
            .path()
            .join("sdk")
            .join("ets")
            .join("build-tools")
            .join("ets-loader");
        fs::create_dir_all(&loader).unwrap();
        let oh_text = "module.exports.sys = {\n  color: {\n    comp_background_list_card: 125831127,\n    alias_color: 125831128,\n  },\n};";
        fs::write(loader.join("sysResource.js"), oh_text).unwrap();
        let previewer = root
            .path()
            .join("sdk")
            .join("previewer")
            .join("common")
            .join("resources")
            .join("entry");
        fs::create_dir_all(&previewer).unwrap();
        fs::write(
            previewer.join("resources.txt"),
            concat!(
                "base\n",
                "id:125831127, '#FFFFFF' 'comp_background_list_card'\n",
                "id:125831128, '$color:125831127' 'alias_color'\n",
                "dark\n",
                "id:125831127, '#19FFFFFF' 'comp_background_list_card'\n"
            ),
        )
        .unwrap();
        let hms = root.path().join("hms").join("ets");
        fs::create_dir_all(&hms).unwrap();
        let hms_text = "module.exports.sys = {\n  string: {\n    hms_only: 130023424,\n  },\n};";
        fs::write(hms.join("sysResource.js"), hms_text).unwrap();
        let options = json!({"ets": {
            "sdkPath": root.path().join("sdk").to_string_lossy(),
            "hmsPath": root.path().join("hms").to_string_lossy()
        }});
        let index = ResourceIndex::load(&[root.path().into()], &options);
        let source = root.path().join("entry/src/main/ets/Index.ets");
        let doc = Document::parse(
            concat!(
                "const a = $r('sys.color.comp_background_list_card');\n",
                "const b = $r('sys.string.hms_only');\n",
                "const c = $r('sys.color.missing');\n",
                "const d = $r('sys.color.alias_color');\n"
            )
            .into(),
        );
        let at = |value: &str, add: usize| {
            let call = doc
                .resource_calls
                .iter()
                .find(|call| call.value == value)
                .unwrap();
            position_at(&doc.text, call.span.start as usize + add)
        };

        let partial = Document::parse("const p = $r('sys.');".into());
        let p = position_at(
            &partial.text,
            partial.resource_calls[0].span.start as usize + 5,
        );
        let list = index.completions(&partial, &p, &source).unwrap();
        let labels: Vec<_> = list["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["label"].as_str().unwrap())
            .collect();
        assert!(labels.contains(&"sys.color.comp_background_list_card"));
        assert!(labels.contains(&"sys.string.hms_only"));

        let definition = index
            .definitions(
                &doc,
                &at("sys.color.comp_background_list_card", 3),
                &source,
                |path| path.display().to_string(),
            )
            .unwrap();
        let definition = &definition.as_array().unwrap()[0];
        assert_eq!(
            Path::new(definition["targetUri"].as_str().unwrap()),
            loader.join("sysResource.js")
        );
        let expected = position_at(oh_text, oh_text.find("comp_background_list_card").unwrap());
        assert_eq!(
            definition["targetSelectionRange"]["start"]["line"],
            expected["line"]
        );
        assert_eq!(
            definition["targetSelectionRange"]["start"]["character"],
            expected["character"]
        );

        let hms_definition = index
            .definitions(&doc, &at("sys.string.hms_only", 3), &source, |path| {
                path.display().to_string()
            })
            .unwrap();
        assert_eq!(
            Path::new(
                hms_definition.as_array().unwrap()[0]["targetUri"]
                    .as_str()
                    .unwrap()
            ),
            hms.join("sysResource.js")
        );

        let hover = index
            .system_hover(&doc, &at("sys.color.comp_background_list_card", 3), true)
            .unwrap();
        let value = hover["contents"]["value"].as_str().unwrap();
        assert!(value.contains("125831127"));
        assert!(value.contains("#FFFFFF"));
        assert!(value.contains("#19FFFFFF"));
        assert!(value.contains("sysResource.js"));

        let alias = index
            .system_hover(&doc, &at("sys.color.alias_color", 3), true)
            .unwrap();
        assert!(alias["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("#FFFFFF"));

        let diagnostics = index.diagnostics(&doc, &source);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0]["code"], "SYS_RESOURCE_NOT_FOUND");
        assert!(diagnostics[0]["message"]
            .as_str()
            .unwrap()
            .contains(&hms.join("sysResource.js").display().to_string()));
    }
}
