import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import {execFileSync} from 'node:child_process';
import {fileURLToPath} from 'node:url';
import {allocatedIdentity,applyAnnexVersion,prepareAnnexSource,workspaceMembers,rewriteCargoLock,dependencyLocks,sha256} from '../managed-array-source.mjs';
import {managedTauriConfiguration,buildProduct} from '../managed-array-release.mjs';

const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'../..');
const version='0.1.8',buildId='annex-0.1.8-fixture',builtAt='2026-11-07T12:00:00Z';
const git=(cwd,...args)=>execFileSync('git',args,{cwd,encoding:'utf8',stdio:['ignore','pipe','pipe']}).trim();
const run=async(file,args,options)=>{execFileSync(file,args,{...options,stdio:['ignore','pipe','pipe']});};
async function fixture(t){
  const directory=await fs.mkdtemp(path.join(os.tmpdir(),'annex-allocated-source-'));t.after(()=>fs.rm(directory,{recursive:true,force:true}));
  const source=path.join(directory,'developer');await fs.mkdir(source);
  const write=async(name,content)=>{await fs.mkdir(path.dirname(path.join(source,name)),{recursive:true});await fs.writeFile(path.join(source,name),content);};
  const names=['annex-types','annex-server','annex-desktop'];
  await write('Cargo.toml','[workspace]\nmembers = [\n'+names.map(n=>'  "crates/'+n+'",').join('\n')+'\n]\n\n[workspace.package]\nversion = "0.1.3"\nedition = "2021"\n\n[workspace.dependencies]\nannex-types = { path = "crates/annex-types", version = "=0.1.3" }\nforeign = { version = "0.1.3" }\n');
  for(const name of names)await write('crates/'+name+'/Cargo.toml','[package]\nname = "'+name+'"\nversion.workspace = true\n\n[dependencies]\n'+(name==='annex-desktop'?'annex-server = { path = "../annex-server", version = "=0.1.3" }\n':'')+'foreign = { version = "0.1.3" }\n');
  await write('Cargo.lock','version = 4\n\n'+names.map(name=>'[[package]]\nname = "'+name+'"\nversion = "0.1.3"\ndependencies = ["foreign"]\n\n').join('')+'[[package]]\nname = "foreign"\nversion = "0.1.3"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\nchecksum = "'+'a'.repeat(64)+'"\n');
  await write('client/package.json',JSON.stringify({name:'client',version:'0.1.3',private:true})+'\n');
  await write('client/package-lock.json',JSON.stringify({name:'client',version:'0.1.3',lockfileVersion:3,packages:{'':{name:'client',version:'0.1.3',dependencies:{foreign:'0.1.3'}},'node_modules/foreign':{version:'0.1.3',integrity:'sha512-unchanged'}}})+'\n');
  for(const prefix of ['zk','tools/tauri-cli'])await write(prefix+'/package-lock.json',JSON.stringify({name:prefix,version:'0.1.0',packages:{'':{version:'0.1.0'}}})+'\n');
  await write('crates/annex-desktop/tauri.conf.json',JSON.stringify({productName:'Annex',identifier:'com.annex.desktop',version:'0.1.3',plugins:{'deep-link':{desktop:{schemes:['annex']}}}})+'\n');
  git(source,'init');git(source,'config','core.autocrlf','false');git(source,'add','.');git(source,'-c','user.name=Fixture','-c','user.email=fixture@example.invalid','commit','-m','Exact synthetic source');
  const sourceRevision=git(source,'rev-parse','HEAD');
  return {directory,source,identity:{sourceRevision,version,buildId,builtAt},destination:path.join(directory,'release-source')};
}

