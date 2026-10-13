import { test, expect, type APIResponse } from '@playwright/test';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { fileURLToPath } from 'node:url';
import { admin, baseURL, newActor, read, select, send, type Actor } from './operator-helpers';
import type { Channel, Message, ServerPolicy } from '../../src/types';

const exec=promisify(execFile);
const script=(name:string)=>fileURLToPath(new URL(`../../../scripts/live/${name}.mjs`,import.meta.url));
const actors:Actor[]=[];let founder:Actor;let member:Actor;
let channel:Channel;
interface StorageHealth {state:string;reason:string;writes_blocked:boolean}
interface StorageControl {action:string;bytes:number;cap:number|null;session:string}
const session=process.env.ANNEX_BOUNDARY_SESSION;
async function storage(action:string):Promise<StorageControl>{
  expect(session,'Explicit isolated session record required for real cap/maintenance controls').toBeTruthy();
  const result=await exec(process.execPath,[script('storage-control'),session!,action],{timeout:70_000,maxBuffer:1024*1024});
  return JSON.parse(result.stdout.trim());
}
async function settings(){await admin(founder,'Server Settings');}
async function openSearch(who:Actor,query:string){
  if(!await who.page.getByRole('search').isVisible())await who.page.getByRole('button',{name:'Search messages',exact:true}).click();
  const search=who.page.getByRole('search');await search.getByRole('textbox',{name:'Search messages',exact:true}).fill(query);return search;
}
async function searchResult(who:Actor,query:string){
  const search=await openSearch(who,query);
  const refusals:Promise<{status:number;error:unknown}>[]=[];
  const observe=(response:import('@playwright/test').Response)=>{
    const url=new URL(response.url());
    if(url.pathname==='/api/messages/search'&&url.searchParams.get('q')===query&&!response.ok()){
      refusals.push(response.json().then(body=>({status:response.status(),error:body.error as unknown})));
    }
  };
  who.page.on('response',observe);
  try {
    const pending=who.page.waitForResponse(r=>new URL(r.url()).pathname==='/api/messages/search'&&new URL(r.url()).searchParams.get('q')===query&&r.status()===200,{timeout:90_000});
    await search.getByRole('button',{name:'Search',exact:true}).click();const response=await pending;
    const observed=await Promise.all(refusals);
    expect(observed.length,'At most one narrowly typed retired-root refusal may precede the exact request retry').toBeLessThanOrEqual(1);
    for(const refusal of observed)expect(refusal).toEqual({status:403,error:'zk_root_retired'});
    return {search,body:await response.json() as {complete:boolean;scanned_per_channel:number;results:Message[]},refusals:observed};
  } finally {who.page.off('response',observe);}
}
async function dataSocket(who:Actor){
  // This helper is an independent actual transport client using a genuine
  // browser registration. Close its browser before connecting, avoiding session takeover.
  await who.page.close();const url=new URL('/ws',baseURL);url.protocol=url.protocol==='https:'?'wss:':'ws:';
  url.searchParams.set('token',who.headers.authorization.replace(/^Bearer /,''));const ws=new WebSocket(url);
  await new Promise<void>((resolve,reject)=>{ws.addEventListener('open',()=>resolve(),{once:true});ws.addEventListener('error',()=>reject(new Error('Actual data socket failed')),{once:true});});
  ws.send(JSON.stringify({type:'subscribe',channelId:channel.channel_id}));
  return {ws,async publish(content:string){
    await new Promise(resolve=>setTimeout(resolve,105));
    const clientRequestId=crypto.randomUUID();
    const pending=new Promise<Record<string,unknown>>((resolve,reject)=>{
      const timer=setTimeout(()=>{ws.removeEventListener('message',listener);reject(new Error('Actual storage write acknowledgment timed out'));},30_000);
      const listener=(event:MessageEvent)=>{const frame=JSON.parse(String(event.data));if(frame.clientRequestId!==clientRequestId)return;clearTimeout(timer);ws.removeEventListener('message',listener);resolve(frame);};ws.addEventListener('message',listener);
    });ws.send(JSON.stringify({type:'message',channelId:channel.channel_id,content,clientRequestId}));return pending;
  }};
}

