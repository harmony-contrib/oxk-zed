#!/usr/bin/env python3
"""Run the old and native servers with identical SDK, workspace, requests and edits.

Results include raw protocol responses, process-tree sampled RSS, latency and time(1) counters.
Legacy dependencies live only in target/baseline and are never used by the extension.
"""
import argparse
import json
import os
from pathlib import Path
import queue
import statistics
import subprocess
import threading
import time
import re
import math
import platform
import hashlib

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_SDK = '/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony'


class Client:
    def __init__(self, command, cwd, env, log, options, measure=True):
        self.started = time.perf_counter()
        self.log = open(log, 'wb')
        self.process = subprocess.Popen((['/usr/bin/time', '-l', *command] if measure else command), cwd=cwd, env=env,
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.log)
        self.options = options
        self.replies = {}
        self.notifications = []
        self.next_id = 0
        self.lock = threading.Lock()
        self.done = False
        self.memory = []
        self.sample_error = None
        self.log_path = Path(log)
        self.reader = threading.Thread(target=self.read, daemon=True)
        self.reader.start()
        self.sampler = threading.Thread(target=self.sample, daemon=True)
        if measure:
            self.sampler.start()

    def send(self, message):
        payload = json.dumps(message, ensure_ascii=False).encode()
        with self.lock:
            self.process.stdin.write(f'Content-Length: {len(payload)}\r\n\r\n'.encode() + payload)
            self.process.stdin.flush()

    def read(self):
        try:
            while True:
                length = None
                while True:
                    line = self.process.stdout.readline()
                    if not line:
                        return
                    if line == b'\r\n':
                        break
                    if line.lower().startswith(b'content-length:'):
                        length = int(line.split(b':')[1])
                data = self.process.stdout.read(length)
                message = json.loads(data)
                if 'method' in message:
                    if 'id' in message:
                        result = None
                        if message['method'] == 'workspace/configuration':
                            result = [self.configuration(item.get('section')) for item in message['params']['items']]
                        self.send({'jsonrpc': '2.0', 'id': message['id'], 'result': result})
                    else:
                        self.notifications.append(message)
                elif message.get('id') in self.replies:
                    self.replies[message['id']].put(message)
        except (OSError, ValueError, TypeError):
            pass

    def sample(self):
        while not self.done:
            start = time.perf_counter()
            try:
                sample = subprocess.run(['ps', '-axo', 'pid=,ppid=,rss='], capture_output=True, text=True)
                if sample.returncode:
                    self.sample_error = sample.stderr.strip()
                    return
                output = sample.stdout
            except OSError as error:
                self.sample_error = str(error)
                return
            rows = [tuple(map(int, line.split())) for line in output.splitlines() if len(line.split()) == 3]
            pids = {self.process.pid}
            for _ in range(8):
                pids.update(pid for pid, parent, _ in rows if parent in pids)
            rss = sum(rss for pid, _, rss in rows if pid in pids and pid != self.process.pid)
            self.memory.append([time.perf_counter() - self.started, rss / 1024])
            time.sleep(max(0, .05 - (time.perf_counter() - start)))

    def request(self, method, params, timeout=60):
        self.next_id += 1
        id_ = self.next_id
        self.replies[id_] = queue.Queue()
        start = time.perf_counter()
        message = {'jsonrpc': '2.0', 'id': id_, 'method': method}
        if params is not None:
            message['params'] = params
        self.send(message)
        try:
            result = self.replies[id_].get(timeout=timeout)
        except queue.Empty:
            raise RuntimeError(f'{method} timed out; server exit={self.process.poll()}')
        finally:
            self.replies.pop(id_, None)
        return result, (time.perf_counter() - start) * 1000

    def configuration(self, section):
        if section in self.options:
            return self.options[section]
        value = self.options
        for part in (section or '').split('.'):
            value = value.get(part, {}) if isinstance(value, dict) else {}
        return value

    def notify(self, method, params):
        message = {'jsonrpc': '2.0', 'method': method}
        if params is not None:
            message['params'] = params
        self.send(message)

    def close(self):
        try:
            self.request('shutdown', None, timeout=10)
            self.notify('exit', None)
            self.process.stdin.close()
            self.process.wait(timeout=15)
        except (OSError, RuntimeError, subprocess.TimeoutExpired):
            self.process.terminate()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait()
        finally:
            self.done = True
            if self.sampler.ident:
                self.sampler.join(timeout=2)
            self.log.close()


