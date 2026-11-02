const { spawn } = require('child_process'); const http = require('http');
const WebSocket = require('/tmp/ab-measure/node_modules/ws');
const wait = (ms) => new Promise(r => setTimeout(r, ms));
const getJson = (u) => new Promise((res, rej) => http.get(u, r => { let d=''; r.on('data',c=>d+=c); r.on('end',()=>{try{res(JSON.parse(d))}catch(e){rej(e)}}); }).on('error', rej));
(async () => {
  const chrome = spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', ['--headless=new','--disable-gpu','--no-sandbox','--remote-debugging-port=9669','--user-data-dir=/tmp/bk3-'+Date.now(),'about:blank'], { stdio: 'ignore' });
  let t=null;
  for(let i=0;i<60;i++){ await wait(300); try{const j=await getJson('http://127.0.0.1:9669/json'); t=j.find(x=>x.type==='page'); if(t)break; }catch(e){} }
  const ws=new WebSocket(t.webSocketDebuggerUrl); let id=0; const pend=new Map();
  ws.on('message',d=>{const m=JSON.parse(d); if(m.id&&pend.has(m.id)){pend.get(m.id)(m);pend.delete(m.id);}});
  const send=(me,pa={})=>new Promise(r=>{const i=++id;pend.set(i,r);ws.send(JSON.stringify({id:i,method:me,params:pa}));});
  await new Promise(r=>ws.on('open',r));
  await send('Runtime.enable'); await send('Page.enable'); await send('Network.enable'); await send('Network.setCacheDisabled',{cacheDisabled:true});
  await send('Emulation.setDeviceMetricsOverride',{width:1280,height:860,deviceScaleFactor:1,mobile:false});
  await send('Page.navigate',{url:'http://127.0.0.1:8770/new/hub.html?t='+Date.now()});
  await wait(4500);
  const ev=async(e)=>{const r=await send('Runtime.evaluate',{expression:e,returnByValue:true,awaitPromise:true}); return r.result&&r.result.result?r.result.result.value:null;};
  await ev(`(function(){document.querySelector('[data-page="backup"]')?.click();return 1})()`);
  await wait(1200);
  console.log('【初始：keys 默认勾选】');
  console.log('  分类:', await ev(`[].slice.call(document.querySelectorAll('#exportCats .bk-cname')).map(function(x){return x.textContent}).join('/')`));
  console.log('  勾选数:', await ev(`document.querySelectorAll('#exportCats input:checked').length`), '/ 5');
  console.log('  口令区存在:', await ev(`!!document.getElementById('exportPwZone')`));
  console.log('  私钥提示存在:', await ev(`!!document.getElementById('exportKeyHint')`));

  console.log('【输入短口令 ab】');
  await ev(`(function(){var p=document.getElementById('expPw1'); if(p){p.value='ab'; p.dispatchEvent(new Event('input',{bubbles:true}))} return 1})()`);
  await wait(600);
  console.log('  错误文案:', await ev(`(document.getElementById('expPwErr')||{}).textContent`));
  console.log('  强度条:', await ev(`(function(){var e=document.getElementById('expStrength'); return e?e.style.width+' / '+e.style.background:'无'})()`));

  console.log('【输入强口令】');
  await ev(`(function(){var p=document.getElementById('expPw1'); if(p){p.value='Abc123!@#'; p.dispatchEvent(new Event('input',{bubbles:true}))} var q=document.getElementById('expPw2'); if(q){q.value='Abc123!@#'; q.dispatchEvent(new Event('input',{bubbles:true}))} return 1})()`);
  await wait(600);
  console.log('  错误文案:', await ev(`(document.getElementById('expPwErr')||{}).textContent || '(空)'`));
  console.log('  强度条:', await ev(`(function(){var e=document.getElementById('expStrength'); return e?e.style.width+' / '+e.style.background:'无'})()`));

  console.log('【两次不一致】');
  await ev(`(function(){var q=document.getElementById('expPw2'); if(q){q.value='xxx'; q.dispatchEvent(new Event('input',{bubbles:true}))} return 1})()`);
  await wait(600);
  console.log('  错误文案:', await ev(`(document.getElementById('expPwErr')||{}).textContent`));
  ws.close(); chrome.kill(); process.exit(0);
})();
