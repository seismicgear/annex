// Called by the reviewed, pinned Array managed publisher. This module only
// builds product bytes; the website remains the registry and storage authority.
import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import {execFileSync} from 'node:child_process';
import {pathToFileURL} from 'node:url';
import {prepareAnnexSource,workspaceVersion,dependencyLocks,sha256} from './managed-array-source.mjs';

export const product='annex';
export async function baseVersion({productDir,sourceRevision}){
  return workspaceVersion(execFileSync('git',['show',sourceRevision+':Cargo.toml'],{cwd:productDir,encoding:'utf8'}));
}
const moduleAt=(root,name)=>import(pathToFileURL(path.join(root,name)).href);
const ordinary=async file=>{const s=await fs.lstat(file);if(!s.isFile()||s.isSymbolicLink())throw Error('A release input must be an ordinary file.');return s;};

export async function verifyProduct({productDir,sourceRevision,arrayDir,run,env}){
  if(process.platform!=='win32'||process.arch!=='x64')throw Error('Managed Annex release packaging requires the existing Windows x64 build host.');
  const {testEnvironment}=await moduleAt(arrayDir,'scripts/managed-release.mjs');
  const clean=testEnvironment(env),options={cwd:productDir,env:clean};
  for(const prefix of ['client','zk','tools/tauri-cli'])await run('npm',['ci','--prefix',prefix,'--no-audit','--no-fund',...(prefix==='tools/tauri-cli'?['--ignore-scripts']:[])],options);
  await run(process.execPath,['scripts/verify-libclang.mjs'],options);
  await run('bash',['scripts/tests/version-sync.test.sh'],options);
  await run('bash',['scripts/verify-production-rejects-dev-fixtures.sh'],options);
  const tests=['managed-array-release','desktop-assets','desktop-download','pinned-git-source','desktop-release','windows-runtime','release-write-eligibility','release-draft-upload'];
  await run(process.execPath,['--test',...tests.map(name=>'scripts/tests/'+name+'.test.mjs')],options);
  await run('npm',['--prefix','client','test'],options);
  await run('cargo',['test','--locked','--workspace','--exclude','annex-desktop'],options);
  await run('cargo',['test','--locked','--manifest-path','scripts/array-update-regressions/Cargo.toml'],options);
  if(execFileSync('git',['rev-parse','HEAD'],options).toString().trim()!==sourceRevision)throw Error('Annex source changed during verification.');
}

export function managedTauriConfiguration({version,signCommand}){
  return {version,bundle:{targets:['nsis'],createUpdaterArtifacts:false,windows:{signCommand}},plugins:{updater:null}};
}

