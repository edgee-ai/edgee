import fs from 'node:fs';import path from 'node:path';import vm from 'node:vm';import {spawnSync} from 'node:child_process';import assert from 'node:assert/strict';import os from 'node:os';import {fileURLToPath} from 'node:url';
const here=path.dirname(fileURLToPath(import.meta.url));
const base=fs.mkdtempSync(path.join(os.tmpdir(),'edgee-shim-test-'));
const fake=path.join(base,'bwrap');fs.writeFileSync(fake,'#!/bin/sh\nprintf "%s\\0" "$@"\n',{mode:0o700});
const source=fs.readFileSync(path.resolve(here,'../../src/commands/launch/sandbox/runtime-driver.mjs'),'utf8');
const f=source.slice(source.indexOf('function linuxBwrapWrapper()'),source.indexOf('\nasync function wrapped'));
const context={fs,path,process:{env:{PATH:base}},specPath:path.join(base,'spec'),quote:s=>"'"+s.replaceAll("'","'\\''")+"'",SetupError:Error,linuxWrapperDirectory:null};
const wrapper=vm.runInNewContext(f+';linuxBwrapWrapper()',context);
function run(args,profile){let fd;if(profile)fd=fs.openSync(profile,'r');const p=spawnSync('/bin/bash',['-p',wrapper,...args],{encoding:'utf8',stdio:['ignore','pipe','pipe',...(fd?[fd]:[])]});if(fd)fs.closeSync(fd);if(p.status!==0)throw Error(JSON.stringify({args,status:p.status,stderr:p.stderr}));return p.stdout.split('\0').slice(0,-1);}
const args=['--new-session','--setenv','TEST','--','--setenv','OTHER','--tmpfs','--tmpfs','/tmp/private root','--bind','/source','/tmp/private root/workspace','--','/bin/sh','-c','echo --tmpfs /'];
assert.deepEqual(run(args),[...args.slice(0,-4),'--remount-ro','/tmp/private root',...args.slice(-4)]);
const over=['--tmpfs','/workspace','--bind','/workspace','/workspace','--','/bin/true'];assert.deepEqual(run(over),over);
const file=path.join(base,'args');fs.writeFileSync(file,'--tmpfs\0/tmp/a b\0--bind\0/src\0/tmp/a b/workspace\0');assert.deepEqual(run(['--args','3','--','/bin/true'],file),['--tmpfs','/tmp/a b','--bind','/src','/tmp/a b/workspace','--remount-ro','/tmp/a b','--','/bin/true']);
assert.deepEqual(run(['--version']),['--version']);
console.log('PASS shim argv boundaries, setenv flag-shaped values, exact mount override, NUL response file, version passthrough');fs.rmSync(base,{recursive:true,force:true});fs.rmSync(context.linuxWrapperDirectory,{recursive:true,force:true});
