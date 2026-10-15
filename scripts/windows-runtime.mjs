#!/usr/bin/env node
// Inspect actual PE imports, including delay loads, without loading binaries or
// searching the build machine's PATH/System32 for redistributable libraries.
import fs from 'node:fs/promises';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { pathToFileURL } from 'node:url';

const inbox = new Set(`advapi32 avrt bcrypt bcryptprimitives cabinet cfgmgr32 comctl32 combase comdlg32 credui crypt32 cryptbase cryptsp d2d1 d3d11 d3d12 d3dcompiler_47 dcomp dnsapi dsound dwmapi dwrite dxgi dwmapi gdi32 hid imm32 iphlpapi kernel32 kernelbase mf mfplat mfreadwrite mfuuid mmdevapi mpr mscms msimg32 msvcrt mswsock ncrypt netapi32 ntdll ole32 oleacc oleaut32 powrprof propsys psapi rpcrt4 secur32 sechost setupapi shell32 shcore shlwapi user32 userenv usp10 uxtheme version win32u windowscodecs winhttp wininet winmm winspool wintrust wldap32 ws2_32 wtsapi32`.split(' ').map(name => `${name}.dll`));
export function isInboxImport(name) {
  const normalized = name.toLowerCase();
  return /^(?:api|ext)-ms-[a-z0-9-]+\.dll$/.test(normalized) || inbox.has(normalized);
}

export function peImports(bytes) {
  function need(offset, length) {
    if (!Number.isSafeInteger(offset) || offset < 0 || length < 0 || offset + length > bytes.length) throw new Error('Truncated or invalid PE image');
  }
  const u16 = offset => { need(offset,2); return bytes.readUInt16LE(offset); };
  const u32 = offset => { need(offset,4); return bytes.readUInt32LE(offset); };
  if (u16(0) !== 0x5a4d) throw new Error('Not a PE executable');
  const pe = u32(0x3c);
  if (u32(pe) !== 0x4550 || u16(pe+4) !== 0x8664) throw new Error('Expected an x64 PE executable');
  const sections = u16(pe+6), optionalSize = u16(pe+20), optional = pe+24;
  if (sections < 1 || sections > 96 || optionalSize < 112 || u16(optional) !== 0x20b) throw new Error('Invalid PE64 headers');
  need(optional,optionalSize);
  const directoryCount = u32(optional+108);
  if (directoryCount > 16 || 112+directoryCount*8 > optionalSize) throw new Error('Invalid PE data directories');
  const sectionBase = optional+optionalSize;
  need(sectionBase,sections*40);
  function rvaOffset(rva) {
    for(let index=0;index<sections;index++) {
      const section=sectionBase+index*40, address=u32(section+12), rawSize=u32(section+16), raw=u32(section+20);
      if(rva>=address && rva-address<rawSize) { const offset=raw+rva-address;need(offset,1);return offset; }
    }
    throw new Error(`PE RVA outside raw sections: ${rva}`);
  }
  function dllName(rva) {
    const start=rvaOffset(rva), end=bytes.indexOf(0,start);
    if(end<start || end-start>255) throw new Error('Invalid PE import name');
    const name=bytes.subarray(start,end).toString('ascii');
    if(!/^[a-zA-Z0-9_.-]+\.dll$/i.test(name)) throw new Error(`Unsafe PE import name: ${name}`);
    return name.toLowerCase();
  }
  const imports=new Set();
  for(const [directory,stride,nameOffset] of [[1,20,12],[13,32,4]]) {
    if(directory>=directoryCount) continue;
    const address=u32(optional+112+directory*8), size=u32(optional+116+directory*8);
    if(!address && !size) continue;
    if(!address || size<stride || size>1024*1024) throw new Error('Invalid PE import directory');
    const start=rvaOffset(address); need(start,size);
    let terminated=false;
    for(let offset=start;offset+stride<=start+size;offset+=stride) {
      if(bytes.subarray(offset,offset+stride).every(byte=>byte===0)){terminated=true;break;}
      // PE64 delay import descriptors must use relative addresses.
      if(directory===13 && u32(offset)!==1) throw new Error('Unsupported non-RVA delay import descriptor');
      imports.add(dllName(u32(offset+nameOffset)));
    }
    if(!terminated) throw new Error('Unterminated PE import directory');
  }
  return [...imports].sort();
}

