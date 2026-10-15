#!/usr/bin/env node
// Locate a runner-installed compiler library; never download executable code.
import fs from 'node:fs';
import path from 'node:path';
import {spawnSync} from 'node:child_process';
const candidates=[];
if(process.env.LIBCLANG_PATH)candidates.push(process.env.LIBCLANG_PATH);
if(process.platform==='win32'){
  candidates.push(path.join(process.env.ProgramFiles??'C:\\Program Files','LLVM','bin'));
  const installer=path.join(process.env['ProgramFiles(x86)']??'C:\\Program Files (x86)','Microsoft Visual Studio','Installer','vswhere.exe');
  const result=spawnSync(installer,['-latest','-products','*','-property','installationPath'],{encoding:'utf8',windowsHide:true});
  if(result.status===0 && result.stdout.trim()) for(const suffix of ['x64/bin','bin'])candidates.push(path.join(result.stdout.trim(),'VC','Tools','Llvm',suffix));
}else if(process.platform==='darwin'){
  const selected=spawnSync('xcode-select',['-p'],{encoding:'utf8'});
  if(selected.status===0)candidates.push(path.join(selected.stdout.trim(),'Toolchains','XcodeDefault.xctoolchain','usr','lib'),path.join(selected.stdout.trim(),'usr','lib'));
}else{
  const result=spawnSync('ldconfig',['-p'],{encoding:'utf8'});
  if(result.status===0)for(const line of result.stdout.split('\n'))if(/libclang(?:-\d+)?\.so/.test(line)&&line.includes('=>'))candidates.push(path.dirname(line.split('=>')[1].trim()));
  if(fs.existsSync('/usr/lib'))for(const entry of fs.readdirSync('/usr/lib'))if(/^llvm-\d+$/.test(entry))candidates.push(path.join('/usr/lib',entry,'lib'));
}
const found=candidates.find(directory=>{
  try{return fs.readdirSync(directory).some(name=>/^libclang(?:-\d+)?\.(?:dll|dylib|so(?:\.\d+)*)$/.test(name)&&fs.statSync(path.join(directory,name)).isFile());}catch{return false;}
});
if(!found){console.error('A native libclang is required for the pinned speech DSP binding. Install libclang-dev on Linux or the LLVM/Xcode compiler tools on Windows/macOS.');process.exitCode=1;}
else{
  console.log(`Verified native libclang directory: ${found}`);
  if(process.env.GITHUB_ENV)fs.appendFileSync(process.env.GITHUB_ENV,`LIBCLANG_PATH=${found}\n`);
}
