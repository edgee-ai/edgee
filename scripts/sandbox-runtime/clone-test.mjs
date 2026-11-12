// Real network clone through the production OS sandbox, without host Git auth.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const base = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), 'edgee-clone-test-')));
const workspace = path.join(base, 'fresh workspace with spaces');
const session = path.join(base, 'session');
const home = path.join(session, 'home');
const tmp = path.join(session, 'tmp');
const template = path.join(session, 'git-template');
let supervisor;
let timer;
try {
  for (const directory of [workspace, session, home, tmp, template]) fs.mkdirSync(directory);
  let git = '/usr/bin/git';
  const readPaths = [
    '/usr/bin', '/bin', '/usr/lib', '/lib', '/lib64', '/usr/share', '/usr/libexec',
    '/System/Library', '/Library/Apple', '/dev/null', '/dev/zero', '/dev/random',
    '/dev/urandom', '/dev/tty', '/etc/ssl', '/private/var/select', '/etc/pki',
    '/etc/ld.so.cache', '/etc/localtime',
  ];
  if (process.platform === 'darwin') {
    const result = spawnSync('/usr/bin/xcrun', ['--find', 'git'], {
      env: { PATH: '/usr/bin:/bin' }, encoding: 'utf8',
    });
    assert.equal(result.status, 0, result.stderr);
    git = fs.realpathSync(result.stdout.trim());
    readPaths.push(path.dirname(path.dirname(git)));
  }
  const env = {
    HOME: home, TMPDIR: tmp, TMP: tmp, TEMP: tmp,
    XDG_CONFIG_HOME: path.join(home, '.config'),
    XDG_CACHE_HOME: path.join(home, '.cache'),
    XDG_DATA_HOME: path.join(home, '.local/share'),
    PATH: `${path.dirname(git)}:/usr/bin:/bin:/usr/sbin:/sbin:/usr/local/bin:/opt/homebrew/bin`,
    SHELL: '/bin/sh', GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null',
    GIT_TERMINAL_PROMPT: '0',
  };
  const input = path.join(base, 'input.json');
  const spec = {
    // Keep upstream CWD security guards on the private setup directory while
    // Git creates metadata in its fresh absolute destination. The agent will
    // subsequently run with workspace CWD and the normal metadata protections.
    workspace, cwd: session, session_dir: session,
    read_paths: readPaths.filter(p => fs.existsSync(p)).map(p => fs.realpathSync(p)),
    write_paths: [], deny_write_paths: [], allowed_domains: ['github.com'],
    runtime_root: process.env.EDGEE_TEST_RUNTIME_ROOT || here,
    node_binary: fs.realpathSync(process.execPath),
    argv: [git, '-c', 'credential.helper=', '-c', 'http.followRedirects=false',
      '-c', 'core.hooksPath=/dev/null', 'clone', '--depth=1', '--template', template,
      '--', 'https://github.com/edgee-ai/edgee.git', workspace],
    env,
  };
  fs.writeFileSync(input, JSON.stringify(spec), { mode: 0o600 });
  const driver = fs.readFileSync(path.resolve(here, '../../src/commands/launch/sandbox/runtime-driver.mjs'), 'utf8');
  supervisor = spawn(spec.node_binary, ['--input-type=module', '--eval', driver, input], {
    cwd: spec.cwd, env: { PATH: '/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin', HOME: session },
    detached: true, stdio: 'inherit',
  });
  timer = setTimeout(() => { try { process.kill(-supervisor.pid, 'SIGKILL'); } catch {} }, 90000);
  const result = await new Promise((resolve, reject) => {
    supervisor.on('error', reject);
    supervisor.on('exit', (code, signal) => resolve({ code, signal }));
  });
  assert.equal(result.code, 0, JSON.stringify(result));
  assert.ok(fs.statSync(path.join(workspace, '.git')).isDirectory());
  assert.ok(fs.statSync(path.join(workspace, 'README.md')).isFile());
  assert.ok(!fs.existsSync(path.join(workspace, '.git/hooks/post-checkout')));
  console.log(`PASS HTTPS public repository clone via real sandbox (${process.platform})`);
} finally {
  clearTimeout(timer);
  if (supervisor?.pid) { try { process.kill(-supervisor.pid, 'SIGKILL'); } catch {} }
  fs.rmSync(base, { recursive: true, force: true });
}
