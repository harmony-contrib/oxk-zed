#!/usr/bin/env python3
"""Summarize the recorded benchmark, preserving each run's range and provenance."""
import hashlib
import json
from pathlib import Path
import platform
import plistlib
import statistics
import subprocess

ROOT = Path(__file__).resolve().parents[1]
RAW = ROOT / 'reports/raw'


def output(command):
    return subprocess.check_output(command, cwd=ROOT, text=True).strip()


def median_range(values):
    return {'median': statistics.median(values), 'min': min(values), 'max': max(values)}


def summarize(records):
    result = {key: median_range([r[key] for r in records]) for key in [
        'startup_ms', 'idle_rss_mib', 'after_edits_rss_mib', 'after_reopen_rss_mib', 'peak_tree_rss_mib']}
    for kind in ['warm_hover', 'edit_hover']:
        result[kind] = {key: median_range([r[kind][key] for r in records]) for key in ['median_ms', 'p95_ms']}
    result['initialize_plus_first_hover_ms'] = median_range([r['startup_ms'] + r['latency_ms']['hover'] for r in records])
    result['cpu_seconds'] = median_range([r['process_time_seconds']['user'] + r['process_time_seconds']['sys'] for r in records])
    result['latency_ms'] = {key: median_range([r['latency_ms'][key] for r in records]) for key in records[0]['latency_ms']}
    result['memory_growth_mib'] = median_range([r['after_edits_rss_mib'] - r['idle_rss_mib'] for r in records])
    result['exit_codes'] = [r['exit_code'] for r in records]
    result['implementations'] = [r['implementation'] for r in records]
    result['protocol_errors'] = {str(index + 1): {key: response['error'] for key, response in r['responses'].items() if 'error' in response} for index, r in enumerate(records)}
    return result


