const { spawn } = require('child_process'); const http = require('http');
const WebSocket = require('/tmp/ab-measure/node_modules/ws');
const wait = (ms) => new Promise(r => setTimeout(r, ms));
const getJson = (u) => new Promise((res, rej) => http.get(u, r => { let d=''; r.on('data',c=>d+=c); r.on('end',()=>{try{res(JSON.parse(d))}catch(e){rej(e)}}); }).on('error', rej));
(async () => {
  const chrome = spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', ['--headless=new','--disable-gpu','--no-sandbox','--remote-debugging-port=9645','--user-data-dir=/tmp/p8-'+Date.now(),'about:blank'], { stdio: 'ignore' });
  let t=null;
  for(let i=0;i<60;i++){ await wait(300); try{const j=await getJson('http://127.0.0.1:9645/json'); t=j.find(x=>x.type==='page'); if(t)break; }catch(e){} }
  const ws=new WebSocket(t.webSocketDebuggerUrl); let id=0; const pend=new Map(); const errs=[];
  ws.on('message',d=>{const m=JSON.parse(d);
    if(m.method==='Runtime.exceptionThrown'){const e=m.params.exceptionDetails; errs.push(String((e.exception&&e.exception.description)||e.text).slice(0,120));}
    if(m.id&&pend.has(m.id)){pend.get(m.id)(m);pend.delete(m.id);}});
  const send=(me,pa={})=>new Promise(r=>{const i=++id;pend.set(i,r);ws.send(JSON.stringify({id:i,method:me,params:pa}));});
  await new Promise(r=>ws.on('open',r));
  await send('Runtime.enable'); await send('Page.enable'); await send('Network.enable'); await send('Network.setCacheDisabled',{cacheDisabled:true});
  await send('Emulation.setDeviceMetricsOverride',{width:1280,height:860,deviceScaleFactor:1,mobile:false});
  await send('Page.navigate',{url:'http://127.0.0.1:8770/new/hub.html?t='+Date.now()});
  await wait(4500);
  const ev=async(e)=>{const r=await send('Runtime.evaluate',{expression:e,returnByValue:true,awaitPromise:true}); return r.result&&r.result.result?r.result.result.value:null;};
  await ev(`window.alert=function(){};window.confirm=function(){return false};window.prompt=function(){return null};`);

  // 终端页
  await ev(`(function(){document.querySelector('[data-page="term"]')?.click();return 1})()`); await wait(900);
  console.log('① term .act-card:', await ev(`!!document.querySelector('#page-term .act-card')`));
  console.log('   termPageHint:', await ev(`(document.getElementById('termPageHint')||{}).textContent`));
  console.log('   pageTermBtn:', await ev(`(function(){var b=document.getElementById('pageTermBtn'); return b?b.textContent.trim()+' disabled='+b.disabled:'无'})()`));

  // 会话页
  await ev(`(function(){document.querySelector('[data-page="sessions"]')?.click();return 1})()`); await wait(900);
  console.log('② sessionList:', await ev(`!!document.getElementById('sessionList')`));
  console.log('   空态:', await ev(`(function(){var a=document.querySelector('#sessionList .act-card .t'); return a?a.textContent:'无'})()`));

  // 传输页
  await ev(`(function(){document.querySelector('[data-page="transfers"]')?.click();return 1})()`); await wait(900);
  console.log('③ 4 个按钮:', await ev(`['txPauseAll','txResumeAll','txAbandonAll','txClearDone'].filter(function(i){return document.getElementById(i)}).length`));
  console.log('   txList:', await ev(`!!document.getElementById('txList')`));
  console.log('   空态:', await ev(`(function(){var a=document.querySelector('#txList .act-card .t'); return a?a.textContent:'无'})()`));

  // 密钥页
  await ev(`(function(){document.querySelector('[data-page="keys"]')?.click();return 1})()`); await wait(900);
  console.log('④ keysPageHint:', await ev(`(document.getElementById('keysPageHint')||{}).textContent`));
  console.log('   keysBtn:', await ev(`!!document.getElementById('keysBtn')`));
  console.log('异常:', errs.slice(0,2).join(' | ') || '(无)');
  ws.close(); chrome.kill(); process.exit(0);
})();
