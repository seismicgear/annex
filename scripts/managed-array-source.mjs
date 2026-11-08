// The only release-time source transformation is the allocated native version.
// Dependency identities and the developer's checkout remain unchanged.
import crypto from 'node:crypto';
import fs from 'node:fs/promises';
import path from 'node:path';
import {execFileSync} from 'node:child_process';

export const dependencyLocks=['Cargo.lock','client/package-lock.json','zk/package-lock.json','tools/tauri-cli/package-lock.json'];
export const sha256=bytes=>crypto.createHash('sha256').update(bytes).digest('hex');
const git=(root,args,env)=>execFileSync('git',args,{cwd:root,env,encoding:'utf8',windowsHide:true,maxBuffer:64*1024*1024});
const inside=(root,file)=>{const rel=path.relative(root,file);return !rel||!rel.startsWith('..'+path.sep)&&rel!=='..'&&!path.isAbsolute(rel);};

export function allocatedIdentity({version,buildId,sourceRevision,builtAt}){
  if(typeof version!=='string'||!/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(version)||version.split('.').some(n=>Number(n)>65535))throw Error('Use an allocated numeric Windows version with components between 0 and 65535.');
  if(typeof buildId!=='string'||!/^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/.test(buildId))throw Error('Use the allocated build ID.');
  if(typeof sourceRevision!=='string'||!/^[a-f0-9]{40}$/.test(sourceRevision))throw Error('Use the exact committed Annex source revision.');
  if(typeof builtAt!=='string'||!/^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d{1,3})?Z$/.test(builtAt)||!Number.isFinite(Date.parse(builtAt)))throw Error('Use a valid UTC build timestamp.');
  return {product:'annex',version,buildId,sourceRevision,builtAt};
}

function section(text,name){
  const escaped=name.replace(/[.*+?^${}()|[\]\\]/g,'\\$&');
  return new RegExp('^\\['+escaped+'\\]\\s*$([\\s\\S]*?)(?=^\\[|$(?![\\s\\S]))','m').exec(text)?.[1]||'';
}
export function workspaceMembers(text){
  const block=/\bmembers\s*=\s*\[([\s\S]*?)\]/.exec(section(text,'workspace'))?.[1];
  if(!block)throw Error('The Annex workspace must explicitly list its crates.');
  const members=[...block.matchAll(/"([^"]+)"/g)].map(match=>match[1]);
  if(!members.length||new Set(members).size!==members.length||members.some(name=>!/^crates\/annex-[a-z0-9-]+$/.test(name)))throw Error('Unsupported Annex workspace member path.');
  return members;
}
export function workspaceVersion(text){
  const value=/^version\s*=\s*"([^"]+)"\s*$/m.exec(section(text,'workspace.package'))?.[1];
  if(!value||!/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(value))throw Error('The Annex workspace version is invalid.');
  return value;
}

/** Preserve every third-party package, checksum, dependency constraint and
 * source. Only this workspace's own package versions/path pins may change. */
export function rewriteCargoLock(text,names,from,to){
  const seen=new Set();
  const result=text.split(/(?=^\[\[package\]\]\s*$)/m).map(block=>{
    const name=/^name = "([^"]+)"$/m.exec(block)?.[1];
    if(names.has(name)&&!/^source = /m.test(block)){
      if(seen.has(name)||!new RegExp('^version = "'+from.replaceAll('.','\\.')+'"$','m').test(block))throw Error('The workspace Cargo lock identity does not match '+name+'.');
      seen.add(name);block=block.replace(/^version = "[^"]+"$/m,'version = "'+to+'"');
    }
    return block.replace(/^(\s*")([a-z][a-z0-9-]*) ([0-9.]+)(",?)$/gm,(all,prefix,dependency,version,suffix)=>
      names.has(dependency)&&version===from?prefix+dependency+' '+to+suffix:all);
  }).join('');
  if(seen.size!==names.size)throw Error('The Cargo lock does not contain every workspace crate.');
  return result;
}
function rewriteManifest(text,names,from,to,{workspace=false}={}){
  if(workspace){
    const current=section(text,'workspace.package');
    if(workspaceVersion(text)!==from)throw Error('Workspace source version changed.');
    text=text.replace(current,current.replace(/^version\s*=\s*"[^"]+"\s*$/m,'version = "'+to+'"'));
  }
  return text.replace(/^([a-z][a-z0-9-]*)\s*=\s*\{[^\r\n]*\}\s*$/gm,line=>{
    const name=/^([a-z][a-z0-9-]*)/.exec(line)[1];
    if(!names.has(name)||! /\bpath\s*=/.test(line))return line;
    const declared=/\bversion\s*=\s*"(=?)([^"]+)"/.exec(line);
    if(!declared||declared[2]!==from)throw Error('Workspace path dependency has a different version: '+name+'.');
    return line.replace(declared[0],'version = "'+declared[1]+to+'"');
  });
}