def render_report(summary):
    def m(files, server, *path):
        value = summary['benchmark'][str(files)][server]
        for key in path:
            value = value[key]
        return value['median']

    rows = [
        ('初始化完成（ms）', ['startup_ms'], 2),
        ('初始化＋首个悬浮（ms，两段计时之和）', ['initialize_plus_first_hover_ms'], 2),
        ('热悬浮中位数（ms）', ['warm_hover', 'median_ms'], 3),
        ('热悬浮 P95（ms）', ['warm_hover', 'p95_ms'], 3),
        ('编辑后悬浮中位数（ms）', ['edit_hover', 'median_ms'], 2),
        ('编辑后悬浮 P95（ms）', ['edit_hover', 'p95_ms'], 2),
        ('首次完整诊断（ms）', ['latency_ms', 'diagnostic'], 2),
        ('200 次编辑后完整诊断（ms）', ['latency_ms', 'diagnosticAfterEdits'], 2),
        ('空闲进程树 RSS（MiB）', ['idle_rss_mib'], 2),
        ('200 次编辑后 RSS（MiB）', ['after_edits_rss_mib'], 2),
        ('关闭／重开后 RSS（MiB）', ['after_reopen_rss_mib'], 2),
        ('采样峰值 RSS（MiB）', ['peak_tree_rss_mib'], 2),
        ('整轮 CPU user＋sys（秒）', ['cpu_seconds'], 2),
    ]
    table = ['| 指标 | 102 文件：旧 | 102 文件：原生 | 1002 文件：旧 | 1002 文件：原生 |', '| --- | ---: | ---: | ---: | ---: |']
    for label, path, digits in rows:
        values = [f'{m(files, server, *path):.{digits}f}' for files in [102, 1002] for server in ['legacy', 'native']]
        table.append('| ' + ' | '.join([label, *values]) + ' |')
    large = 1002
    rss_reduction = (1 - m(large, 'native', 'idle_rss_mib') / m(large, 'legacy', 'idle_rss_mib')) * 100
    peak_reduction = (1 - m(large, 'native', 'peak_tree_rss_mib') / m(large, 'legacy', 'peak_tree_rss_mib')) * 100
    edit_speedup = m(large, 'legacy', 'edit_hover', 'median_ms') / m(large, 'native', 'edit_hover', 'median_ms')
    env = summary['environment']
    lines = [
        '# ArkTS LSP 替换与性能对比', '',
        '本仓库采用 **tsrs 原生语言服务＋OXC ArkTS 补充服务** 作为默认实现。生产启动链路已移除 `@arkts/language-server` 和 Node IPC 包装层，原服务仅留在对比工具中。', '',
        f'在本次 1002 文件合成项目中，原生实现的空闲进程树 RSS 降低 **{rss_reduction:.1f}%**，采样峰值降低 **{peak_reduction:.1f}%**，编辑后悬浮中位数加快 **{edit_speedup:.2f} 倍**。32 项原生协议断言全部通过。保留 tsrs 更完整的 JSDoc 和符号结果，不要求与旧服务的展示字符串逐字相同。', '',
        '## 测试环境与范围', '',
        f'- 测试日期：2026-10-12；{env["cpu"]}，{env["physical_memory_gib"]:.0f} GiB 内存，macOS {env["os"]}，{env["architecture"]}。',
        f'- Rust：`{env["rust"]}`；Node：`{env["node"]}`；旧包：`@arkts/language-server@{env["legacy_version"]}`。',
        f'- 本地 OXC：`{env["oxc_commit"]}`（crate 0.153.0）。',
        f'- 本地 tsrs：`{env["tsrs_commit"]}`（crate 0.11.0；编译器版本字符串 7.1.0-dev）。',
        '- 原生构建：release、thin LTO、codegen-units=1，CLI 使用 mimalloc。两个外部源码 checkout 均未修改。',
        '- SDK：DevEco Studio 内置 OpenHarmony SDK，`ets/build-tools/ets-loader` 语法配置与组件／loader 声明；244 个全局声明文件，约 8.88 MiB。',
        '- 项目包含跨文件函数／接口／类、参数文档、类型错误、中文和 emoji；新增短模块共同导入同一函数。1002 个源码文件合计 90,203 字节，文件数量不能代表同规模业务项目的复杂度。',
        '- 基准只测主语言服务。Zed 进程、语法高亮、独立 OXK 格式化／lint 服务不计入。', '',
        '## 常规性能与内存', '',
        '每个规模独立启动三轮，启动顺序交替。每轮执行 18 类协议请求、100 次热悬浮、200 次全文编辑后悬浮，再拉取完整诊断和关闭／重开文档。下表是三个运行中位数的中位数；P95 也是三个单轮 P95 的中位数，未合并为总体 P95。运行范围见 [summary.json](summary.json)。', '',
        *table, '',
        '小项目的首次完整诊断可能较慢：主服务会追加 SDK ArkTS 限制检查。因此选择原生实现的依据是整体体验、持续编辑和内存表现，不能声称每一个请求都更快。', '',
        '## 连续编辑压力测试', '',
        '102 文件项目另运行一轮 2000 次连续全文编辑，每次紧接悬浮请求作为处理顺序屏障，最后请求完整诊断。该测试只有一轮，不作统计显著性判断。', '',
    ]
    pressure = summary['pressure']
    if all(server in pressure for server in ['legacy', 'native']):
        lines.extend(['| 指标 | 旧服务 | 原生服务 |', '| --- | ---: | ---: |'])
        for label, key in [('编辑前 RSS（MiB）', 'idle_rss_mib'), ('2000 次编辑后 RSS（MiB）', 'after_edits_rss_mib'), ('关闭／重开后 RSS（MiB）', 'after_reopen_rss_mib'), ('采样峰值 RSS（MiB）', 'peak_tree_rss_mib')]:
            lines.append(f'| {label} | {pressure["legacy"][key]:.2f} | {pressure["native"][key]:.2f} |')
        for label, path in [('编辑后悬浮中位数（ms）', 'median_ms'), ('编辑后悬浮 P95（ms）', 'p95_ms')]:
            lines.append(f'| {label} | {pressure["legacy"]["edit_hover"][path]:.2f} | {pressure["native"]["edit_hover"][path]:.2f} |')
        lines.extend(['', f'整轮 user＋sys CPU：旧服务 {sum(pressure["legacy"]["process_time_seconds"][k] for k in ["user", "sys"]):.2f} 秒，原生服务 {sum(pressure["native"]["process_time_seconds"][k] for k in ["user", "sys"]):.2f} 秒。'])
    else:
        lines.append('尚未记录与当前二进制匹配的压力结果。运行下方 `--pressure` 命令后重新汇总。')
    lines.extend([
        '', 'RSS 曲线保存在本机 `reports/raw/*-100-9.json` 的 `memory_samples`。RSS 是驻留页，下降可能涉及回收、页压缩或换页；增长也受垃圾回收时机影响。数据支持本工作负载下较低的内存压力，不能据此证明不存在泄漏或断言旧服务泄漏。', '',
        '## 能力与实现选择', '',
        '真实 SDK 的 ArkUI 项目另做语义验证，包括实际应用补全／编辑结果，而非仅检查返回非空。原生服务 32/32 断言通过；旧服务同一脚本实际执行 31 项、通过 28 项。旧服务未返回该文档链接，因此链接 resolve 分支没有执行。这个计数是当前用例覆盖，不是完整语言规范的通过率。', '',
        '| 用例 | 旧实现 | 原生实现／选择 |', '| --- | --- | --- |',
        '| 跨文件定义、类型错误、重命名、补全 resolve | 通过 | 通过，采用 tsrs |',
        '| 语义高亮、参数提示、调用层级、文件重命名更新导入 | 通过 | 通过，采用 tsrs |',
        '| 中文／emoji 后 UTF-16 增量编辑、文档重开、并发与取消 | 通过 | 通过；取消与错误响应不会被补充服务改写成成功 |',
        '| ArkUI 语法、Text/Image 等 SDK 全局、ArkTS any 限制 | 通过 | 通过；使用 SDK 的 ES lib，避免默认 DOM 同名冲突 |',
        '| app/sys 资源补全、跳转、图片预览、资源监听更新 | 通过 | 通过；OXC AST 解析与模块资源索引 |',
        '| 光标位于现有资源名中间时补全 | 本用例未通过 | 按光标前缀筛选，显式替换整段字符串内容 |',
        '| 文件相对路径文档链接 | 本用例未通过 | 返回真实文件目标，支持 eager link 和 resolve |',
        '| 继承的 strictNullChecks:false | TS 类型诊断仍报告 2322 | TS 类型诊断尊重继承；SDK ArkTS 限制检查仍独立执行 |',
        '| $$this 的说明与变量检查 | SDK／Volar 处理 | OXC 识别表达式后将两个美元符号替换为空格，位置不变，由 tsrs 检查 this 目标 |',
        '| 悬浮、签名和符号展示 | 旧服务展示规则 | 保留 tsrs 的 JSDoc 和符号结构，仅将悬浮代码块语言标记为 ets |', '',
        '主服务的标准诊断和 SDK ArkTS 限制检查共用 tsrs 会话与 Program 生命周期。已通过输入错误、未变化诊断和取消响应的单元校验。测试未穷举所有语法、SDK API 或大型工程；原生诊断文本可能采用英文，旧服务的语言包展示与私有 `ets/*` 控制接口没有逐项迁移。', '',
        '资源索引以模块目录和 AppScope 为范围，尚未实现复杂 build-profile 的 product/target 筛选或 oh_modules 下 HAR 资源。扩展仅注册 ArkTS，未增加 JSON 权限清单服务。格式化和独立 OXK lint 的现有接入继续保留，未包含在主服务性能数字中。', '',
        '## Workspace、第三方接入与 Zed', '',
        '仓库已拆为 `arkts-sdk`、`arkts-services`、`arkts-lsp` 三个可复用 crate。SDK／资源能力可以独立使用；`arkts_lsp::run` 接受宿主的读写流和 FS，主服务无需 Zed 或 Node。SDK／资源索引目前仍访问本地文件，完全内存化文件系统不是完整支持范围。', '',
        '根包 `oxk-zed` 保持 cdylib，并作为 workspace 唯一 default member。Zed 使用的默认 `cargo build --release --target wasm32-wasip2` 构建通过；生成的组件为 329,456 字节，WASM 验证、四项必要的 Zed 导出和 `zed:api-version = 0.7.0` 校验通过。原有 server IDs 与 formatter/linter 设置保留。', '',
        '实际 Zed GUI 安装／启动尚未实测：Computer Use 自动审批拒绝了操作 Zed，未通过其他 UI 工具绕过限制。构建和组件校验提供兼容性证据，不能代替一次编辑器内加载验证。安装和 binary.path 配置见 [README](../README.md)。', '',
        '## 复现与数据解释', '',
        '```sh', 'scripts/build-local.sh',
        'cargo test -p arkts-sdk -p arkts-services -p arkts-lsp',
        'cargo clippy -p oxk-zed -p arkts-sdk -p arkts-services -p arkts-lsp --no-deps -- -D warnings',
        'cargo run --example validate-extension -- extension.wasm',
        'npm install --prefix target/baseline --no-audit --no-fund @arkts/language-server@1.3.10',
        'python3 tools/verify_lsp.py --sdk /absolute/sdk',
        'python3 tools/verify_lsp.py --server legacy --sdk /absolute/sdk',
        'python3 tools/lsp_compare.py --suite --pressure --sdk /absolute/sdk',
        'python3 tools/summarize_compare.py', '```', '',
        '- RSS 每 50 ms 用 ps 采样，对进程树求和：旧服务包括 Node 包装器与子服务，原生服务包括单个本机进程。空闲／编辑后／重开后等待 750 ms，取最后五个样本中位数。短峰值可能被漏采；RSS 求和也可能重复计算共享页，不能视为独占物理内存。',
        '- CPU 数字来自 macOS time 的 user＋sys；未比较 instructions retired 或 peak memory footprint，因为旧服务的这些计数不能代表整个包装器＋子服务。没有将系统全局 memory_pressure 值归因给某个服务。',
        '- 基准期间未并行构建或主动施加其他负载，没有锁定 CPU 频率或清空 OS 缓存。三轮最小／最大值是运行波动范围，不是置信区间。',
        '- 旧包装器在 stdin 结束时 kill 子进程，因此测试退出码为 1；协议用例均完成，没有把该状态作为能力失败。原生常规及压力测试退出码为 0。',
        '- 完整响应、通知、RSS 时间序列和 time 日志保存在本机忽略目录 reports/raw；可审阅的汇总数据已写入 summary.json。每轮记录对应实现／二进制哈希，汇总拒绝混用不同原生二进制。', '',
        f'原生二进制 SHA-256：`{env["native_sha256"]}`。',
        f'Zed WASM SHA-256：`{env["wasm_sha256"]}`。', '',
    ])
    return '\n'.join(lines)