test.describe.serial('real history, rate and storage boundaries',()=>{
  test.beforeAll(async({browser})=>{
    founder=await newActor(browser);actors.push(founder);await expect(founder.page.locator('.admin-menu-btn'),'Fresh dedicated server is required').toBeVisible();
    member=await newActor(browser);actors.push(member);
    const name=`boundary-${Date.now()}`;
    const response=await founder.context.request.post(`${baseURL}/api/channels`,{headers:founder.headers,data:{channel_id:name,name,channel_type:'Text',federation_scope:'Local'}});
    expect(response.status()).toBe(200);channel=(await read<Channel[]>(founder,'/api/channels')).find(c=>c.channel_id===name)!;
    await select(founder,name);await select(member,name);
  });
  test.afterAll(async()=>{for(const actor of actors)await actor.context.close();});

  test('more than five thousand actual messages produce honest partial search results and absence boundaries',async () => {
    const info = test.info();
    const priorHeaders={...founder.headers};
    const priorProof=JSON.parse(Buffer.from(priorHeaders['x-annex-zk-proof'],'base64').toString('utf8')) as {root_hex:string};
    const heldDraft=`held-during-real-proof-retirement-${Date.now()}`;
    await founder.page.getByPlaceholder('Type a message...').fill(heldDraft);
    const output=await exec(process.execPath,[script('history-populator'),baseURL,channel.channel_id,'5005'],{timeout:600_000,maxBuffer:1024*1024});
    const receipt=JSON.parse(output.stdout.trim());expect(receipt.type).toBe('complete');expect(receipt.uniquePersistedMessageIds).toBe(5005);
    const currentResponse=await founder.context.request.get(`${baseURL}/api/registry/current-root`);expect(currentResponse.status()).toBe(200);
    const current=await currentResponse.json() as {rootHex:string};expect(current.rootHex).not.toBe(priorProof.root_hex);
    const old=await searchResult(founder,receipt.oldestText);expect(old.body.complete).toBe(false);expect(old.body.results).toEqual([]);expect(old.body.scanned_per_channel).toBeGreaterThan(0);expect(old.body.scanned_per_channel).toBeLessThan(5005);
    expect(old.refusals,'The real long-session trigger must exercise the retired-root recovery').toEqual([{status:403,error:'zk_root_retired'}]);
    const renewed=JSON.parse(Buffer.from(founder.headers['x-annex-zk-proof'],'base64').toString('utf8')) as {root_hex:string};
    expect(renewed.root_hex).toBe(current.rootHex);expect(founder.headers.authorization).toBe(priorHeaders.authorization);
    await expect(founder.page.getByPlaceholder('Type a message...')).toHaveValue(heldDraft);
    await expect(old.search.locator('.search-no-results')).toContainText('Older messages were not searched');await expect(old.search.locator('.search-no-results')).not.toHaveText('No messages found');
    const recent=await searchResult(founder,receipt.recentText);expect(recent.body.complete).toBe(false);expect(recent.body.results.some(m=>m.content===receipt.recentText)).toBe(true);
    await expect(recent.search.locator('.search-coverage-note')).toContainText('Anything older was not searched');
    await recent.search.getByRole('option').filter({hasText:receipt.recentText}).click();await expect(founder.page.getByRole('search')).toHaveCount(0);
    await expect(founder.page.locator('.message-content').filter({hasText:receipt.recentText})).toBeVisible();
    await founder.page.getByPlaceholder('Type a message...').fill('');
    await info.attach('real-search-coverage',{body:Buffer.from(JSON.stringify({...receipt,scannedPerChannel:recent.body.scanned_per_channel,
      proofRenewal:{typedRefusals:old.refusals,retiredRoot:priorProof.root_hex,currentRoot:renewed.root_hex,sessionTokenUnchanged:true,draftPreserved:true}})),contentType:'application/json'});
  });

  test('configured real429 preserves an error and Retry succeeds after the actual advertised interval',async () => {
    const info = test.info();
    const original=await read<ServerPolicy>(founder,'/api/admin/policy');const marker=`rate-marker-${Date.now()}`;await send(member,channel.name,marker);
    const policy={...original,rate_limit:{...original.rate_limit,default_limit:64}};
    const updated=await founder.context.request.put(`${baseURL}/api/admin/policy`,{headers:founder.headers,data:policy});expect(updated.status()).toBe(200);
    let limited:APIResponse|undefined;let calls=0;
    try{
      for(;calls<100;calls++){const response=await member.context.request.get(`${baseURL}/api/channels`,{headers:member.headers});if(response.status()===429){limited=response;break;}expect(response.status()).toBe(200);}
      expect(limited,'Actual requests must exhaust the configured bucket').toBeDefined();const retryAfter=Number(limited!.headers()['retry-after']);expect(retryAfter).toBeGreaterThan(0);expect(retryAfter).toBeLessThanOrEqual(120);
      const channelRow=member.page.locator('.channel-item').filter({has:member.page.locator('.channel-name',{hasText:channel.name})});
      const refusedLeave=member.page.waitForResponse(r=>r.status()===429&&new URL(r.url()).pathname.startsWith(`/api/channels/${channel.channel_id}/`));
      await channelRow.locator('.leave-btn').click();expect((await refusedLeave).status()).toBe(429);
      await expect(channelRow.locator('.channel-action-error')).toContainText(`Rate limit exceeded. Try again in ${retryAfter} seconds.`);
      const search=await openSearch(member,marker);const blocked=member.page.waitForResponse(r=>new URL(r.url()).pathname==='/api/messages/search');await search.getByRole('button',{name:'Search',exact:true}).click();expect((await blocked).status()).toBe(429);
      await expect(search.locator('.search-error')).toContainText('Search failed');await expect(search.locator('.search-no-results')).toHaveCount(0);
      // Real time, no clock override. Five seconds permits sliding-window decay.
      await new Promise(resolve=>setTimeout(resolve,(retryAfter+5)*1000));
      const recovered=member.page.waitForResponse(r=>new URL(r.url()).pathname==='/api/messages/search');await search.getByRole('button',{name:'Retry',exact:true}).click();expect((await recovered).status()).toBe(200);await expect(search.getByRole('option').filter({hasText:marker})).toBeVisible();
      await channelRow.getByRole('button',{name:'Dismiss',exact:true}).click();await expect(channelRow.locator('.channel-action-error')).toHaveCount(0);
      await info.attach('real-rate-recovery',{body:Buffer.from(JSON.stringify({calls,retryAfter,actualElapsedWaitSeconds:retryAfter+5})),contentType:'application/json'});
    }finally{
      const restored=await founder.context.request.put(`${baseURL}/api/admin/policy`,{headers:founder.headers,data:original});expect(restored.status(),'Restore actual operator policy').toBe(200);
    }
  });

  test('actual WAL growth trips507, offline clear/read failures remain visible, maintenance and Clear restore persisted writes',async({browser},info)=>{
    // Explicit opt-in is required because this case restarts its disposable
    // captured server and changes only that session configuration temporarily.
    expect(session,'Set ANNEX_BOUNDARY_SESSION to the matching dedicated session JSON').toBeTruthy();
    let prepared=false;let transport:Awaited<ReturnType<typeof dataSocket>>|undefined;
    try{
      prepared=true;const preparedState=await storage('prepare');expect(preparedState.cap).toBeGreaterThan(preparedState.bytes);
      await founder.page.reload();await expect(founder.page.getByRole('button',{name:'Chat',exact:true})).toBeVisible({timeout:150_000});
      const writer=await newActor(browser);actors.push(writer);await select(writer,channel.name);transport=await dataSocket(writer);
      let size=preparedState;let writes=0;
      for(;writes<500 && size.bytes<(size.cap!+65536);writes++){
        const frame=await transport.publish(`real-storage-pressure-${writes}-${'x'.repeat(1024)}`);expect(frame.type).toBe('message');
        if(writes%10===9)size=await storage('size');
      }
      expect(size.bytes,'Genuine writes must exceed the configured cap before waiting for the real probe').toBeGreaterThan(size.cap!);
      await expect.poll(async()=>(await read<StorageHealth>(founder,'/api/admin/storage')).writes_blocked,{timeout:75_000,intervals:[1000]}).toBe(true);
      const refused=await founder.context.request.put(`${baseURL}/api/admin/policy`,{headers:founder.headers,data:{}});expect(refused.status()).toBe(507);
      await settings();await expect(founder.page.locator('.storage-gate-state')).toContainText('degraded');
      await founder.context.setOffline(true);
      await founder.page.getByRole('button',{name:'Clear storage gate',exact:true}).click();await expect(founder.page.getByRole('alert').filter({hasText:'Could not clear the storage gate'})).toBeVisible();
      await founder.page.getByRole('button',{name:'Chat',exact:true}).click();await settings();await expect(founder.page.getByRole('alert').filter({hasText:'Could not read storage health'})).toBeVisible();await expect(founder.page.locator('.storage-gate-healthy')).toHaveCount(0);
      await founder.context.setOffline(false);
      const maintained=await storage('checkpoint');expect(maintained.bytes,'Real WAL checkpoint must remove the underlying cap pressure').toBeLessThan(maintained.cap!-2048);
      await founder.page.getByRole('button',{name:'Chat',exact:true}).click();await settings();await expect(founder.page.locator('.storage-gate-state')).toContainText('degraded');
      const cleared=founder.page.waitForResponse(r=>r.request().method()==='POST'&&new URL(r.url()).pathname==='/api/admin/storage/clear');await founder.page.getByRole('button',{name:'Clear storage gate',exact:true}).click();expect((await cleared).status()).toBe(200);await expect(founder.page.getByRole('status').filter({hasText:'Storage gate cleared'})).toBeVisible();
      const marker=`storage-recovered-${Date.now()}`;await send(founder,channel.name,marker);
      await info.attach('real-storage-recovery',{body:Buffer.from(JSON.stringify({writes,cap:preparedState.cap,overCapBytes:size.bytes,afterMaintenanceBytes:maintained.bytes,recoveredMessage:marker})),contentType:'application/json'});
    }finally{transport?.ws.close();await founder.context.setOffline(false);if(prepared)await storage('restore');}
  });
});
