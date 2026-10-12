#!/usr/bin/env python3
"""Exercise native ArkTS behavior against a real SDK. --server legacy records the baseline.

Assertions check useful results and applied edits, rather than byte-for-byte presentation.
"""
import argparse
import json
import os
from pathlib import Path
import queue

from lsp_compare import Client, DEFAULT_SDK, MAIN, MATH, ROOT, position

UI = '''@Entry
@Component
struct Index {
  @State message: string = 'Hello';
  @State isEnabled: boolean = true;
  build() {
    Column() {
      Text(this.message).fontSize(20)
      Text($r('app.string.title'))
      Image($r('app.media.icon'))
      Toggle({ type: ToggleType.Switch, isOn: $$this.isEnabled })
    }
  }
}
const illegal: any = 1;
const partial = $r('app.string.');
const invalid = $r('app.string.missing');
const system = $r('sys.color.');
const link = '../resources/base/media/icon.svg';
'''


def capabilities():
    return {
        'workspace': {'configuration': True, 'workspaceFolders': True, 'didChangeWatchedFiles': {'dynamicRegistration': True}},
        'general': {'positionEncodings': ['utf-8', 'utf-16']},
        'textDocument': {
            'hover': {'contentFormat': ['markdown']},
            'completion': {'completionItem': {'snippetSupport': True, 'insertReplaceSupport': True}},
            'definition': {'linkSupport': True}, 'rename': {'prepareSupport': True},
            'documentSymbol': {'hierarchicalDocumentSymbolSupport': True},
            'publishDiagnostics': {'relatedInformation': True},
            'codeAction': {'codeActionLiteralSupport': {'codeActionKind': {'valueSet': ['quickfix', 'source.organizeImports', 'refactor']}}},
            'diagnostic': {}, 'inlayHint': {},
            'semanticTokens': {'requests': {'range': True, 'full': True}, 'formats': ['relative'],
                'tokenTypes': ['namespace', 'class', 'enum', 'interface', 'typeParameter', 'type', 'parameter', 'variable', 'property', 'enumMember', 'function', 'method'],
                'tokenModifiers': ['declaration', 'static', 'async', 'readonly', 'defaultLibrary', 'local']},
        },
    }


def apply_edit(text, edit):
    def offset(p):
        lines = text.splitlines(keepends=True)
        before = ''.join(lines[:p['line']])
        line = lines[p['line']] if p['line'] < len(lines) else ''
        prefix = line.encode('utf-16-le')[:p['character'] * 2].decode('utf-16-le')
        return len(before) + len(prefix)
    range_ = edit.get('range', edit.get('replace'))
    return text[:offset(range_['start'])] + edit['newText'] + text[offset(range_['end']):]