test('allocated Annex versions stage an exact isolated commit and preserve developer source and dependency identities',async t=>{
  const f=await fixture(t),original=await fs.readFile(path.join(f.source,'Cargo.lock'),'utf8');
  const {source,receipt}=await prepareAnnexSource({productDir:f.source,...f.identity,destination:f.destination,run});
  assert.equal(git(source,'rev-parse','HEAD'),f.identity.sourceRevision);
  assert.equal(git(f.source,'status','--porcelain'),'');assert.equal(await fs.readFile(path.join(f.source,'Cargo.lock'),'utf8'),original);
  assert.equal(receipt.version,version);assert.equal(receipt.sourceVersion,'0.1.3');assert.equal(receipt.buildId,buildId);assert.equal(receipt.sourceDirty,false);
  assert.equal(JSON.parse(await fs.readFile(path.join(source,'client/package.json'))).version,version);
  const config=JSON.parse(await fs.readFile(path.join(source,'crates/annex-desktop/tauri.conf.json')));
  assert.equal(config.version,version);assert.equal(config.identifier,'com.annex.desktop');assert.deepEqual(config.plugins['deep-link'].desktop.schemes,['annex']);
  assert.match(await fs.readFile(path.join(source,'Cargo.toml'),'utf8'),/annex-types = \{ path = "crates\/annex-types", version = "=0\.1\.8" \}/);
  assert.match(await fs.readFile(path.join(source,'crates/annex-desktop/Cargo.toml'),'utf8'),/version = "=0\.1\.8"/);
  const transformed=await fs.readFile(path.join(source,'Cargo.lock'),'utf8');
  assert.equal(transformed.split('[[package]]').find(v=>v.includes('name = "foreign"')),original.split('[[package]]').find(v=>v.includes('name = "foreign"')));
  const lock=JSON.parse(await fs.readFile(path.join(source,'client/package-lock.json')));
  assert.equal(lock.packages['node_modules/foreign'].version,'0.1.3');assert.equal(lock.packages[''].dependencies.foreign,'0.1.3');
  for(const name of dependencyLocks){assert.equal(receipt.sourceDependencyLocks[name],sha256(await fs.readFile(path.join(f.source,name))));assert.equal(receipt.packagedDependencyLocks[name],sha256(await fs.readFile(path.join(source,name))));}
  assert.notEqual(receipt.sourceDependencyLocks['Cargo.lock'],receipt.packagedDependencyLocks['Cargo.lock']);
  assert.equal(receipt.sourceDependencyLocks['zk/package-lock.json'],receipt.packagedDependencyLocks['zk/package-lock.json']);
});

test('unchanged numeric allocation preserves every version and dependency file byte',async t=>{
  const f=await fixture(t),receipt=await applyAnnexVersion(f.source,{...f.identity,version:'0.1.3'});
  assert.equal(receipt.versionOverride,null);assert.deepEqual(receipt.sourceFiles,receipt.packagedFiles);assert.equal(git(f.source,'status','--porcelain'),'');
});

test('source staging rejects dirty trees, different commits and a destination inside the developer checkout',async t=>{
  const f=await fixture(t);
  await assert.rejects(prepareAnnexSource({productDir:f.source,...f.identity,sourceRevision:'f'.repeat(40),destination:f.destination,run}),/exact clean/);
  await assert.rejects(prepareAnnexSource({productDir:f.source,...f.identity,destination:path.join(f.source,'build'),run}),/separate new directory/);
  await fs.writeFile(path.join(f.source,'uncommitted.txt'),'owner work');
  await assert.rejects(prepareAnnexSource({productDir:f.source,...f.identity,destination:f.destination,run}),/exact clean/);
  assert.equal(await fs.readFile(path.join(f.source,'uncommitted.txt'),'utf8'),'owner work');
  await assert.rejects(fs.stat(f.destination),{code:'ENOENT'});
});

test('version transformations refuse source mismatch and cannot relabel a third-party lock entry',async t=>{
  const f=await fixture(t);
  await fs.writeFile(path.join(f.source,'client/package.json'),JSON.stringify({version:'0.1.4'}));
  const before=await fs.readFile(path.join(f.source,'Cargo.toml'));
  await assert.rejects(applyAnnexVersion(f.source,f.identity),/must agree/);
  assert.deepEqual(await fs.readFile(path.join(f.source,'Cargo.toml')),before);
  assert.throws(()=>rewriteCargoLock('[[package]]\nname = "annex-types"\nversion = "0.1.3"\nsource = "registry+https://example.invalid"\n',new Set(['annex-types']),'0.1.3',version),/every workspace crate/);
  for(const invalid of ['0.1.8-beta','01.1.8','1.2.65536','1.2'])assert.throws(()=>allocatedIdentity({...f.identity,version:invalid}),/numeric Windows version/);
  assert.throws(()=>allocatedIdentity({...f.identity,buildId:'https://private/?token=secret'}),/build ID/);
});

