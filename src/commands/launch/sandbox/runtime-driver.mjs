// The supervisor runs OUTSIDE the sandbox. Never inherit the caller's environment.
// Its JSON input is a mode-0600 file outside the allowed filesystem and is removed
// before starting any untrusted process. No credentials are printed on errors.
import fs from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';
import { spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';

const specPath = process.argv[1];
let manager;
let child;
let interrupted = false;
class SetupError extends Error {}
const parentPid = process.ppid;
const spec = JSON.parse(fs.readFileSync(specPath, 'utf8'));
fs.unlinkSync(specPath);
const quote = s => "'" + String(s).replaceAll("'", "'\\''") + "'";
const literal = p => ({ path: p, literal: true });

function narrowMacProfile(wrapper) {
  // Upstream's default-deny profile grants keychain and host preference IPC.
  // Remove these grants, rather than layering sandbox-exec (Seatbelt forbids
  // nested sandbox_apply). Match exactly once against the PINNED runtime;
  // an upstream profile change is a hard error, never a weaker fallback.
  const replacements = [
    // Codex probes an optional system policy here. Without ancestor metadata,
    // Seatbelt reports EPERM even when /etc/codex does not exist. Grant stat,
    // never directory enumeration or file contents, to distinguish ENOENT.
    ['(version 1)', '(version 1)\n(allow file-read-metadata (literal "/etc") (literal "/private/etc"))'],
    ['  (global-name "com.apple.securityd.xpc")', ''],
    ['(allow mach-lookup (global-name "com.apple.SecurityServer"))', '(deny mach-lookup (global-name "com.apple.SecurityServer"))'],
    ['(allow user-preference-read)', '(deny user-preference-read)'],
    ['(allow ipc-posix-shm)', '(deny ipc-posix-shm)'],
    ['(allow ipc-posix-sem)', '(deny ipc-posix-sem)'],
  ];
  for (const [from, to] of replacements) {
    if (wrapper.split(from).length !== 2) throw new SetupError('Runtime macOS profile differs from the reviewed profile; reinstall the pinned dependency.');
    wrapper = wrapper.replace(from, to);
  }
  return wrapper;
}

async function wrapped(command) {
  let result = await manager.wrapWithSandboxArgv(command, '/bin/sh', undefined, undefined, spec.cwd, { commandId: randomUUID(), commandText: 'edgee sandbox agent' });
  if (process.platform === 'darwin') {
    if (result.argv.length !== 3 || result.argv[1] !== '-c') throw new SetupError('Unexpected sandbox wrapper.');
    result.argv[2] = narrowMacProfile(result.argv[2]);
  }
  return result;
}

async function run(command, stdio = 'inherit') {
  // The runtime advertises localhost proxy URLs. Some native resolvers consult
  // host files/services even for localhost, which the read allowlist forbids.
  // Change only those trusted proxy environment values, inside the sandbox,
  // preserving credentials and ports; do not rewrite the user's argv or grant
  // access to host DNS files. POSIX parameter expansion needs no helper binary.
  const numericProxies = ['HTTP_PROXY', 'HTTPS_PROXY', 'http_proxy', 'https_proxy', 'ALL_PROXY', 'all_proxy']
    .map(name => `case "$${name}" in http://*@localhost:*) export ${name}="\${${name}%localhost:*}127.0.0.1:\${${name}##*:}" ;; *) exit 125 ;; esac; `).join('');
  const invocation = await wrapped(numericProxies + command);
  return await new Promise((resolve, reject) => {
    child = spawn(invocation.argv[0], invocation.argv.slice(1), {
      cwd: spec.cwd,
      env: { ...invocation.env, ...spec.env },
      stdio,
    });
    child.once('error', reject);
    child.once('exit', (code, signal) => resolve({ code, signal }));
  });
}

// Foreground terminal signals reach the whole supervisor process group. Keep the
// proxy alive while the actual agent handles Ctrl-C; SIGTERM/HUP terminate it.
process.on('SIGINT', () => { if (!child) interrupted = true; });
for (const signal of ['SIGTERM', 'SIGHUP']) process.on(signal, () => {
  child?.kill(signal);
  setTimeout(() => process.kill(-process.pid, 'SIGKILL'), 1000).unref();
});
const orphanCheck = setInterval(() => {
  if (process.ppid !== parentPid) process.kill(-process.pid, 'SIGKILL');
}, 1000);
orphanCheck.unref();

try {
  if (!['darwin', 'linux'].includes(process.platform)) throw new SetupError('Local sandbox supports macOS and Linux only.');
  const [major, minor] = process.versions.node.split('.').map(Number);
  if (major < 22 || (major === 22 && minor < 12)) throw new SetupError('Sandbox requires Node.js >=22.12.');
  const packageRoot = path.join(spec.runtime_root, 'node_modules/@anthropic-ai/sandbox-runtime');
  const pkg = JSON.parse(fs.readFileSync(path.join(packageRoot, 'package.json'), 'utf8'));
  if (pkg.version !== '0.0.82') throw new SetupError('Sandbox requires @anthropic-ai/sandbox-runtime exactly 0.0.82.');
  ({ SandboxManager: manager } = await import(pathToFileURL(path.join(packageRoot, 'dist/index.js'))));
  const dependencies = manager.checkDependencies();
  if (dependencies.errors.length || dependencies.warnings.length) {
    throw new SetupError('Sandbox dependencies unavailable or degraded: ' + [...dependencies.errors, ...dependencies.warnings].join('; '));
  }
  if (spec.preflight) process.exit(0);
  if (interrupted) throw new SetupError('Launch interrupted before the agent started.');
  const config = {
    network: {
      allowedDomains: spec.allowed_domains,
      deniedDomains: ['localhost', '*.localhost'],
      strictAllowlist: true,
      allowLocalBinding: false,
      allowAllUnixSockets: false,
      allowUnixSockets: [],
      deniedResolvedAddresses: ['10.0.0.0/8', '172.16.0.0/12', '192.168.0.0/16', '100.64.0.0/10', 'fc00::/7'],
    },
    filesystem: {
      denyRead: [literal('/')],
      allowRead: [...spec.read_paths, spec.workspace, spec.session_dir].map(literal),
      allowWrite: [...spec.write_paths, spec.workspace, spec.session_dir].map(literal),
      denyWrite: ['/tmp/claude', '/private/tmp/claude', ...spec.deny_write_paths].map(literal),
      allowGitConfig: false,
    },
    enableWeakerNestedSandbox: false,
    allowNestedUserNamespaces: false,
    enableWeakerNetworkIsolation: false,
    allowPty: true,
  };
  await manager.initialize(config, undefined, false);
  // Prove the backend actually launches, and that a real outside secret is not
  // readable, before printing an active-isolation status or launching the agent.
  const probe = fs.mkdtempSync(path.join(path.dirname(specPath), 'edgee-probe-'));
  fs.writeFileSync(path.join(probe, 'secret'), 'edgee-fake-secret', { mode: 0o600 });
  try {
    const command = `/bin/sh -c ${quote('test ! -r "$1" && ! (echo denied > "$2") 2>/dev/null')} edgee-probe ${quote(path.join(probe, 'secret'))} ${quote(path.join(probe, 'write'))}`;
    const result = await run(command, ['ignore', 'ignore', 'ignore']);
    if (result.code !== 0 || fs.existsSync(path.join(probe, 'write'))) throw new SetupError('OS sandbox isolation probe failed; agent was not launched.');
  } finally { fs.rmSync(probe, { recursive: true, force: true }); }
  if (interrupted) throw new SetupError('Launch interrupted before the agent started.');
  process.stderr.write(`Edgee sandbox active (${process.platform === 'darwin' ? 'Seatbelt' : 'bubblewrap + seccomp'}); workspace: ${spec.workspace}\n`);
  process.stderr.write(`Network allowlist: ${spec.allowed_domains.join(', ') || '(none)'}; host localhost/Unix sockets blocked; isolated home.\n`);
  // env(1) is inside the OS sandbox. Values are quoted, never evaluated. This
  // restores the private TMPDIR after upstream sets its shared /tmp/claude.
  const temp = spec.env.TMPDIR || spec.session_dir;
  const command = `exec /usr/bin/env ${quote(`TMPDIR=${temp}`)} ${spec.argv.map(quote).join(' ')}`;
  const result = await run(command);
  await manager.reset();
  manager = undefined;
  if (result.signal) {
    process.removeAllListeners(result.signal);
    process.kill(process.pid, result.signal);
  } else process.exitCode = result.code ?? 1;
} catch (error) {
  // Only known setup errors are displayed; child argv/env may contain secrets.
  process.stderr.write(`Edgee sandbox refused launch: ${error instanceof SetupError ? error.message : 'runtime setup or execution failed. Verify the pinned runtime, OS permissions and configured paths; no unconfined fallback was attempted.'}\n`);
  process.exitCode = 125;
} finally {
  clearInterval(orphanCheck);
  if (manager) await manager.reset().catch(() => {});
}