export async function inspectDirectory(directory, expectedExecutable) {
  const entries=await fs.readdir(directory,{withFileTypes:true});
  const files=new Map();
  // The NSIS uninstaller is a separate 32-bit installer program. Inspect the
  // application entrypoint and every library in its loader search directory.
  for(const entry of entries.filter(entry=>/\.dll$/i.test(entry.name) || entry.name.toLowerCase()===expectedExecutable.toLowerCase())) {
    if(!entry.isFile()) throw new Error(`Native binary is not a regular file: ${entry.name}`);
    const key=entry.name.toLowerCase();
    if(files.has(key)) throw new Error(`Case-colliding native binaries: ${entry.name}`);
    files.set(key,entry.name);
  }
  if(!files.has(expectedExecutable.toLowerCase())) throw new Error(`Missing expected executable: ${expectedExecutable}`);
  const inventory=[];
  for(const [name,actual] of files) {
    const bytes=await fs.readFile(path.join(directory,actual));
    const imports=peImports(bytes);
    const missing=imports.filter(dependency=>!files.has(dependency) && !isInboxImport(dependency));
    if(missing.length) throw new Error(`${actual} has unbundled imports: ${missing.join(', ')}`);
    inventory.push({name,bytes:bytes.length,sha256:createHash('sha256').update(bytes).digest('hex'),imports});
  }
  return inventory.sort((a,b)=>a.name.localeCompare(b.name));
}

export async function inspectBundle(directory) {
  return {
    app:await inspectDirectory(directory,'annex-desktop.exe'),
    piper:await inspectDirectory(path.join(directory,'_up_','_up_','assets','piper'),'piper.exe'),
    whisper:await inspectDirectory(path.join(directory,'_up_','_up_','assets','whisper'),'whisper-cli.exe'),
  };
}

export async function inspectRuntimeProvenance(directory) {
  const receiptBytes = await fs.readFile(path.join(directory, 'windows-runtime-provenance.json'));
  const receipt = JSON.parse(receiptBytes);
  if (receipt.version !== 1 || receipt.architecture !== 'x86_64' || receipt.source !== 'Visual Studio 2022 Microsoft.VC143.CRT' || !Array.isArray(receipt.files)) {
    throw new Error('Invalid bundled Microsoft runtime provenance');
  }
  const roots = { app: directory, piper: path.join(directory, '_up_', '_up_', 'assets', 'piper'), whisper: path.join(directory, '_up_', '_up_', 'assets', 'whisper') };
  const names = new Set();
  for (const file of receipt.files) {
    if (!/^(app|piper|whisper)\/[A-Za-z0-9_.-]+$/.test(file.path) || names.has(file.path.toLowerCase())) throw new Error('Invalid or repeated runtime provenance path');
    names.add(file.path.toLowerCase());
    const [component, name] = file.path.split('/');
    const actual = await fs.readFile(path.join(roots[component], name));
    if (actual.length !== file.bytes || createHash('sha256').update(actual).digest('hex') !== file.sha256) throw new Error(`Bundled Microsoft runtime differs from provenance: ${file.path}`);
  }
  for (const component of Object.keys(roots)) {
    for (const required of ['msvcp140.dll', 'vcruntime140.dll', 'vcruntime140_1.dll']) {
      if (!names.has(`${component}/${required}`)) throw new Error(`Missing runtime provenance: ${component}/${required}`);
    }
  }
  return { sha256: createHash('sha256').update(receiptBytes).digest('hex'), ...receipt };
}

if(process.argv[1] && import.meta.url===pathToFileURL(path.resolve(process.argv[1])).href) {
  try {
    const args=process.argv.slice(2);
    if(args.length!==2 || args[0]!=='--directory') throw new Error('Usage: node scripts/windows-runtime.mjs --directory <actual Tauri Windows output directory>');
    const directory=path.resolve(args[1]);
    const files=await inspectBundle(directory);
    const runtime=await inspectRuntimeProvenance(directory);
    const receipt={version:1,checked_utc:new Date().toISOString(),directory,build_machine_runtime_search:false,runtime,files};
    await fs.writeFile(path.join(directory,'windows-native-closure.json'),JSON.stringify(receipt,null,2)+'\n');
    console.log(`[windows-runtime] dependency closure verified for ${Object.values(files).flat().length} actual PE binaries`);
  } catch(error) { console.error(`[windows-runtime] ${error.message}`);process.exitCode=1; }
}
