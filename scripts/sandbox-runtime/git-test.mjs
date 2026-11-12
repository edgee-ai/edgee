// Real backend regression for scoped linked-worktree Git writes.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const base = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), 'edgee-git-test-')));
const repo = path.join(base, 'main');
const workspace = path.join(base, 'linked worktree');
const session = path.join(base, 'session');
const gitEnvironment = {
  PATH: '/usr/bin:/bin', HOME: session,
  GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null',
  GIT_AUTHOR_NAME: 'Sandbox Test', GIT_AUTHOR_EMAIL: 'test@example.invalid',
  GIT_COMMITTER_NAME: 'Sandbox Test', GIT_COMMITTER_EMAIL: 'test@example.invalid',
};
function git(cwd, ...args) {
  const result = spawnSync('/usr/bin/git', args, { cwd, env: gitEnvironment, encoding: 'utf8' });
  assert.equal(result.status, 0, result.stderr);
  return result.stdout.trim();
}
let sandboxGit = '/usr/bin/git';
const literalPaths = [
  '/usr/bin', '/bin', '/usr/lib', '/lib', '/lib64', '/usr/share', '/usr/libexec',
  '/System/Library', '/Library/Apple', '/dev/null', '/dev/zero', '/dev/random',
  '/dev/urandom', '/dev/tty', '/etc/ssl', '/private/var/select', '/etc/pki',
  '/etc/ld.so.cache', '/etc/localtime',
];
let supervisor;
try {
  fs.mkdirSync(repo); fs.mkdirSync(session);
  git(repo, 'init', '-b', 'main');
  fs.writeFileSync(path.join(repo, 'file'), 'before\n');
  git(repo, 'add', 'file'); git(repo, 'commit', '-m', 'initial');
  git(repo, 'worktree', 'add', '-b', 'sandbox-test', workspace);
  const common = fs.realpathSync(path.join(repo, '.git'));
  const gitdir = fs.realpathSync(git(workspace, 'rev-parse', '--absolute-git-dir'));
  const pointer = fs.realpathSync(fs.readFileSync(path.join(gitdir, 'gitdir'), 'utf8').trim());
  assert.equal(pointer, fs.realpathSync(path.join(workspace, '.git')));
  assert.equal(path.dirname(gitdir), path.join(common, 'worktrees'));
  fs.mkdirSync(path.join(gitdir, 'hooks'));
  fs.writeFileSync(path.join(gitdir, 'hooks/pre-commit'), '#!/bin/sh\nexit 0\n');
  fs.writeFileSync(path.join(gitdir, 'config'), '# protected fixture\n');
  const writePaths = [gitdir, ...['objects', 'refs', 'logs'].map(p => fs.realpathSync(path.join(common, p)))];
  const deny = [
    path.join(workspace, '.git'),
    ...['config', 'config.worktree', 'hooks', 'info', 'commondir', 'gitdir', 'worktrees'].map(p => path.join(gitdir, p)),
    path.join(common, 'objects/info'),
  ];
  // On macOS /usr/bin/git dispatches to the selected Xcode/CLT installation.
  // Grant only its usr tooling root, as an explicit policy read_paths entry.
  if (process.platform === 'darwin') {
    const execPath = git(repo, '--exec-path');
    literalPaths.push(path.resolve(execPath, '../..'));
    sandboxGit = path.resolve(execPath, '../../bin/git');
  }
  const node = fs.realpathSync(process.execPath);
  const childFile = path.join(workspace, 'verify.cjs');
  fs.writeFileSync(childFile, `
const assert = require('node:assert/strict');
const fs = require('node:fs');
const {spawnSync} = require('node:child_process');
const denied = ${JSON.stringify([
    path.join(common, 'config'), path.join(common, 'hooks/pre-commit'),
    path.join(gitdir, 'config'), path.join(gitdir, 'config.worktree'),
    path.join(gitdir, 'hooks/pre-commit'), path.join(gitdir, 'commondir'),
    path.join(gitdir, 'gitdir'), path.join(common, 'objects/info/alternates'),
    path.join(common, 'worktrees/other/config'), path.join(repo, 'file'),
  ])};
for (const file of denied) assert.throws(() => fs.writeFileSync(file, 'forbidden'), file);
assert.throws(() => fs.mkdirSync(${JSON.stringify(path.join(common, 'worktrees/other'))}));
assert.throws(() => fs.unlinkSync(${JSON.stringify(path.join(gitdir, 'config'))}));
assert.throws(() => fs.renameSync(${JSON.stringify(path.join(gitdir, 'hooks'))}, ${JSON.stringify(path.join(gitdir, 'replaced-hooks'))}));
fs.writeFileSync('file', 'after\\n');
for (const args of [['status','--porcelain'], ['add','file'], ['commit','-m','sandbox commit']]) {
  const p = spawnSync(${JSON.stringify(sandboxGit)}, args, {encoding:'utf8'});
  assert.equal(p.status, 0, p.stderr);
}
console.log('PASS linked-worktree status/add/commit; shared and current configuration/hooks protected');
`);
  const input = path.join(base, 'input.json');
  const spec = {
    workspace, cwd: workspace, session_dir: session,
    // A read bind of common would shadow its nested writable binds on Linux.
    // Expose only Git's shared files plus the exact writable data directories.
    read_paths: [...literalPaths.filter(p => fs.existsSync(p)).map(p => fs.realpathSync(p)), node, ...writePaths,
      ...['config', 'HEAD', 'packed-refs', 'shallow', 'description', 'info/exclude', 'info/attributes']
        .map(p => path.join(common, p)).filter(p => fs.existsSync(p))],
    write_paths: writePaths, deny_write_paths: deny, allowed_domains: [],
    runtime_root: process.env.EDGEE_TEST_RUNTIME_ROOT || here,
    node_binary: node, argv: [node, childFile],
    env: { ...gitEnvironment, TMPDIR: session },
  };
  fs.writeFileSync(input, JSON.stringify(spec), {mode: 0o600});
  const driver = fs.readFileSync(path.resolve(here, '../../src/commands/launch/sandbox/runtime-driver.mjs'), 'utf8');
  supervisor = spawn(node, ['--input-type=module', '--eval', driver, input], {
    cwd: workspace, env: {PATH: '/opt/homebrew/bin:/usr/bin:/bin', HOME: session},
    detached: true, stdio: 'inherit',
  });
  const timer = setTimeout(() => { try { process.kill(-supervisor.pid, 'SIGKILL'); } catch {} }, 15000);
  const result = await new Promise((resolve, reject) => {
    supervisor.on('error', reject);
    supervisor.on('exit', (code, signal) => resolve({code, signal}));
  });
  clearTimeout(timer);
  assert.equal(result.code, 0, JSON.stringify(result));
  assert.equal(git(workspace, 'log', '-1', '--format=%s'), 'sandbox commit');
  assert.equal(fs.readFileSync(path.join(repo, 'file'), 'utf8'), 'before\n');
  assert.equal(fs.readFileSync(path.join(workspace, 'file'), 'utf8'), 'after\n');
  assert.equal(fs.readFileSync(path.join(gitdir, 'gitdir'), 'utf8').trim(), pointer);
  console.log(`PASS real Git worktree isolation (${process.platform})`);
} finally {
  if (supervisor?.pid) { try { process.kill(-supervisor.pid, 'SIGKILL'); } catch {} }
  fs.rmSync(base, {recursive: true, force: true});
}
