import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import test from 'node:test';
import { assessRun, watchChild } from './editor-perf.mjs';

const events = [
  { event: 'driver_document', ready: 'true' },
  { event: 'driver_step', name: 'writing' },
  { event: 'driver_step', name: 'quit' },
  { event: 'driver_quit' },
];
const metrics = { writer_frames: { total_ms: { p95: 1, p99: 2 } } };

test('execution requires a clean exit and the complete scripted driver', () => {
  const run = { events, expectedSteps: 2, exit: { code: 0, signal: null } };
  assert.equal(assessRun(run, metrics).execution.status, 'passed');
  for (const broken of [
    { exit: { code: 7, signal: null } },
    { exit: { code: null, signal: 'SIGSEGV' } },
    { events: events.slice(0, -1) },
    { expectedSteps: 3 },
    { timedOut: true },
    { fullscreenRequested: 2, fullscreenPairs: 1 },
  ]) {
    assert.equal(assessRun({ ...run, ...broken }, metrics).execution.status, 'failed');
  }
});

test('a completed baseline records failed latency independently', () => {
  const result = assessRun(
    { events, expectedSteps: 2, exit: { code: 0, signal: null } },
    { writer_frames: { total_ms: { p95: 140, p99: 140 } } },
  );
  assert.equal(result.execution.status, 'passed');
  assert.equal(result.performance.status, 'failed');
  assert.equal(assessRun({ events, expectedSteps: 2, exit: { code: 0 } }, {}).performance.status, 'not_run');
});

test('a locked desktop cannot pass native performance acceptance', () => {
  const result = assessRun({ events, expectedSteps: 2, exit: { code: 0 }, screenLocked: true }, metrics);
  assert.equal(result.execution.status, 'passed');
  assert.equal(result.performance.status, 'not_run');
});

test('process exit is retained even when observed after the child closes', async () => {
  const child = spawn(process.execPath, ['-e', 'process.exit(7)']);
  const finished = watchChild(child, 1000);
  await once(child, 'close');
  assert.deepEqual(await finished, { code: 7, signal: null, timedOut: false, error: null });
});

test('a stuck child is killed and reported as timed out', async () => {
  const child = spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)']);
  const result = await watchChild(child, 100);
  assert.equal(result.timedOut, true);
  assert.equal(result.signal, 'SIGTERM');
});