export async function buildProduct({productDir,sourceRevision,version,buildId,builtAt,out,policyFile,arrayDir,run,env}){
  if(process.platform!=='win32'||process.arch!=='x64')throw Error('Managed Annex release packaging requires the existing Windows x64 build host.');
  const [{testEnvironment},{loadSigningConfig},{createRunner,createPowerShell},{writeMetadata,signtoolSignArgs,verifyWindowsFile,fileVersionScript,preflightWindows},
    {SigningSession,sha256File},{artifactRecord,writeReleaseArtifacts},{evidenceMatchesPolicy}]=await Promise.all([
    moduleAt(arrayDir,'scripts/managed-release.mjs'),moduleAt(arrayDir,'scripts/signing/config.mjs'),moduleAt(arrayDir,'scripts/signing/exec.mjs'),
    moduleAt(arrayDir,'scripts/signing/windows.mjs'),moduleAt(arrayDir,'scripts/signing/session.mjs'),moduleAt(arrayDir,'scripts/signing/release-metadata.mjs'),moduleAt(arrayDir,'scripts/signing/policy.mjs')]);
  const clean=testEnvironment(env),configuration=loadSigningConfig({platforms:['win32'],revision:sourceRevision,env,host:process.platform});
  if(configuration.errors.length)throw Error('Native signing is not configured: '+configuration.errors.join(' '));
  const nativeRun=createRunner({env}),powershell=createPowerShell(nativeRun),windows=configuration.windows;
  const readiness=await preflightWindows({windows,powershell});if(readiness.problems.length)throw Error(readiness.problems.join(' '));
  const policy=JSON.parse(await fs.readFile(policyFile,'utf8'));
  await fs.mkdir(out,{recursive:true});if((await fs.readdir(out)).length)throw Error('Choose a new empty managed Annex artifact directory.');
  const work=await fs.mkdtemp(path.join(os.tmpdir(),'annex-array-release-'));
  const session=new SigningSession({logsDir:path.join(out,'verification-logs'),dryRun:false});
  try{
    const {source,receipt}=await prepareAnnexSource({productDir,sourceRevision,version,buildId,builtAt,destination:path.join(work,'source'),run,env:clean});
    // Reuse only dependencies installed by npm ci against the recorded source
    // locks. The version transformation does not change a dependency entry.
    for(const prefix of ['client','zk','tools/tauri-cli']){
      const lock=prefix+'/package-lock.json';
      if(sha256(await fs.readFile(path.join(productDir,lock)))!==receipt.sourceDependencyLocks[lock])throw Error('Verified dependency lock changed before packaging.');
      await fs.symlink(path.join(productDir,prefix,'node_modules'),path.join(source,prefix,'node_modules'),'junction');
    }
    const buildEnv={...clean,ANNEX_BUILD_PROFILE:'production',CMAKE_ARGS:'-DCMAKE_POLICY_VERSION_MINIMUM=3.5',
      CARGO_TARGET_DIR:path.join(work,'target'),ANNEX_RELEASE_BUILD_ID:buildId,ANNEX_RELEASE_SOURCE_REVISION:sourceRevision};
    const options={cwd:source,env:buildEnv};
    await run(process.execPath,['scripts/verify-artifacts.js','--all'],{cwd:path.join(source,'zk'),env:buildEnv});
    await run(process.execPath,['scripts/verify-ceremony.js'],{cwd:path.join(source,'zk'),env:buildEnv});
    await run('powershell.exe',['-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-File','scripts/prepare-desktop-assets.ps1','-WorkDir',path.join(work,'native-assets')],options);
    await run(process.execPath,['scripts/prepare-desktop-assets.mjs','--verify','--target','x86_64-pc-windows-msvc'],options);
    await run('cargo',['metadata','--locked','--no-deps','--format-version','1'],options);
    const desktop=path.join(source,'crates/annex-desktop'),tauri=path.join(source,'tools/tauri-cli/node_modules/@tauri-apps/cli/tauri.js');
    const sourceConfig=JSON.parse(await fs.readFile(path.join(desktop,'tauri.conf.json'),'utf8'));
    if(sourceConfig.build?.beforeBundleCommand)throw Error('A new beforeBundleCommand needs explicit credential-free staging before managed signing.');
    const metadata=await writeMetadata(windows,path.join(work,'native-signing'));
    const configurationFile=path.join(work,'managed-tauri.json');
    const signCommand={cmd:windows.signtoolPath,args:signtoolSignArgs({file:'%1',dlibPath:windows.dlibPath,metadataPath:metadata.file,timestampUrl:windows.timestampUrl})};
    await fs.writeFile(configurationFile,JSON.stringify(managedTauriConfiguration({version,signCommand})),{flag:'wx'});
    const common=['--target','x86_64-pc-windows-msvc','--config',configurationFile];
    // Rust/frontend code runs without signing authority. Tauri's native bundle
    // step signs after its binary patching, before embedding the executable.
    await run(process.execPath,[tauri,'build','--no-bundle',...common,'--','--locked'],{cwd:desktop,env:buildEnv});
    await run(process.execPath,[tauri,'bundle',...common,'--bundles','nsis'],{cwd:desktop,env:{...buildEnv,...Object.fromEntries(Object.entries(env).filter(([name])=>/^(AZURE_|ARTIFACT_SIGNING_|SIGNTOOL_PATH$)/.test(name)))}});
    const release=path.join(work,'target/x86_64-pc-windows-msvc/release');
    await run(process.execPath,['scripts/windows-runtime.mjs','--directory',release],options);
    await run('powershell.exe',['-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-File','scripts/verify-webview2-bundle.ps1','-BuildDir',release],options);
    const installers=(await fs.readdir(path.join(release,'bundle/nsis'))).filter(name=>name.endsWith('.exe'));
    if(installers.length!==1)throw Error('Expected exactly one real NSIS installer.');
    const installedExecutable=path.join(release,'annex-desktop.exe');await ordinary(installedExecutable);
    const actualVersion=(await powershell(fileVersionScript(installedExecutable))).stdout.trim();
    if(actualVersion!==version&&actualVersion!==version+'.0')throw Error('The actual native executable does not carry the allocated version.');
    const file=path.join(out,'Annex-'+version+'-win32-x64.exe');
    await fs.copyFile(path.join(release,'bundle/nsis',installers[0]),file,1);
    const executableEvidence=await verifyWindowsFile(installedExecutable,{windows,run:nativeRun,powershell,session});
    const evidence=await verifyWindowsFile(file,{windows,run:nativeRun,powershell,session});
    for(const verified of [executableEvidence,evidence]){const problem=evidenceMatchesPolicy(verified,policy,{product});if(problem)throw Error(problem);}
    for(const lock of dependencyLocks)if(sha256(await fs.readFile(path.join(source,lock)))!==receipt.packagedDependencyLocks[lock])throw Error('Native build changed a dependency lock: '+lock+'.');
    const artifact=await artifactRecord({out,file,platform:'win32',arch:'x64',kind:'nsis',executable:'annex-desktop.exe',executableSha256:await sha256File(installedExecutable),codeSigning:evidence,executableCodeSigning:executableEvidence});
    const document={schemaVersion:1,...receipt,mode:'signed',releasePolicySha256:sha256(await fs.readFile(policyFile)),
      artifacts:[artifact],supplementary:[],nativeInstallAcceptance:'not-run',verification:{nativeVersion:actualVersion,signingSteps:session.steps}};
    await writeReleaseArtifacts(out,document);return path.join(out,'release-artifacts.json');
  }finally{await fs.rm(work,{recursive:true,force:true});}
}