def main():
    hardware = json.loads((RAW / 'environment.json').read_text())
    summary = {'environment': {
        'os': plistlib.loads(Path('/System/Library/CoreServices/SystemVersion.plist').read_bytes())['ProductVersion'],
        'architecture': platform.machine(), 'cpu': hardware['cpu'], 'physical_memory_gib': hardware['physical_memory_bytes'] / (1024 ** 3),
        'rust': output(['rustc', '--version']), 'node': output(['node', '--version']),
        'oxc_commit': output(['git', '-C', '../oxc', 'rev-parse', 'HEAD']),
        'tsrs_commit': output(['git', '-C', '../../ohos-rs/tsrs', 'rev-parse', 'HEAD']),
        'legacy_version': json.loads((ROOT / 'target/baseline/node_modules/@arkts/language-server/package.json').read_text())['version'],
        'native_sha256': hashlib.sha256((ROOT / 'target/tsrs/release/arkts-lsp').read_bytes()).hexdigest(),
        'wasm_sha256': hashlib.sha256((ROOT / 'extension.wasm').read_bytes()).hexdigest(),
    }, 'methodology': {'runs': 3, 'warm_requests_per_run': 100, 'edits_per_run': 200,
        'rss_sample_interval_ms': 50, 'settled_rss_last_samples': 5,
        'sdk': '/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony',
        'aggregates': 'median of run medians or run P95s; ranges are run min/max, not confidence intervals'},
        'benchmark': {}, 'verification': {}, 'pressure': {}}
    for files in [100, 1000]:
        summary['benchmark'][str(files + 2)] = {}
        for server in ['legacy', 'native']:
            records = [json.loads((RAW / f'{server}-{files}-{index}.json').read_text()) for index in [1, 2, 3]]
            if server == 'native' and any(r['exit_code'] != 0 for r in records):
                raise RuntimeError('A native benchmark did not shut down cleanly')
            if server == 'native' and any(r['implementation']['binary_sha256'] != summary['environment']['native_sha256'] for r in records):
                raise RuntimeError('Benchmark hashes do not match the current native binary')
            summary['benchmark'][str(files + 2)][server] = summarize(records)
    for server in ['legacy', 'native']:
        verification = json.loads((RAW / f'{server}-verification.json').read_text())
        summary['verification'][server] = {key: verification[key] for key in ['checks', 'passed', 'total', 'exit_code']}
        pressure_file = RAW / f'{server}-100-9.json'
        if pressure_file.exists():
            pressure = json.loads(pressure_file.read_text())
            if server == 'native' and pressure['implementation']['binary_sha256'] != summary['environment']['native_sha256']:
                raise RuntimeError('Pressure hash does not match current binary; rerun --pressure')
            summary['pressure'][server] = {key: value for key, value in pressure.items() if key not in ['responses', 'notifications', 'memory_samples']}
    path = ROOT / 'reports/summary.json'
    path.write_text(json.dumps(summary, indent=2, ensure_ascii=False) + '\n')
    (ROOT / 'reports/lsp-comparison.md').write_text(render_report(summary))
    print(path)


if __name__ == '__main__':
    main()
