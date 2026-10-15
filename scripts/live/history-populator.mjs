// Genuine author registration, proof and acknowledged WS messages; no SQL writes.
import assert from 'node:assert/strict';
import { register, joinChannel, socket } from './protocol.mjs';
const [url, channelId, countText = '5005'] = process.argv.slice(2);
const count=Number(countText); assert(url && channelId && Number.isSafeInteger(count) && count>=5001 && count<=6000);
try {
  const author=await register(url); await joinChannel(author,channelId); const connection=await socket(author);
  connection.send({type:'subscribe',channelId});
  const nonce=`history-${Date.now()}`; const oldestText=`${nonce}-oldest-marker`; const recentText=`${nonce}-recent-marker`;
  const ids=new Set();
  for(let n=0;n<count;n++) {
    const started=Date.now();
    const content=n===0?oldestText:n===count-1?recentText:`${nonce}-entry-${n}`;
    const clientRequestId=`${nonce}-${n}`;
    const pending=new Promise((resolve,reject)=>{
      const timer=setTimeout(()=>{connection.ws.removeEventListener('message',listener);reject(new Error('Actual history write acknowledgment timed out'));},30000);
      const listener=event=>{const f=JSON.parse(String(event.data));if(f.clientRequestId!==clientRequestId)return;clearTimeout(timer);connection.ws.removeEventListener('message',listener);resolve(f);};
      connection.ws.addEventListener('message',listener);
    });
    connection.send({type:'message',channelId,content,clientRequestId});
    const frame=await pending;
    assert.equal(frame.type,'message'); assert.equal(frame.content,content); assert(frame.messageId); ids.add(frame.messageId);
    // Bound retained test observations without changing the production socket.
    connection.frames.splice(0,connection.frames.length);
    // Honor the actual server's sustained10/s command budget; do not
    // evade it by reconnecting or spreading work across fake identities.
    await new Promise(resolve=>setTimeout(resolve,Math.max(0,105-(Date.now()-started))));
  }
  assert.equal(ids.size,count); connection.ws.close();
  console.log(JSON.stringify({type:'complete',channelId,count,oldestText,recentText,uniquePersistedMessageIds:ids.size}));
  process.exit(0);
} catch(error) { console.error(error.message); process.exit(1); }
