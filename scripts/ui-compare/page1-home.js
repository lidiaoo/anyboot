const { spawn } = require('child_process'); const http = require('http');
const WebSocket = require('/tmp/ab-measure/node_modules/ws');
const wait = (ms) => new Promise(r => setTimeout(r, ms));
const getJson = (u) => new Promise((res, rej) => http.get(u, r => { let d=''; r.on('data',c=>d+=c); r.on('end',()=>{try{res(JSON.parse(d))}catch(e){rej(e)}}); }).on('error', rej));
(async () => {
  const chrome = spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', ['--headless=new','--disable-gpu','--no-sandbox','--remote-debugging-port=9661','--user-data-dir=/tmp/p1-'+Date.now(),'about:blank'], { stdio: 'ignore' });
  let t=null;
  for(let i=0;i<60;i++){ await wait(300); try{const j=await getJson('http://127.0.0.1:9661/json'); t=j.find(x=>x.type==='page'); if(t)break; }catch(e){} }
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
  await ev(`(function(){document.querySelector('[data-page="home"]')?.click();return 1})()`);
  await wait(1200);
  console.log('所有 .page 的 id:', await ev(`[].slice.call(document.querySelectorAll('.page')).map(function(x){return x.id}).join(',')`));
  console.log('当前 active 页签:', await ev(`(function(){var b=document.querySelector('.snav.active'); return b?b.textContent.trim():'无'})()`));
  console.log('页面 id:', await ev(`(function(){var p=document.querySelector('.page.active'); return p?p.id:'无 active 页'})()`));
  console.log('page-home 存在:', await ev(`!!document.getElementById('page-home')`));
  console.log('① pageMore 存在:', await ev(`document.querySelectorAll('[aria-label="更多"]').length >= 1`));
  console.log('② ＋添加 不再 disabled:', await ev(`(function(){var b=document.querySelector('.pc-chip.add'); return b ? (b.disabled?'仍禁用':'可点') : '无'})()`));
  console.log('③ 电源球展开:', await ev(`(function(){var b=document.querySelector('.pc-fab-ball'); if(!b)return '无球'; b.click(); return 'clicked'})()`));
  await wait(500);
  console.log('   电源 chip 数:', await ev(`document.querySelectorAll('.pc-fab-chip').length`));
  // 本机点唤醒 → 应提示「本机就在运行，无需唤醒」
  await ev(`(function(){var b=[].slice.call(document.querySelectorAll('.pc-fab-chip')).find(function(x){return /唤醒/.test(x.textContent)}); if(b)b.click(); return 1})()`);
  await wait(700);
  console.log('④ 本机唤醒提示:', await ev(`(function(){var t=document.querySelector('.toast'); return t?t.textContent.trim():'无 toast'})()`));
  // 点 ＋添加 应切到电脑列表并开弹层
  await ev(`document.querySelector('.pc-chip.add')?.click()`);
  await wait(1000);
  console.log('⑤ ＋添加 → 弹层:', await ev(`document.querySelectorAll('.overlay').length`), '（应 ≥1）');
  console.log('异常:', errs.slice(0,2).join(' | ') || '(无)');
  ws.close(); chrome.kill(); process.exit(0);
})();