MATH = '''export interface Person { name: string; age: number; }
/** Greets a person. */
export function greet(person: Person, times: number = 1): string {
  return person.name.repeat(times);
}
export class Greeter { greet(person: Person): string { return greet(person); } }
'''
MAIN = '''import { greet, Greeter, Person } from './math';
// 中文和 emoji 🦀 verify UTF-16 positions.
const person: Person = { name: 'ArkTS', age: 20 };
const result = greet(person, 2);
const greeter = new Greeter();
greeter.greet(person);
const broken: number = 'wrong';
'''


def position(text, needle, add=0):
    offset = text.index(needle) + add
    before = text[:offset]
    return {'line': before.count('\n'), 'character': len(before.rsplit('\n', 1)[-1].encode('utf-16-le')) // 2}


def prepare(path, files):
    path.mkdir(parents=True, exist_ok=True)
    (path / 'math.ets').write_text(MATH)
    (path / 'main.ets').write_text(MAIN)
    (path / 'tsconfig.json').write_text(json.dumps({'compilerOptions': {'strict': True, 'noEmit': True}, 'include': ['*.ets']}))
    for index in range(files):
        (path / f'module{index:04}.ets').write_text(f"import {{greet}} from './math';\nexport const message{index} = greet({{name:'item{index}', age:{index}}});\n" * 1)


def run(name, args, project):
    options = {'ets': {'sdkPath': args.sdk}, 'typescript': {'inlayHints': {'parameterNames': {'enabled': 'all'}}}}
    env = os.environ.copy()
    if name == 'legacy':
        env['ARKTS_LANGUAGE_SERVER_PATH'] = str(ROOT / 'target/baseline/node_modules/@arkts/language-server/bin/ets-language-server.js')
        command = ['node', str(ROOT / 'tools/legacy-lsp-wrapper.cjs')]
    else:
        command = [str(ROOT / args.native), '--stdio']
    implementation = {'version': json.loads((ROOT / 'target/baseline/node_modules/@arkts/language-server/package.json').read_text())['version'], 'wrapper_sha256': hashlib.sha256((ROOT / 'tools/legacy-lsp-wrapper.cjs').read_bytes()).hexdigest()} if name == 'legacy' else {'binary_sha256': hashlib.sha256(Path(command[0]).read_bytes()).hexdigest()}
    log = args.output / f'{name}-{args.files}-{args.run}.stderr.log'
    client = Client(command, project, env, log, options)
    responses, latency = {}, {}
    uri = (project / 'main.ets').as_uri()
    textdoc = {'uri': uri}
    try:
        init = {'processId': os.getpid(), 'rootUri': project.as_uri(), 'workspaceFolders': [{'uri': project.as_uri(), 'name': 'fixture'}],
                'capabilities': {'workspace': {'configuration': True, 'workspaceFolders': True, 'didChangeWatchedFiles': {'dynamicRegistration': True}},
                    'textDocument': {'hover': {'contentFormat': ['markdown']}, 'completion': {'completionItem': {'snippetSupport': True, 'labelDetailsSupport': True, 'insertReplaceSupport': True}},
                        'definition': {'linkSupport': True}, 'rename': {'prepareSupport': True}, 'documentSymbol': {'hierarchicalDocumentSymbolSupport': True},
                        'codeAction': {'codeActionLiteralSupport': {'codeActionKind': {'valueSet': ['quickfix','source.organizeImports','refactor']}}},
                        'publishDiagnostics': {'relatedInformation': True}, 'inlayHint': {}, 'diagnostic': {'dynamicRegistration': False}},
                    'general': {'positionEncodings': ['utf-16']}}, 'initializationOptions': options}
        responses['initialize'], latency['initialize'] = client.request('initialize', init)
        startup = (time.perf_counter() - client.started) * 1000
        client.notify('initialized', {})
        for file, text in [('math.ets', MATH), ('main.ets', MAIN)]:
            client.notify('textDocument/didOpen', {'textDocument': {'uri': (project / file).as_uri(), 'languageId': 'ets', 'version': 1, 'text': text}})
        queries = [
            ('hover', 'textDocument/hover', {'textDocument': textdoc, 'position': position(MAIN, 'greet(person,', 2)}),
            ('definition', 'textDocument/definition', {'textDocument': textdoc, 'position': position(MAIN, 'greet(person,', 2)}),
            ('typeDefinition', 'textDocument/typeDefinition', {'textDocument': textdoc, 'position': position(MAIN, 'person: Person', 2)}),
            ('references', 'textDocument/references', {'textDocument': textdoc, 'position': position(MAIN, 'greet(person,', 2), 'context': {'includeDeclaration': True}}),
            ('prepareRename', 'textDocument/prepareRename', {'textDocument': textdoc, 'position': position(MAIN, 'person: Person', 2)}),
            ('rename', 'textDocument/rename', {'textDocument': textdoc, 'position': position(MAIN, 'person: Person', 2), 'newName': 'user'}),
            ('completion', 'textDocument/completion', {'textDocument': textdoc, 'position': position(MAIN, 'greeter.greet(person)', 8), 'context': {'triggerKind': 1}}),
            ('signatureHelp', 'textDocument/signatureHelp', {'textDocument': textdoc, 'position': position(MAIN, 'greet(person, 2)', 13), 'context': {'triggerKind': 1, 'isRetrigger': False}}),
            ('documentSymbol', 'textDocument/documentSymbol', {'textDocument': textdoc}),
            ('documentHighlight', 'textDocument/documentHighlight', {'textDocument': textdoc, 'position': position(MAIN, 'person: Person', 2)}),
            ('foldingRange', 'textDocument/foldingRange', {'textDocument': {'uri': (project / 'math.ets').as_uri()}}),
            ('selectionRange', 'textDocument/selectionRange', {'textDocument': textdoc, 'positions': [position(MAIN, 'greet(person,', 2)]}),
            ('inlayHint', 'textDocument/inlayHint', {'textDocument': textdoc, 'range': {'start': {'line': 0, 'character': 0}, 'end': {'line': 7, 'character': 0}}}),
            ('diagnostic', 'textDocument/diagnostic', {'textDocument': textdoc}),
            ('workspaceSymbol', 'workspace/symbol', {'query': 'greet'}),
            ('codeAction', 'textDocument/codeAction', {'textDocument': textdoc, 'range': {'start': {'line': 0, 'character': 0}, 'end': {'line': 7, 'character': 0}}, 'context': {'diagnostics': [], 'only': ['source.organizeImports']}}),
        ]
        for key, method, params in queries:
            responses[key], latency[key] = client.request(method, params)
        completions = responses['completion'].get('result') or {}
        items = completions.get('items', []) if isinstance(completions, dict) else completions
        if items:
            responses['completionResolve'], latency['completionResolve'] = client.request('completionItem/resolve', items[0])
        warm = []
        for _ in range(args.requests):
            _, elapsed = client.request('textDocument/hover', queries[0][2])
            warm.append(elapsed)
        time.sleep(.75)
        def settled_rss():
            return statistics.median(s[1] for s in client.memory[-5:]) if client.memory else 0
        idle_rss = settled_rss()
        edits = []
        for index in range(args.edits):
            # Full source replacement includes a non-ASCII prefix; follow with a request as an ordering barrier.
            text = MAIN.replace("'wrong'", str(index))
            client.notify('textDocument/didChange', {'textDocument': {'uri': uri, 'version': index + 2}, 'contentChanges': [{'text': text}]})
            _, elapsed = client.request('textDocument/hover', queries[0][2])
            edits.append(elapsed)
            if args.edits >= 1000 and (index + 1) % 500 == 0:
                print(json.dumps({'server': name, 'completed_edits': index + 1, 'rss_mib': settled_rss()}), flush=True)
        responses['diagnosticAfterEdits'], latency['diagnosticAfterEdits'] = client.request('textDocument/diagnostic', {'textDocument': textdoc})
        time.sleep(.75)
        edited_rss = settled_rss()
        client.notify('textDocument/didClose', {'textDocument': textdoc})
        client.notify('textDocument/didOpen', {'textDocument': {'uri': uri, 'languageId': 'ets', 'version': args.edits + 2, 'text': MAIN}})
        responses['hoverAfterReopen'], _ = client.request('textDocument/hover', queries[0][2])
        time.sleep(.75)
        reopened_rss = settled_rss()
    finally:
        client.close()
    if client.sample_error:
        raise RuntimeError(f'Process-tree RSS measurement failed: {client.sample_error}')
    def timing(values):
        values = sorted(values)
        return {'median_ms': statistics.median(values) if values else 0, 'p95_ms': values[math.ceil(len(values)*.95)-1] if values else 0}
    result = {'server': name, 'implementation': implementation, 'files': args.files + 2, 'requests': args.requests, 'edits': args.edits,
              'startup_ms': startup, 'latency_ms': latency, 'warm_hover': timing(warm), 'edit_hover': timing(edits),
              'idle_rss_mib': idle_rss, 'after_edits_rss_mib': edited_rss, 'peak_tree_rss_mib': max((s[1] for s in client.memory), default=0),
              'after_reopen_rss_mib': reopened_rss,
              'memory_samples': client.memory, 'responses': responses, 'notifications': client.notifications, 'exit_code': client.process.returncode}
    counters = re.search(r'([\d.]+) real\s+([\d.]+) user\s+([\d.]+) sys', client.log_path.read_text())
    if counters:
        result['process_time_seconds'] = dict(zip(['real', 'user', 'sys'], map(float, counters.groups())))
    output = args.output / f'{name}-{args.files}-{args.run}.json'
    output.write_text(json.dumps(result, indent=2, ensure_ascii=False))
    print(json.dumps({k: v for k, v in result.items() if k not in ('responses', 'notifications', 'memory_samples')}), flush=True)
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--sdk', default=DEFAULT_SDK)
    parser.add_argument('--native', default='target/tsrs/release/arkts-lsp')
    parser.add_argument('--server', choices=['legacy', 'native', 'both'], default='both')
    parser.add_argument('--files', type=int, default=100)
    parser.add_argument('--requests', type=int, default=100)
    parser.add_argument('--edits', type=int, default=200)
    parser.add_argument('--run', type=int, default=1)
    parser.add_argument('--native-first', action='store_true', help='Alternate process order to reduce cache/order bias')
    parser.add_argument('--suite', action='store_true', help='Three alternating runs each with 102 and 1002 source files')
    parser.add_argument('--pressure', action='store_true', help='Append one 2000-edit comparison on 102 source files')
    parser.add_argument('--output', type=Path, default=ROOT / 'reports/raw')
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    hardware = subprocess.check_output(['sysctl', '-n', 'machdep.cpu.brand_string', 'hw.memsize'], text=True).splitlines()
    (args.output / 'environment.json').write_text(json.dumps({'cpu': hardware[0], 'physical_memory_bytes': int(hardware[1]), 'architecture': platform.machine()}, indent=2) + '\n')
    for size in [100, 1000] if args.suite else [args.files]:
        args.files = size
        project = ROOT / f'target/benchmark-project-{size}'
        prepare(project, size)
        for repetition in range(1, 4) if args.suite else [args.run]:
            args.run = repetition
            native_first = repetition % 2 == 0 if args.suite else args.native_first
            for name in (['native', 'legacy'] if native_first else ['legacy', 'native']) if args.server == 'both' else [args.server]:
                run(name, args, project)
    if args.pressure:
        args.files, args.edits, args.run = 100, 2000, 9
        project = ROOT / 'target/benchmark-project-100'
        prepare(project, args.files)
        for name in ['legacy', 'native'] if args.server == 'both' else [args.server]:
            run(name, args, project)


if __name__ == '__main__':
    main()
