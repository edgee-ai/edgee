import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import http from 'node:http';
import net from 'node:net';
import {spawn, spawnSync} from 'node:child_process';
import {fileURLToPath} from 'node:url';
// This suite executes the real OS backend. Setup failures fail the suite.
const here=path.dirname(fileURLToPath(import.meta.url));
const runtimeRoot=process.env.EDGEE_TEST_RUNTIME_ROOT || here;
const base=fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(),'edgee-test-')));
const workspace=path.join(base,'workspace with spaces');fs.mkdirSync(workspace);
fs.mkdirSync(path.join(workspace,'.git'));fs.writeFileSync(path.join(workspace,'.git/config'),'fake-config');
const session=path.join(base,'session');fs.mkdirSync(session);
const secret=path.join(base,'secret');fs.writeFileSync(secret,'fake-secret');
const server=http.createServer((req,res)=>res.end('controlled-relay'));
await new Promise(r=>server.listen(0,'127.0.0.1',r));
const port=server.address().port;
// Socket and keychain are readable in the workspace: these assertions test
// IPC/network confinement rather than merely an outside-path filesystem denial.
const socket=path.join(workspace,'s');
const unix=net.createServer(s=>s.end('fake-docker'));
await new Promise(r=>unix.listen(socket,r));
const node=fs.realpathSync(process.execPath);
const keychain=path.join(workspace,'fake.keychain-db');
if(process.platform==='darwin'){
  for(const args of [['create-keychain','-p','fake-test-password',keychain],['add-generic-password','-a','edgee-sandbox-test','-s','edgee-fake-secret','-w','fake-value',keychain]]) {
    const p=spawnSync('/usr/bin/security',args,{encoding:'utf8'});
    if(p.status!==0) throw Error('Cannot create isolated fake keychain: '+p.stderr);
  }
  const positive=spawnSync('/usr/bin/security',['find-generic-password','-a','edgee-sandbox-test','-s','edgee-fake-secret','-w',keychain],{encoding:'utf8'});
  if(positive.status!==0 || positive.stdout.trim()!=='fake-value') throw Error('Fake keychain positive control failed');
}
const test=`
const fs=require('node:fs');const net=require('node:net');const {spawnSync}=require('node:child_process');
const assert=require('node:assert/strict');
assert.equal(process.env.FAKE_HOST_SECRET,undefined);
assert.throws(()=>fs.readFileSync('/etc/passwd'));
if(process.platform==='darwin') assert.throws(()=>fs.statSync('/etc/edgee-sandbox-absent-'+${JSON.stringify(path.basename(base))}+'/requirements.toml'), error=>error.code==='ENOENT');
assert.deepEqual(process.argv.slice(2),['spaces and \"quotes\"', '$(touch should-not-exist)', '--flag=value']);
if(process.platform==='darwin'){const key=spawnSync('/usr/bin/security',['find-generic-password','-a','edgee-sandbox-test','-s','edgee-fake-secret','-w',${JSON.stringify(keychain)}]);assert.notEqual(key.status,0,'keychain broker access');}
assert.throws(()=>fs.writeFileSync('.git/config','override'));
assert.throws(()=>fs.renameSync('.git/config','.git/replaced-config'));
assert.throws(()=>fs.unlinkSync('.git/config'));
fs.writeFileSync('allowed','okay');assert.equal(fs.readFileSync('allowed','utf8'),'okay');
assert.throws(()=>fs.readFileSync(${JSON.stringify(secret)}));
assert.throws(()=>fs.writeFileSync(${JSON.stringify(path.join(base,'outside'))},'bad'));
fs.symlinkSync(${JSON.stringify(secret)},'symlink');assert.throws(()=>fs.readFileSync('symlink'));
assert.throws(()=>fs.writeFileSync('symlink','overwritten'));
fs.symlinkSync(${JSON.stringify(base)},'outside-directory');assert.throws(()=>fs.writeFileSync('outside-directory/escaped','bad'));
const allowed=spawnSync('/bin/sh',['-c','printf mcp-output > mcp-output']);assert.equal(allowed.status,0);assert.equal(fs.readFileSync('mcp-output','utf8'),'mcp-output');
assert.throws(()=>fs.readFileSync('../secret'));
const denied=spawnSync('/bin/sh',['-c','cat "$1"','mcp-local',${JSON.stringify(secret)}]);assert.notEqual(denied.status,0);
const connect=(opts)=>new Promise((resolve,reject)=>{const s=net.createConnection(opts);s.on('connect',()=>{s.destroy();reject(Error('FORBIDDEN CONNECTION'))});s.on('error',()=>resolve());s.setTimeout(2000,()=>{s.destroy();resolve()})});
(async()=>{await connect({host:'127.0.0.1',port:${port}});await connect({path:${JSON.stringify(socket)}});
const curl=spawnSync('/usr/bin/curl',['--silent','--show-error','--max-time','5','--noproxy','','http://127.0.0.1:${port}'],{encoding:'utf8'});assert.equal(curl.status,0,curl.stderr);assert.equal(curl.stdout,'controlled-relay');
const proxy=new URL(process.env.HTTP_PROXY);const unauth=spawnSync('/usr/bin/curl',['--silent','--show-error','--fail','--max-time','5','--noproxy','','--proxy',proxy.protocol+'//'+proxy.host,'http://127.0.0.1:${port}'],{encoding:'utf8'});assert.notEqual(unauth.status,0,'proxy must require session authentication');
const forbidden=spawnSync('/usr/bin/curl',['--silent','--max-time','5','--noproxy','','https://example.com'],{encoding:'utf8'});assert.notEqual(forbidden.status,0);
console.log('PASS filesystem, fake-secret, outside-write, symlink, relative-path, local-MCP, environment, raw-loopback, Unix-socket, permitted-proxy, denied-proxy, proxy-auth, keychain, complex-argv');})().catch(e=>{console.error(e);process.exit(1)});
`;
const testPath=path.join(workspace,'test.cjs');fs.writeFileSync(testPath,test);
const input=path.join(base,'input.json');
const spec={workspace,cwd:workspace,session_dir:session,read_paths:['/usr/bin','/bin','/usr/lib','/lib','/lib64','/usr/share','/usr/libexec','/System/Library','/Library/Apple','/dev/null','/dev/zero','/dev/random','/dev/urandom','/dev/tty','/etc/ssl','/private/var/select','/etc/pki','/etc/ld.so.cache','/etc/localtime',node].filter(p=>fs.existsSync(p)).map(p=>fs.realpathSync(p)),write_paths:[],deny_write_paths:[path.join(workspace,'.git/config')],allowed_domains:[`127.0.0.1:${port}`],runtime_root:runtimeRoot,node_binary:node,argv:[node,testPath,'spaces and \"quotes\"', '$(touch should-not-exist)', '--flag=value'],env:{PATH:'/usr/bin:/bin',HOME:session,TMPDIR:session}};
fs.writeFileSync(input,JSON.stringify(spec),{mode:0o600});
const driver=fs.readFileSync(path.resolve(here,'../../src/commands/launch/sandbox/runtime-driver.mjs'),'utf8');
async function launch(next, signal=false) {
  const file=path.join(base,'input-'+Math.random()+'.json');
  fs.writeFileSync(file,JSON.stringify(next),{mode:0o600});
  const p=spawn(node,['--input-type=module','--eval',driver,file],{cwd:workspace,env:{PATH:'/opt/homebrew/bin:/usr/bin:/bin',HOME:session},detached:true,stdio:'inherit'});
  let timer;
  if(signal) timer=setInterval(()=>{
    if(fs.existsSync(path.join(workspace,'ready'))){clearInterval(timer);process.kill(-p.pid,'SIGINT');}
  },10);
  const timeout=setTimeout(()=>{try{process.kill(-p.pid,'SIGKILL')}catch{}},15000);
  const status=await new Promise((resolve,reject)=>{p.on('error',reject);p.on('exit',(code,signal)=>resolve({code,signal}));});
  clearTimeout(timeout);clearInterval(timer);
  try{process.kill(-p.pid,'SIGKILL')}catch{}
  return status;
}
let failed=false;
try {
  const status=await launch(spec);
  if(status.code!==0)throw Error('OS isolation suite failed: '+JSON.stringify(status));
  if(fs.readFileSync(secret,'utf8')!=='fake-secret'||fs.existsSync(path.join(base,'escaped')))throw Error('Symlink write escaped sandbox');
  if(fs.existsSync(path.join(workspace,'should-not-exist')))throw Error('Agent argv was shell-evaluated');
  const exit=await launch({...spec,argv:['/bin/sh','-c','exit 42']});
  if(exit.code!==42)throw Error('Agent exit code was not preserved');
  const missing=await launch({...spec,runtime_root:path.join(base,'missing'),argv:['/bin/sh','-c','touch should-not-launch']});
  if(missing.code!==125 || fs.existsSync(path.join(workspace,'should-not-launch')))throw Error('Runtime failure was not closed');
  const interrupt=await launch({...spec,argv:[node,'-e',"require('node:fs').writeFileSync('ready','yes');setInterval(()=>{},1000)"]},true);
  if(interrupt.signal!=='SIGINT')throw Error('SIGINT was not preserved: '+JSON.stringify(interrupt));
  console.log('PASS exit-code, missing-runtime fail-closed, SIGINT propagation');
} catch(error){console.error(error);failed=true;}
finally {
  server.close();unix.close();
  if(process.platform==='darwin')spawnSync('/usr/bin/security',['delete-keychain',keychain]);
  fs.rmSync(base,{recursive:true,force:true});
}
console.log({passed:!failed,platform:process.platform});process.exitCode=failed?1:0;
