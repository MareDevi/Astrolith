#!/usr/bin/env node
// Reproducible native editor workloads for the performance implementation plan.
//
//   node scripts/editor-perf.mjs fixtures [--out DIR]
//   node scripts/editor-perf.mjs run --fixture ID --backend pre|fast [options]
//   node scripts/editor-perf.mjs ab [options]
//
// Fixtures are deterministic and never committed: the generator records their
// byte counts and hashes in a manifest, and the run artifacts reference that
// manifest. Each run uses its own HOME, config, state, cache, runtime directory
// and preferences registry, and a project-local Astro discovery stub.
//
// The app side is opt-in: `ASTROLITH_EDITOR_PERF=1` enables numeric counters and
// `ASTROLITH_PERF_SCENARIO` drives mode/scroll/input steps. A development build
// logs target `astrolith::perf`; nothing text-like is logged.

import { createHash } from 'node:crypto';
import { spawn, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import process from 'node:process';

const ROOT = path.resolve(import.meta.dirname, '..');
const GENERATOR_VERSION = 1;
const DEFAULT_FIXTURE_DIR = path.join(ROOT, 'target/editor-perf/fixtures');
const ARTIFACT_ROOT = path.join(ROOT, 'artifacts/e2e');

function parseArgs(argv) {
  const args = { _: [] };
  for (let index = 0; index < argv.length; index += 1) {
    const value = argv[index];
    if (value.startsWith('--')) {
      const [key, inline] = value.slice(2).split('=', 2);
      if (inline !== undefined) {
        args[key] = inline;
      } else if (argv[index + 1] && !argv[index + 1].startsWith('--')) {
        args[key] = argv[index + 1];
        index += 1;
      } else {
        args[key] = true;
      }
    } else {
      args._.push(value);
    }
  }
  return args;
}

function byteLength(text) {
  return Buffer.byteLength(text, 'utf8');
}

/** Truncate at a UTF-8 code point boundary, then pad with ASCII spaces. */
function exactBytes(unit, bytes) {
  if (!unit.length) throw new Error('fixture unit must not be empty');
  let text = unit.repeat(Math.ceil(bytes / byteLength(unit)) + 1);
  while (byteLength(text) > bytes) {
    text = text.slice(0, -1);
  }
  const missing = bytes - byteLength(text);
  return text + ' '.repeat(missing);
}

// Long prose lines keep the historical byte sizes under the application's
// 50,000-line document limit while still producing many semantic blocks.
const proseBlock =
  (
    'A multilingual paragraph with **emphasis**, `inline code`, 日本語 and a ' +
    '[reference][ref]. The 中文 text, emoji 🦀 and ordinary words repeat so a ' +
    'single logical paragraph carries realistic wrapping work. '
  ).repeat(24);
const proseBlockTwo =
  (
    'Another paragraph with *italic* text and `inline code` covering 日本語 and ' +
    'mixed scripts. '
  ).repeat(36);
const articleUnit =
  '# Field notes 中文 🦀\n\n' +
  `${proseBlock}\n\n` +
  `${proseBlockTwo}\n\n` +
  '- First item with *text*\n' +
  '- Second item with 🦀\n\n' +
  '```rust\nfn main() { println!("日本語"); }\n```\n\n' +
  '[ref]: https://example.com/reference\n\n';

const styledParagraphUnit =
  '中文日本語🦀 **粗体** *斜体* `代码` [链接](https://example.com) ';

const fixtureSpecs = [
  {
    id: 'history-multiblock-1112000',
    title: 'Historical 1,112,000-byte multi-block article',
    file: 'history-multiblock-1112000.md',
    bytes: 1_112_000,
    unit: articleUnit,
    shape: 'multi-block Markdown with headings, lists, quotes and fences',
  },
  {
    id: 'history-single-paragraph-102420',
    title: 'Historical 102,420-byte styled CJK single paragraph',
    file: 'history-single-paragraph-102420.md',
    bytes: 102_420,
    unit: styledParagraphUnit,
    shape: 'one paragraph; CJK, emoji, emphasis and inline code',
  },
  {
    id: 'plain-long-line-1mib',
    title: '1 MiB unmarked single source line',
    file: 'plain-long-line-1mib.md',
    bytes: 1024 * 1024,
    unit: '中文 abc 🦀 ',
    shape: 'one line without Markdown syntax',
  },
  {
    id: 'complex-script',
    title: 'Complex script sample',
    file: 'complex-script.md',
    bytes: 64 * 1024,
    unit:
      'مرحبا بالعالم שלום עולם\n' +
      'e\u0301 a\u0308 ЖЗИ 中文\n' +
      '👩🏽‍💻 👨‍👩‍👧‍👦 🇨🇳🇯🇵\n' +
      '\ttabbed\tline\n' +
      'a very long unbroken word: ' + 'x'.repeat(120) + '\n' +
      'https://example.com/' + 'segment/'.repeat(20) + '\n',
    shape: 'RTL, combining marks, ZWJ emoji, flags, tabs, long URL',
  },
  {
    id: 'complex-markdown',
    title: 'Complex Markdown feature sample',
    file: 'complex-markdown.mdx',
    bytes: 96 * 1024,
    unit:
      '# Heading with [link](https://example.com)\n\n' +
      '1. ordered\n' +
      '   - nested **bold**\n' +
      '     - deeper `code`\n\n' +
      '| a | b |\n| --- | --- |\n| 中 | 文 |\n\n' +
      'Footnote reference[^1] and [ref link][r].\n\n' +
      '[^1]: Footnote body with *emphasis*.\n' +
      '[r]: https://example.com/ref "Title"\n\n' +
      '<Callout tone="note">\nMDX block content\n</Callout>\n\n' +
      '![image](./image.png)\n\n' +
      '```ts\nconst value: number = 1;\n```\n\n',
    shape: 'tables, footnotes, references, HTML/MDX, images, fences',
  },
  {
    id: 'boundary-8mib',
    title: '8 MiB document support boundary',
    file: 'boundary-8mib.md',
    bytes: 8 * 1024 * 1024,
    unit: articleUnit,
    shape: '8 MiB multi-block Markdown',
  },
];

function fixtureText(spec) {
  return exactBytes(spec.unit, spec.bytes);
}

function hash(text) {
  return createHash('sha256').update(text, 'utf8').digest('hex');
}

function writeFixtures(outDir) {
  fs.mkdirSync(outDir, { recursive: true });
  const fixtures = fixtureSpecs.map((spec) => {
    const text = fixtureText(spec);
    const file = path.join(outDir, spec.file);
    fs.writeFileSync(file, text, 'utf8');
    const bytes = byteLength(text);
    if (bytes !== spec.bytes) {
      throw new Error(`${spec.id}: generated ${bytes} bytes, expected ${spec.bytes}`);
    }
    return {
      id: spec.id,
      title: spec.title,
      file: spec.file,
      bytes,
      sha256: hash(text),
      shape: spec.shape,
      generator: { version: GENERATOR_VERSION, unit_sha256: hash(spec.unit) },
    };
  });
  const manifest = {
    schema: 'astrolith.editor-perf.fixtures/1',
    generated_at: new Date().toISOString(),
    generator_version: GENERATOR_VERSION,
    fixtures,
  };
  fs.writeFileSync(
    path.join(outDir, 'manifest.json'),
    `${JSON.stringify(manifest, null, 2)}\n`,
    'utf8',
  );
  return manifest;
}

function loadManifest(dir) {
  const file = path.join(dir, 'manifest.json');
  if (!fs.existsSync(file)) {
    throw new Error(`missing fixture manifest at ${file}; run: node scripts/editor-perf.mjs fixtures`);
  }
  return JSON.parse(fs.readFileSync(file, 'utf8'));
}

function isoNow() {
  return new Date().toISOString();
}

function mkdirp(...parts) {
  const dir = path.join(...parts);
  fs.mkdirSync(dir, { recursive: true });
  return dir;
}

function writeJson(file, value) {
  fs.writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`, 'utf8');
}

function projectStub(directory) {
  mkdirp(directory, 'src/content');
  mkdirp(directory, 'node_modules/astro');
  mkdirp(directory, 'node_modules/@astrojs/mdx');
  fs.writeFileSync(
    path.join(directory, 'package.json'),
    JSON.stringify(
      {
        name: 'editor-perf-fixture',
        private: true,
        dependencies: { astro: '5.18.2', '@astrojs/mdx': '4.3.14' },
      },
      null,
      2,
    ),
  );
  fs.writeFileSync(
    path.join(directory, 'astro.config.mjs'),
    "import { defineConfig } from 'astro/config';\nimport mdx from '@astrojs/mdx';\nexport default defineConfig({ integrations: [mdx()] });\n",
  );
  fs.writeFileSync(
    path.join(directory, 'node_modules/astro/package.json'),
    JSON.stringify({ name: 'astro', version: '5.18.2' }, null, 2),
  );
  fs.writeFileSync(
    path.join(directory, 'node_modules/@astrojs/mdx/package.json'),
    JSON.stringify(
      {
        name: '@astrojs/mdx',
        version: '4.3.14',
        peerDependencies: { astro: '^5.0.0' },
      },
      null,
      2,
    ),
  );
  fs.writeFileSync(
    path.join(directory, 'src/content.config.ts'),
    "import { defineCollection } from 'astro:content';\nimport { glob } from 'astro/loaders';\nconst blog = defineCollection({ loader: glob({ base: './src/content', pattern: '**/*.{md,mdx}' }) });\nexport const collections = { blog };\n",
  );
}

function registry(root) {
  return {
    version: 1,
    projects: [
      {
        id: 'editor-perf-fixture',
        path: root,
        display_name: 'editor-perf-fixture',
        package_name: 'editor-perf-fixture',
        last_opened_at: 1,
      },
    ],
  };
}

function scenarioFor(options) {
  const settle = Number(options.settle ?? 1500);
  const waitDocument = Number(options['wait-document'] ?? 15000);
  const pause = Number(options.pause ?? 700);
  const steps = [];
  const pushMode = (mode) => steps.push({ mode, wait_ms: pause });
  const name = options.scenario ?? 'cycle';
  if (name === 'idle') {
    steps.push({ wait_ms: Number(options.duration ?? 6000) });
  } else if (name === 'cycle') {
    const cycles = Number(options.cycles ?? 2);
    const order = [
      ['writing', 'build'],
      ['build', 'preview'],
      ['preview', 'writing'],
      ['writing', 'preview'],
      ['preview', 'build'],
      ['build', 'writing'],
    ];
    for (let cycle = 0; cycle < cycles; cycle += 1) {
      for (const [from, to] of order) {
        pushMode(from);
        pushMode(to);
      }
    }
  } else if (name === 'scroll-input') {
    pushMode('writing');
    for (let step = 0; step < Number(options.cycles ?? 4); step += 1) {
      steps.push({ scroll_y: -4000 * (step + 1), wait_ms: pause });
      steps.push({ scroll_y: 0, wait_ms: pause / 2 });
      steps.push({ input: '中🦀', wait_ms: pause });
    }
  } else {
    throw new Error(`unknown scenario ${name}`);
  }
  steps.push({ wait_ms: 300 });
  steps.push({ quit: true });
  return { settle_ms: settle, wait_document_ms: waitDocument, steps };
}

function targetDir(backend) {
  return path.join(ROOT, 'target/editor-perf', backend === 'fast' ? 'fast' : 'pre');
}

function buildBinary(backend, log) {
  const args = ['build', '-p', 'astrolith-desktop', '--locked'];
  if (backend === 'fast') args.push('--features', 'gpui-fast');
  const started = Date.now();
  const result = spawnSync('cargo', args, {
    cwd: ROOT,
    env: { ...process.env, CARGO_TARGET_DIR: targetDir(backend) },
    encoding: 'utf8',
  });
  const elapsed = (Date.now() - started) / 1000;
  fs.appendFileSync(log, `$ cargo ${args.join(' ')}\n${result.stdout}${result.stderr}\n`);
  if (result.status !== 0) {
    throw new Error(`cargo build failed for ${backend}; see ${log}`);
  }
  return {
    binary: path.join(targetDir(backend), 'debug', 'astrolith-desktop'),
    build_ms: elapsed * 1000,
  };
}

function hyprlandWindowTitles() {
  const result = spawnSync('hyprctl', ['clients', '-j'], { encoding: 'utf8' });
  if (result.status !== 0) return [];
  try {
    return JSON.parse(result.stdout).map((client) => ({
      title: client.title ?? '',
      address: client.address ?? '',
    }));
  } catch {
    return [];
  }
}

function hyprctl(...args) {
  const result = spawnSync('hyprctl', args, { encoding: 'utf8' });
  return { status: result.status, output: `${result.stdout ?? ''}${result.stderr ?? ''}` };
}

async function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function parsePerfLog(text) {
  const events = [];
  for (const line of text.split('\n')) {
    if (!line.includes('astrolith::perf')) continue;
    const fields = {};
    const pattern = /([a-z_]+)=("[^"]*"|[^ ]+)/g;
    let match;
    while ((match = pattern.exec(line))) {
      fields[match[1]] = match[2].replace(/^"|"$/g, '');
    }
    if (fields.event) events.push(fields);
  }
  return events;
}

function percentile(sorted, fraction) {
  if (!sorted.length) return null;
  const index = Math.min(sorted.length - 1, Math.ceil(sorted.length * fraction) - 1);
  return sorted[Math.max(index, 0)];
}

function summarize(events) {
  const frames = events.filter((event) => event.event === 'writer_frame');
  const repaints = events.filter((event) => event.event === 'writer_repaint');
  const repaintMs = repaints
    .map((event) => Number(event.paint_ms ?? 0))
    .sort((a, b) => a - b);
  const totals = frames
    .map((frame) =>
      ['render_ms', 'layout_ms', 'prepaint_ms', 'paint_ms']
        .map((key) => Number(frame[key] ?? 0))
        .reduce((sum, value) => sum + value, 0),
    )
    .sort((a, b) => a - b);
  const phase = (key) => {
    const values = frames.map((frame) => Number(frame[key] ?? 0)).sort((a, b) => a - b);
    return {
      p50: percentile(values, 0.5),
      p95: percentile(values, 0.95),
      max: values.at(-1) ?? null,
    };
  };
  const steps = events.filter((event) => event.event === 'driver_step');
  const stepMs = steps.map((event) => Number(event.ms ?? 0)).sort((a, b) => a - b);
  const modes = events
    .filter((event) => event.event === 'mode_handler')
    .map((event) => Number(event.ms ?? 0))
    .sort((a, b) => a - b);
  const changes = events
    .filter((event) => event.event === 'source_transaction')
    .map((event) => Number(event.ms ?? 0))
    .sort((a, b) => a - b);
  const parses = events
    .filter((event) => event.event === 'parse_queue')
    .map((event) => Number(event.ms ?? 0))
    .sort((a, b) => a - b);
  return {
    writer_repaints: {
      samples: repaints.length,
      paint_ms: {
        p50: percentile(repaintMs, 0.5),
        p95: percentile(repaintMs, 0.95),
        max: repaintMs.at(-1) ?? null,
      },
      caret_blinks_max: Math.max(0, ...repaints.map((event) => Number(event.caret_blinks ?? 0))),
    },
    writer_frames: {
      samples: frames.length,
      total_ms: {
        p50: percentile(totals, 0.5),
        p95: percentile(totals, 0.95),
        p99: percentile(totals, 0.99),
        max: totals.at(-1) ?? null,
      },
      render_ms: phase('render_ms'),
      layout_ms: phase('layout_ms'),
      prepaint_ms: phase('prepaint_ms'),
      paint_ms: phase('paint_ms'),
      blocks_rendered_max: Math.max(0, ...frames.map((frame) => Number(frame.blocks_rendered ?? 0))),
      blocks_total_max: Math.max(0, ...frames.map((frame) => Number(frame.blocks_total ?? 0))),
      nodes_built_total: frames.reduce((sum, frame) => sum + Number(frame.nodes_built ?? 0), 0),
      nodes_reused_total: frames.reduce((sum, frame) => sum + Number(frame.nodes_reused ?? 0), 0),
      geometry_leaves_max: Math.max(0, ...frames.map((frame) => Number(frame.geometry_leaves ?? 0))),
      shaping_bytes_max: Math.max(0, ...frames.map((frame) => Number(frame.shaping_bytes ?? 0))),
      caret_blinks_max: Math.max(0, ...frames.map((frame) => Number(frame.caret_blinks ?? 0))),
    },
    driver_steps: {
      samples: steps.length,
      p50: percentile(stepMs, 0.5),
      p95: percentile(stepMs, 0.95),
      max: stepMs.at(-1) ?? null,
    },
    mode_handlers: {
      samples: modes.length,
      p50: percentile(modes, 0.5),
      p95: percentile(modes, 0.95),
      max: modes.at(-1) ?? null,
    },
    change_handlers: {
      samples: changes.length,
      p50: percentile(changes, 0.5),
      p95: percentile(changes, 0.95),
      max: changes.at(-1) ?? null,
    },
    parse_queue_vs_install: {
      samples: parses.length,
      p50: percentile(parses, 0.5),
      p95: percentile(parses, 0.95),
      max: parses.at(-1) ?? null,
    },
  };
}

async function runOnce(options) {
  const backend = options.backend ?? 'pre';
  if (!['pre', 'fast'].includes(backend)) {
    throw new Error(`unknown backend ${backend}`);
  }
  const fixtureDir = path.resolve(options.fixtures ?? DEFAULT_FIXTURE_DIR);
  const manifest = loadManifest(fixtureDir);
  const fixture = manifest.fixtures.find((entry) => entry.id === (options.fixture ?? 'history-single-paragraph-102420'));
  if (!fixture) throw new Error(`unknown fixture ${options.fixture}`);

  const startedAt = isoNow();
  const stamp = startedAt.replace(/[:.]/g, '-');
  const testId = options['test-id'] ?? `editor-m0-${backend}-${fixture.id}-${stamp}`;
  const caseDir = mkdirp(ARTIFACT_ROOT, testId);
  // Isolated runtime state lives outside the committed artifact tree: the
  // contract forbids copied project trees, and the manifest names the command.
  const runDir = fs.mkdtempSync(path.join(os.tmpdir(), 'astrolith-editor-perf-'));
  const logFile = path.join(caseDir, 'build.log');
  fs.writeFileSync(logFile, '');
  const build = buildBinary(backend, logFile);

  const project = mkdirp(runDir, 'project');
  projectStub(project);
  const sourceName = `fixture.${path.extname(fixture.file).slice(1)}`;
  const entry = path.join(project, 'src/content', sourceName);
  fs.copyFileSync(path.join(fixtureDir, fixture.file), entry);

  const preferences = mkdirp(runDir, 'preferences');
  writeJson(path.join(preferences, 'projects.json'), registry(project));
  const settings = path.join(preferences, 'global.json');
  // Keep the compositor's XDG_RUNTIME_DIR: the Wayland socket must resolve.
  // Single-instance isolation comes from the isolated HOME, which seeds the
  // per-user socket name, so this run never joins the developer's window.
  for (const dir of ['home', 'config', 'state', 'cache', 'recovery', 'save-failures']) {
    mkdirp(runDir, dir);
  }

  const scenario = scenarioFor(options);
  const env = {
    ...process.env,
    HOME: path.join(runDir, 'home'),
    XDG_CONFIG_HOME: path.join(runDir, 'config'),
    XDG_STATE_HOME: path.join(runDir, 'state'),
    XDG_CACHE_HOME: path.join(runDir, 'cache'),
    ASTROLITH_SETTINGS_PATH: settings,
    ASTROLITH_RECOVERY_DIR: path.join(runDir, 'recovery'),
    ASTROLITH_SAVE_FAILURE_DIR: path.join(runDir, 'save-failures'),
    ASTROLITH_EDITOR_PERF: '1',
    ASTROLITH_PERF_SCENARIO: JSON.stringify(scenario),
    ASTROLITH_LOG: 'info',
    ASTROLITH_LOG_FILE: 'off',
    ASTROLITH_LOG_FORMAT: 'text',
  };
  const command = [
    build.binary,
    entry,
  ];
  fs.writeFileSync(path.join(caseDir, 'command.txt'), `${command.join(' ')}\n`);
  fs.writeFileSync(
    path.join(caseDir, 'scenario.json'),
    `${JSON.stringify(scenario, null, 2)}\n`,
  );

  const stdoutPath = path.join(caseDir, 'stdout.log');
  const stderrPath = path.join(caseDir, 'stderr.log');
  const stdout = fs.createWriteStream(stdoutPath);
  const stderr = fs.createWriteStream(stderrPath);
  const started = Date.now();
  const child = spawn(command[0], command.slice(1), {
    cwd: ROOT,
    env,
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  child.stdout.pipe(stdout);
  child.stderr.pipe(stderr);

  let fullscreenPairs = 0;
  let hyprland = 'not_run';
  if (options['fullscreen-pairs']) {
    const pairs = Number(options['fullscreen-pairs']);
    hyprland = 'attempted';
    for (let pair = 0; pair < pairs; pair += 1) {
      await sleep(Number(options['fullscreen-wait'] ?? 1200));
      const before = hyprctl('dispatch', 'hl.dsp.window.fullscreen({mode=0})');
      await sleep(Number(options['fullscreen-hold'] ?? 1200));
      const after = hyprctl('dispatch', 'hl.dsp.window.fullscreen({mode=0})');
      if (before.status === 0 && after.status === 0) fullscreenPairs += 1;
      await sleep(Number(options['fullscreen-gap'] ?? 600));
    }
  }

  const timeout = Number(options.timeout ?? 180) * 1000;
  const timedOut = await Promise.race([
    new Promise((resolve) => child.on('exit', () => resolve(false))),
    sleep(timeout).then(() => true),
  ]);
  if (timedOut) child.kill('SIGTERM');
  await Promise.race([
    new Promise((resolve) => child.on('exit', resolve)),
    sleep(5000),
  ]);
  await new Promise((resolve) => stdout.end(resolve));
  await new Promise((resolve) => stderr.end(resolve));
  const duration = (Date.now() - started) / 1000;
  const stderrText = fs.readFileSync(stderrPath, 'utf8');
  const events = parsePerfLog(stderrText);
  const metrics = summarize(events);
  const documentReady = events.some(
    (event) => event.event === 'driver_document' && event.ready === 'true',
  );
  const backendLine = stderrText
    .split('\n')
    .find((line) => line.includes('GPUI backend selected'));
  const status = timedOut || !documentReady ? 'failed' : 'passed';
  const observed = [
    timedOut ? 'driver timed out' : null,
    documentReady ? 'document opened through a startup entry' : 'document did not open',
    `writer frames sampled: ${metrics.writer_frames.samples}`,
  ]
    .filter(Boolean)
    .join('; ');

  const title = `Native ${backend} baseline: ${fixture.id}`;
  const manifestEntry = {
    schema: 'astrolith.e2e.manifest/1',
    test_id: testId,
    title,
    purpose:
      'M0 baseline for the editor performance plan: repeatable fixture, isolated launch, counters and phase timings.',
    status,
    layer: 'L2 real process',
    started_at: startedAt,
    duration_s: duration,
    source: backendLine?.trim() ?? 'backend banner not captured',
    environment: {
      os: `${os.type()} ${os.release()}`,
      arch: os.arch(),
      backend,
      features: backend === 'fast' ? ['gpui-fast'] : [],
      session: process.env.XDG_SESSION_TYPE ?? null,
      wayland_display: process.env.WAYLAND_DISPLAY ?? null,
      scale: null,
      profiled: false,
    },
    fixture: {
      id: fixture.id,
      bytes: fixture.bytes,
      sha256: fixture.sha256,
      generated: path.relative(ROOT, path.join(fixtureDir, fixture.file)),
      shape: fixture.shape,
    },
    command: command.join(' '),
    repro: `node scripts/editor-perf.mjs run --fixture ${fixture.id} --backend ${backend} --scenario ${options.scenario ?? 'cycle'}`,
    expected:
      'The document opens at a startup entry, the scripted steps complete, and numeric writer frames, mode, change and parse spans are recorded.',
    observed,
    evidence: {
      stdout: path.relative(ROOT, stdoutPath),
      stderr: path.relative(ROOT, stderrPath),
      build_log: path.relative(ROOT, logFile),
      scenario: path.relative(ROOT, path.join(caseDir, 'scenario.json')),
      traces: 'not captured in this run',
    },
    metrics: {
      build_ms: build.build_ms,
      ...metrics,
      fullscreen_pairs: fullscreenPairs,
    },
    limitations: [
      'Scripted actions call the real writer input adapter; physical keyboard and IME candidate windows are not exercised.',
      'Frame timings cover the writer request-layout through paint CPU phases; compositor presentation is excluded.',
      hyprland === 'not_run'
        ? 'No Hyprland fullscreen/resize actions were requested for this run.'
        : 'Hyprland fullscreen dispatch was attempted; check the pair count.',
      'Preview and Build use Kit TextView/Editor internals and are measured externally in this baseline.',
    ],
    rerun_notes: 'Run the same command with an isolated preferences directory; do not compare across different fixtures or fonts.',
  };
  writeJson(path.join(caseDir, 'manifest.json'), manifestEntry);
  writeJson(path.join(caseDir, 'perf.json'), {
    schema: 'astrolith.editor-perf.metrics/1',
    test_id: testId,
    backend,
    fixture: fixture.id,
    metrics: manifestEntry.metrics,
  });
  fs.rmSync(runDir, { recursive: true, force: true });
  return manifestEntry;
}

async function main() {
  const [command, ...rest] = process.argv.slice(2);
  const options = parseArgs(rest);
  if (command === 'fixtures') {
    const out = path.resolve(options.out ?? DEFAULT_FIXTURE_DIR);
    const manifest = writeFixtures(out);
    console.log(`wrote ${manifest.fixtures.length} fixtures to ${path.relative(ROOT, out)}`);
    for (const fixture of manifest.fixtures) {
      console.log(`  ${fixture.id}: ${fixture.bytes} bytes sha256=${fixture.sha256}`);
    }
    return;
  }
  if (command === 'run') {
    const result = await runOnce(options);
    console.log(`${result.test_id}: ${result.status}`);
    console.log(JSON.stringify(result.metrics, null, 2));
    if (result.status !== 'passed') process.exitCode = 1;
    return;
  }
  if (command === 'ab') {
    for (const backend of ['pre', 'fast']) {
      const result = await runOnce({ ...options, backend });
      console.log(`${backend}: ${result.status} frames=${result.metrics.writer_frames.samples}`);
    }
    return;
  }
  console.error('usage: node scripts/editor-perf.mjs <fixtures|run|ab> [options]');
  process.exitCode = 2;
}

main().catch((error) => {
  console.error(error.stack ?? error.message ?? error);
  process.exitCode = 1;
});