export async function applyAnnexVersion(source,identity){
  allocatedIdentity(identity);
  const cargo=await fs.readFile(path.join(source,'Cargo.toml'),'utf8'),members=workspaceMembers(cargo),from=workspaceVersion(cargo);
  const names=new Set(),manifests=new Map([['Cargo.toml',cargo]]);
  for(const member of members){
    const relative=member+'/Cargo.toml',text=await fs.readFile(path.join(source,relative),'utf8');
    const name=/^name\s*=\s*"([^"]+)"\s*$/m.exec(section(text,'package'))?.[1];
    if(!name||name!==path.posix.basename(member)||names.has(name)||!/^version\.workspace\s*=\s*true\s*$/m.test(section(text,'package')))throw Error('Workspace crate must inherit its canonical version: '+member+'.');
    names.add(name);manifests.set(relative,text);
  }
  const files=new Map(manifests);
  for(const relative of [...dependencyLocks,'client/package.json','crates/annex-desktop/tauri.conf.json'])files.set(relative,await fs.readFile(path.join(source,relative),'utf8'));
  const sourceFiles=Object.fromEntries([...files].map(([name,bytes])=>[name,sha256(bytes)]));
  const pkg=JSON.parse(files.get('client/package.json')),lock=JSON.parse(files.get('client/package-lock.json')),tauri=JSON.parse(files.get('crates/annex-desktop/tauri.conf.json'));
  if(pkg.version!==from||lock.version!==from||lock.packages?.['']?.version!==from||tauri.version!==from||tauri.identifier!=='com.annex.desktop')throw Error('Cargo, Tauri and client package identities must agree before allocation.');
  const changes=new Map();
  if(identity.version!==from){
    for(const [name,text]of manifests)changes.set(name,rewriteManifest(text,names,from,identity.version,{workspace:name==='Cargo.toml'}));
    changes.set('Cargo.lock',rewriteCargoLock(files.get('Cargo.lock'),names,from,identity.version));
    pkg.version=identity.version;lock.version=identity.version;lock.packages[''].version=identity.version;tauri.version=identity.version;
    for(const [name,value]of [['client/package.json',pkg],['client/package-lock.json',lock],['crates/annex-desktop/tauri.conf.json',tauri]])changes.set(name,JSON.stringify(value,null,2)+'\n');
  }
  const changed=[];
  for(const [name,text]of changes)if(text!==files.get(name)){await fs.writeFile(path.join(source,name),text);changed.push(name);}
  const packagedFiles=Object.fromEntries(await Promise.all([...files.keys()].map(async name=>[name,sha256(await fs.readFile(path.join(source,name)))])));
  return {...identity,sourceVersion:from,sourceDirty:false,versionOverride:identity.version===from?null:{from,to:identity.version,files:changed.sort()},
    sourceFiles,packagedFiles,sourceDependencyLocks:Object.fromEntries(dependencyLocks.map(name=>[name,sourceFiles[name]])),
    packagedDependencyLocks:Object.fromEntries(dependencyLocks.map(name=>[name,packagedFiles[name]]))};
}

export async function prepareAnnexSource({productDir,sourceRevision,version,buildId,builtAt,destination,run,env=process.env}){
  const identity=allocatedIdentity({sourceRevision,version,buildId,builtAt});
  const root=await fs.realpath(productDir),source=path.resolve(destination);
  if(inside(root,source)||inside(source,root))throw Error('Build source must be a separate new directory outside the developer checkout.');
  if(git(root,['rev-parse','HEAD'],env).trim()!==sourceRevision||git(root,['status','--porcelain','--untracked-files=all'],env).trim())throw Error('Use the exact clean Annex source checkout.');
  // A local clone owns its index/config and never writes the developer's worktree.
  await run('git',['clone','--shared','--no-checkout','--config','core.autocrlf=false','--',root,source],{cwd:path.dirname(source),env});
  try{
    await run('git',['checkout','--detach',sourceRevision],{cwd:source,env});
    if(git(source,['rev-parse','HEAD'],env).trim()!==sourceRevision||git(source,['status','--porcelain'],env).trim())throw Error('The isolated source export is not the selected commit.');
    for(const name of dependencyLocks){
      const expected=execFileSync('git',['show',sourceRevision+':'+name],{cwd:root,env,maxBuffer:64*1024*1024});
      if(!expected.equals(await fs.readFile(path.join(source,name))))throw Error('Exported dependency lock changed: '+name+'.');
    }
    const receipt=await applyAnnexVersion(source,identity);
    if(git(root,['status','--porcelain','--untracked-files=all'],env).trim())throw Error('The developer checkout changed during staging.');
    return {source,receipt};
  }catch(error){await fs.rm(source,{recursive:true,force:true});throw error;}
}