def run(args):
    project = ROOT / f'target/lsp-regression-{args.server}'
    source = project / 'entry/src/main/ets/Index.ets'
    element = project / 'entry/src/main/resources/base/element/string.json'
    media = element.parent.parent / 'media/icon.svg'
    for path in [source, element, media]:
        path.parent.mkdir(parents=True, exist_ok=True)
    source.write_text(UI)
    element.write_text(json.dumps({'string': [{'name': 'title', 'value': 'Title'}]}))
    media.write_text('<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"/>')
    (source.parent.parent / 'module.json5').write_text('{module: {name: "entry", type: "entry", srcEntry: "./ets/Index.ets"}}')
    (project / 'entry/build-profile.json5').write_text('{apiType: "stageMode", buildOption: {}, targets: [{name: "default"}]}')
    (project / 'build-profile.json5').write_text('{app: {products: [{name: "default", compatibleSdkVersion: 20}]}, modules: [{name: "entry", srcPath: "./entry", targets: [{name: "default", applyToProducts: ["default"]}]}]}')
    (project / 'tsconfig.json').write_text(json.dumps({'compilerOptions': {'strict': True, 'noEmit': True}, 'include': ['**/*.ets']}))
    (project / 'main.ets').write_text(MAIN)
    (project / 'math.ets').write_text(MATH)
    inherited = project / 'inherited'
    inherited.mkdir(exist_ok=True)
    (inherited / 'base.json').write_text('{"compilerOptions":{"strictNullChecks":false}}')
    (inherited / 'tsconfig.json').write_text('{"extends":"./base.json","include":["*.ets"]}')
    (inherited / 'nullable.ets').write_text('const nullable: string = null;\n')
    options = {'ets': {'sdkPath': args.sdk}, 'typescript': {'inlayHints': {'parameterNames': {'enabled': 'all'}}}}
    env = os.environ.copy()
    if args.server == 'legacy':
        env['ARKTS_LANGUAGE_SERVER_PATH'] = str(ROOT / 'target/baseline/node_modules/@arkts/language-server/bin/ets-language-server.js')
        command = ['node', str(ROOT / 'tools/legacy-lsp-wrapper.cjs')]
    else:
        command = [str(ROOT / args.native), '--stdio']
    args.output.mkdir(parents=True, exist_ok=True)
    client = Client(command, project, env, args.output / f'{args.server}-verification.stderr.log', options, measure=False)
    checks, responses = {}, {}

    def query(key, method, params):
        response, _ = client.request(method, params)
        responses[key] = response
        return response.get('result')

    def check(key, condition):
        checks[key] = bool(condition)

    def at(uri, text, needle, add=0):
        return {'textDocument': {'uri': uri}, 'position': position(text, needle, add)}

    def open_doc(path, text, version=1):
        client.notify('textDocument/didOpen', {'textDocument': {'uri': path.as_uri(), 'languageId': 'ets', 'version': version, 'text': text}})

    try:
        initialize = query('initialize', 'initialize', {'processId': os.getpid(), 'rootUri': project.as_uri(), 'workspaceFolders': [{'uri': project.as_uri(), 'name': 'fixture'}], 'locale': 'zh-CN', 'capabilities': capabilities(), 'initializationOptions': options})
        check('initialize', isinstance(initialize, dict) and 'capabilities' in initialize)
        check('utf16_negotiation', initialize['capabilities'].get('positionEncoding', 'utf-16') == 'utf-16')
        client.notify('initialized', {})
        for path, text in [(project / 'math.ets', MATH), (project / 'main.ets', MAIN), (source, UI)]:
            open_doc(path, text)
        uri = (project / 'main.ets').as_uri()
        hover = query('hover', 'textDocument/hover', at(uri, MAIN, 'greet(person,', 2))
        check('hover_documentation', hover and 'Greets a person' in json.dumps(hover))
        definition = query('definition', 'textDocument/definition', at(uri, MAIN, 'greet(person,', 2))
        check('cross_file_definition', definition and any('math.ets' in str(location) for location in definition))
        rename = query('rename', 'textDocument/rename', dict(at(uri, MAIN, 'person: Person', 2), newName='user'))
        check('rename_applied_edits', rename and any(edit.get('newText') == 'user' for edits in rename.get('changes', {}).values() for edit in edits))
        completion = query('completion', 'textDocument/completion', at(uri, MAIN, 'greeter.greet', 8))
        items = completion.get('items', []) if isinstance(completion, dict) else completion or []
        check('member_completion', any(i['label'] == 'greet' for i in items))
        if items:
            resolved = query('completionResolve', 'completionItem/resolve', next((i for i in items if i['label'] == 'greet'), items[0]))
            check('completion_resolve', resolved and resolved['label'] == 'greet')
        diagnostics = query('diagnostic', 'textDocument/diagnostic', {'textDocument': {'uri': uri}})
        check('type_error', any(i.get('code') == 2322 for i in (diagnostics or {}).get('items', [])))
        tokens = query('semanticTokens', 'textDocument/semanticTokens/range', {'textDocument': {'uri': uri}, 'range': {'start': {'line': 0, 'character': 0}, 'end': {'line': 7, 'character': 0}}})
        check('semantic_highlighting', tokens and bool(tokens.get('data')))
        signature = query('signatureHelp', 'textDocument/signatureHelp', at(uri, MAIN, 'greet(person, 2)', 13))
        check('signature_active_parameter', signature and signature.get('activeParameter') == 1)
        hints = query('inlayHint', 'textDocument/inlayHint', {'textDocument': {'uri': uri}, 'range': {'start': {'line': 0, 'character': 0}, 'end': {'line': 7, 'character': 0}}})
        check('configured_inlay_hints', bool(hints))
        hierarchy = query('callHierarchy', 'textDocument/prepareCallHierarchy', at(uri, MAIN, 'greet(person,', 2))
        if hierarchy:
            calls = query('incomingCalls', 'callHierarchy/incomingCalls', {'item': hierarchy[0]})
            check('call_hierarchy', bool(calls))
        else:
            check('call_hierarchy', False)
        file_edits = query('renameFile', 'workspace/willRenameFiles', {'files': [{'oldUri': (project / 'math.ets').as_uri(), 'newUri': (project / 'calculations.ets').as_uri()}]})
        check('file_rename_import_edits', file_edits and 'calculations' in json.dumps(file_edits))
        edited = MAIN.replace("'wrong'", '2')
        client.notify('textDocument/didChange', {'textDocument': {'uri': uri, 'version': 2}, 'contentChanges': [{'range': {'start': position(MAIN, "'wrong'"), 'end': position(MAIN, "'wrong'", len("'wrong'"))}, 'text': '2'}]})
        diagnostics = query('fixedDiagnostic', 'textDocument/diagnostic', {'textDocument': {'uri': uri}})
        check('incremental_edit_clears_type_error', not any(i.get('code') == 2322 for i in (diagnostics or {}).get('items', [])))
        # The edit column follows a surrogate pair on the same line.
        unicode_text = "const text = '🦀'; const n: number = 'bad';\n"
        client.notify('textDocument/didChange', {'textDocument': {'uri': uri, 'version': 3}, 'contentChanges': [{'text': unicode_text}]})
        client.notify('textDocument/didChange', {'textDocument': {'uri': uri, 'version': 4}, 'contentChanges': [{'range': {'start': position(unicode_text, "'bad'"), 'end': position(unicode_text, "'bad'", 5)}, 'text': '42'}]})
        diagnostics = query('unicodeDiagnostic', 'textDocument/diagnostic', {'textDocument': {'uri': uri}})
        check('utf16_surrogate_edit', diagnostics is not None and not any(i.get('code') == 2322 for i in diagnostics.get('items', [])))
        client.notify('textDocument/didClose', {'textDocument': {'uri': uri}})
        open_doc(project / 'main.ets', edited, 5)
        check('close_reopen', query('reopenHover', 'textDocument/hover', at(uri, edited, 'greet(person,', 2)) is not None)
        inherited_source = inherited / 'nullable.ets'
        open_doc(inherited_source, inherited_source.read_text())
        inherited_diags = query('inheritedDiagnostic', 'textDocument/diagnostic', {'textDocument': {'uri': inherited_source.as_uri()}})
        check('tsconfig_extends_precedence', inherited_diags is not None and not any(i.get('code') == 2322 and i.get('source') == 'ts' for i in inherited_diags.get('items', [])))
        ui_uri = source.as_uri()
        diagnostics = query('arkuiDiagnostic', 'textDocument/diagnostic', {'textDocument': {'uri': ui_uri}})
        ui_diags = (diagnostics or {}).get('items', [])
        check('arkui_grammar_and_sdk_globals', diagnostics is not None and not any(i.get('severity') == 1 and i['range']['start']['line'] < 14 for i in ui_diags))
        check('arkts_restriction_diagnostic', any(i.get('source') == 'arkts-linter' and i['range']['start']['line'] == 14 for i in ui_diags))
        check('invalid_resource_diagnostic', any(i.get('code') == 'APP_RESOURCE_NOT_FOUND' and 'missing' in i.get('message', '') for i in ui_diags))
        resource = query('resourceCompletion', 'textDocument/completion', at(ui_uri, UI, "app.string.');", len('app.string.')))
        resource_items = resource.get('items', []) if isinstance(resource, dict) else resource or []
        title = next((i for i in resource_items if i['label'].endswith('title')), None)
        check('resource_completion', title is not None)
        if title:
            text_edit = title.get('textEdit')
            if text_edit is None:
                cursor = position(UI, "app.string.');", len('app.string.'))
                text_edit = {'range': {'start': cursor, 'end': cursor}, 'newText': title.get('insertText', title['label'])}
            check('resource_completion_applies', "const partial = $r('app.string.title');" in apply_edit(UI, text_edit))
        partial = UI.replace("const partial = $r('app.string.');", "const partial = $r('app.string.tzzz');")
        client.notify('textDocument/didChange', {'textDocument': {'uri': ui_uri, 'version': 2}, 'contentChanges': [{'text': partial}]})
        middle = query('resourceCompletionMiddle', 'textDocument/completion', at(ui_uri, partial, 'app.string.tzzz', len('app.string.t')))
        middle_items = middle.get('items', []) if isinstance(middle, dict) else middle or []
        title = next((i for i in middle_items if i['label'].endswith('title')), None)
        check('resource_completion_mid_string', title and title.get('textEdit') and "const partial = $r('app.string.title');" in apply_edit(partial, title['textEdit']))
        client.notify('textDocument/didChange', {'textDocument': {'uri': ui_uri, 'version': 3}, 'contentChanges': [{'text': UI}]})
        sys = query('systemCompletion', 'textDocument/completion', at(ui_uri, UI, "sys.color.');", len('sys.color.')))
        check('system_resources', bool(sys.get('items') if isinstance(sys, dict) else sys))
        definition = query('resourceDefinition', 'textDocument/definition', at(ui_uri, UI, 'app.string.title', 6))
        check('resource_definition', definition and 'string.json' in str(definition))
        media_hover = query('mediaHover', 'textDocument/hover', at(ui_uri, UI, 'app.media.icon', 6))
        check('media_preview', media_hover and 'icon.svg' in str(media_hover))
        binding = query('bindingHover', 'textDocument/hover', at(ui_uri, UI, '$$this.isEnabled', 2))
        check('two_way_binding_docs', binding and '保持同步' in json.dumps(binding, ensure_ascii=False))
        links = query('documentLink', 'textDocument/documentLink', {'textDocument': {'uri': ui_uri}})
        check('document_links', links and 'icon.svg' in str(links))
        if links:
            check('document_link_resolve', query('documentLinkResolve', 'documentLink/resolve', links[0]) is not None)
        # Disk resource changes must be visible without restarting the server.
        element.write_text(json.dumps({'string': [{'name': 'title', 'value': 'Title'}, {'name': 'new_title', 'value': 'New'}]}))
        client.notify('workspace/didChangeWatchedFiles', {'changes': [{'uri': element.as_uri(), 'type': 2}]})
        updated = query('resourceReload', 'textDocument/completion', at(ui_uri, UI, "app.string.');", len('app.string.')))
        check('resource_watch_reload', 'new_title' in json.dumps(updated))
        # Changing only TS settings must retain the initialization SDK.
        client.notify('workspace/didChangeConfiguration', {'settings': {'typescript': {'inlayHints': {'parameterNames': {'enabled': 'all'}}}}})
        check('configuration_keeps_sdk', query('configHover', 'textDocument/hover', at(ui_uri, UI, 'Text(this.message)', 2)) is not None)
        # Pipeline requests and cancellation; a canceled request may finish or report -32800.
        replies = []
        for index in range(12):
            id_ = 10000 + index
            replies.append((id_, queue.Queue()))
            client.replies[id_] = replies[-1][1]
            client.send({'jsonrpc': '2.0', 'id': id_, 'method': 'textDocument/hover', 'params': at(ui_uri, UI, 'Text(this.message)', 2)})
        client.notify('$/cancelRequest', {'id': 10005})
        pipelined = [q.get(timeout=30) for _, q in replies]
        responses['pipelined'] = pipelined
        check('concurrent_requests_and_cancel', all('result' in r or r.get('error', {}).get('code') == -32800 for r in pipelined))
        for id_, _ in replies:
            client.replies.pop(id_, None)
    finally:
        client.close()
    result = {'server': args.server, 'checks': checks, 'passed': sum(checks.values()), 'total': len(checks), 'responses': responses, 'exit_code': client.process.returncode}
    path = args.output / f'{args.server}-verification.json'
    path.write_text(json.dumps(result, indent=2, ensure_ascii=False))
    print(json.dumps({k: v for k, v in result.items() if k != 'responses'}, ensure_ascii=False))
    if args.server == 'native' and not all(checks.values()):
        raise SystemExit(1)


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--sdk', default=DEFAULT_SDK)
    parser.add_argument('--native', default='target/tsrs/release/arkts-lsp')
    parser.add_argument('--server', choices=['native', 'legacy'], default='native')
    parser.add_argument('--output', type=Path, default=ROOT / 'reports/raw')
    run(parser.parse_args())