test('current canonical Annex workspace and all own lock entries accept allocation without changing external dependencies',async t=>{
  const directory=await fs.mkdtemp(path.join(os.tmpdir(),'annex-real-version-'));t.after(()=>fs.rm(directory,{recursive:true,force:true}));
  const cargo=await fs.readFile(path.join(root,'Cargo.toml'),'utf8');
  const names=['Cargo.toml',...workspaceMembers(cargo).map(v=>v+'/Cargo.toml'),...dependencyLocks,'client/package.json','crates/annex-desktop/tauri.conf.json'];
  for(const name of names){await fs.mkdir(path.dirname(path.join(directory,name)),{recursive:true});await fs.copyFile(path.join(root,name),path.join(directory,name));}
  const identity={sourceRevision:'a'.repeat(40),version:'0.2.19',buildId:'annex-contract-fixture',builtAt};
  const receipt=await applyAnnexVersion(directory,identity);
  assert.equal(receipt.version,'0.2.19');assert.equal(receipt.versionOverride.from,'0.1.3');
  const oldBlocks=(await fs.readFile(path.join(root,'Cargo.lock'),'utf8')).split('[[package]]').filter(v=>/^source = /m.test(v));
  const newBlocks=(await fs.readFile(path.join(directory,'Cargo.lock'),'utf8')).split('[[package]]').filter(v=>/^source = /m.test(v));
  assert.deepEqual(newBlocks,oldBlocks);
});

test('managed Windows bundling keeps one update authority and the explicit native signing argv',async()=>{
  const command={cmd:'C:\\Program Files\\SDK\\signtool.exe',args:['sign','/dmdf','C:\\private inputs\\metadata.json','%1']};
  const config=managedTauriConfiguration({version,signCommand:command});
  assert.deepEqual(config.bundle.targets,['nsis']);assert.equal(config.bundle.createUpdaterArtifacts,false);assert.equal(config.plugins.updater,null);
  assert.deepEqual(config.bundle.windows.signCommand,command);
  if(process.platform!=='win32')await assert.rejects(buildProduct({}),/existing Windows x64 build host/);
});

test('the actual workflow reports confirmed root publication receipts and refuses a different product',async t=>{
  const directory=await fs.mkdtemp(path.join(os.tmpdir(),'annex-summary-'));
  t.after(()=>fs.rm(directory,{recursive:true,force:true}));
  const workflow=await fs.readFile(new URL('../../.github/workflows/release-array-employee.yml',import.meta.url),'utf8');
  assert.doesNotMatch(workflow.split('    steps:\n')[0],/\$\{\{\s*runner\./,'runner context is unavailable in job-level env');
  const match=/^          @'\n([\s\S]*?)^          '@ \| node -$/m.exec(workflow);
  assert.ok(match,'execute the actual workflow summary program');
  const script=match[1].replace(/^          /gm,'');
  const output=path.join(directory,'output'),summary=path.join(directory,'summary.md');
  const execute=()=>execFileSync(process.execPath,['-'],{input:script,encoding:'utf8',stdio:['pipe','pipe','pipe'],env:{...process.env,ARRAY_RELEASE_OUTPUT_DIR:output,GITHUB_STEP_SUMMARY:summary}});
  execute();assert.match(await fs.readFile(summary,'utf8'),/No confirmed employee publication receipt/);
  await fs.mkdir(path.join(output,'allocated-build/win'),{recursive:true});
  await fs.writeFile(path.join(output,'allocated-build/win/release-artifacts.json'),JSON.stringify({product:'annex',mode:'signed'}));
  const receipt={schemaVersion:1,product:'annex',channel:'employee',sourceRevision:'a'.repeat(40),version:version,buildId:buildId,artifactSha256:'b'.repeat(64),nativeInstallAcceptance:'not-run',publication:{id:'release-id',published:true},secretUnrelatedField:'never render this'};
  const receiptFile=path.join(output,'publication-employee-1-aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee.json');
  await fs.writeFile(receiptFile,JSON.stringify(receipt));await fs.writeFile(summary,'');execute();
  const written=await fs.readFile(summary,'utf8');assert.match(written,/employee publication/);assert.ok(written.includes(receipt.artifactSha256));assert.doesNotMatch(written,/never render this/);
  await fs.writeFile(receiptFile,JSON.stringify({...receipt,product:'different-product'}));
  assert.throws(execute,/Command failed/);
});
