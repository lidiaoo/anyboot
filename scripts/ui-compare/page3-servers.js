const { spawn } = require('child_process'); const http = require('http');
const WebSocket = require('/tmp/ab-measure/node_modules/ws');
const wait = (ms) => new Promise(r => setTimeout(r, ms));
const getJson = (u) => new Promise((res, rej) => http.get(u, r => { let d=''; r.on('data',c=>d+=c); r.on('end',()=>{try{res(JSON.parse(d))}catch(e){rej(e)}}); }).on('error', rej));
(async () => {
  const chrome = spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', ['--headless=new','--disable-gpu','--no-sandbox','--remote-debugging-port=9655','--user-data-dir=/tmp/p3-'+Date.now(),'about:blank'], { stdio: 'ignore' });
  let t=null;
  for(let i=0;i<60;i++){ await wait(300); try{const j=await getJson('http://127.0.0.1:9655/json'); t=j.find(x=>x.type==='page'); if(t)break; }catch(e){} }
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
  await ev(`(function(){document.querySelector('[data-page="servers"]')?.click();return 1})()`);
  await wait(1200);
  console.log('① 空态 placeholder:', await ev(`!!document.querySelector('#detail .placeholder')`));
  console.log('   空态文案:', await ev(`(document.querySelector('#detail .card-hint h2')||{}).textContent`));
  console.log('② 服务器列表行:', await ev(`document.querySelectorAll('.server-row').length`));
  console.log('③ 工具栏 summary:', await ev(`(document.getElementById('summary')||{}).textContent`));
  // 选一个服务器
  await ev(`document.querySelector('.server-row')?.click()`);
  await wait(900);
  console.log('④ 详情 .d-head:', await ev(`!!document.querySelector('#detail .d-head')`));
  console.log('   名称:', await ev(`(document.querySelector('#detail .d-head h3')||{}).textContent`));
  console.log('   状态胶囊:', await ev(`(function(){var p=document.querySelector('#detail .pill'); return p?p.className+' / '+p.textContent.trim():'无'})()`));
  console.log('   协议徽章:', await ev(`(document.querySelector('#detail .badge')||{}).textContent`));
  console.log('⑤ .info-grid 5 格:', await ev(`document.querySelectorAll('#detail .info-grid > div').length`));
  console.log('   各格标题:', await ev(`[].slice.call(document.querySelectorAll('#detail .info-grid .k')).map(function(x){return x.textContent}).join('/')`));
  console.log('⑥ .d-section 数:', await ev(`document.querySelectorAll('#detail .d-section').length`));
  console.log('⑦ 打开终端按钮:', await ev(`(function(){var b=[].slice.call(document.querySelectorAll('#detail .btn')).find(function(x){return /打开终端/.test(x.textContent)}); return b?'存在 disabled='+b.disabled:'无'})()`));
  console.log('⑧ 浏览文件按钮:', await ev(`(function(){var b=[].slice.call(document.querySelectorAll('#detail .btn')).find(function(x){return /浏览文件/.test(x.textContent)}); return b?'存在':'无'})()`));
  console.log('⑨ 编辑/删除图标按钮:', await ev(`document.querySelectorAll('#detail .icon-btn.boxed').length`));
  console.log('异常:', errs.slice(0,2).join(' | ') || '(无)');
  ws.close(); chrome.kill(); process.exit(0);
})();
