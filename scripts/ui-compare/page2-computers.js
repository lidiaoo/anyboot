const { spawn } = require('child_process'); const http = require('http');
const WebSocket = require('/tmp/ab-measure/node_modules/ws');
const wait = (ms) => new Promise(r => setTimeout(r, ms));
const getJson = (u) => new Promise((res, rej) => http.get(u, r => { let d=''; r.on('data',c=>d+=c); r.on('end',()=>{try{res(JSON.parse(d))}catch(e){rej(e)}}); }).on('error', rej));
(async () => {
  const chrome = spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', ['--headless=new','--disable-gpu','--no-sandbox','--remote-debugging-port=9657','--user-data-dir=/tmp/p2-'+Date.now(),'about:blank'], { stdio: 'ignore' });
  let t=null;
  for(let i=0;i<60;i++){ await wait(300); try{const j=await getJson('http://127.0.0.1:9657/json'); t=j.find(x=>x.type==='page'); if(t)break; }catch(e){} }
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
  await ev(`(function(){document.querySelector('[data-page="computers"]')?.click();return 1})()`);
  await wait(1200);
  console.log('① 页头标题:', await ev(`(document.querySelector('#page-computers h1')||{}).textContent`));
  console.log('② 三个统计:', await ev(`['cqStOnline','cqStOffline','cqStPending'].filter(function(i){return document.getElementById(i)}).length`));
  console.log('③ 四个筛选计数:', await ev(`['cqNAll','cqNOnline','cqNOffline','cqNPending'].filter(function(i){return document.getElementById(i)}).length`));
  await ev(`document.getElementById('cqSort')?.click()`);
  await wait(600);
  console.log('④ 点排序开菜单:', await ev(`(function(){return document.getElementById('cqSortMenu')?'已开':'未开'})()`));
  console.log('⑤ 菜单 3 项:', await ev(`document.querySelectorAll('#cqSortMenu [data-order]').length`));
  console.log('⑥ 当前自定义选中:', await ev(`(function(){var b=document.querySelector('#cqSortMenu [data-order="custom"]'); return b?b.classList.contains('on'):'无'})()`));
  // 点「名称」
  await ev(`document.querySelector('#cqSortMenu [data-order="name"]')?.click()`);
  await wait(500);
  console.log('⑦ 切名称后文案:', await ev(`(document.getElementById('cqSortText')||{}).textContent`));
  console.log('⑧ 持久化 key:', await ev(`localStorage.getItem('anyboot.orderMode')`));
  // 操作说明
  await ev(`document.getElementById('cqHelpToggle')?.click()`);
  await wait(400);
  console.log('⑨ 操作说明展开:', await ev(`(function(){var h=document.getElementById('cqHelp'); return h?getComputedStyle(h).display:'无'})()`));
  console.log('   说明条目数:', await ev(`document.querySelectorAll('#cqHelp li').length`));
  // 空态（搜不存在）
  await ev(`(function(){var q=document.getElementById('cqQ'); if(q){q.value='zzz'; q.dispatchEvent(new Event('input',{bubbles:true}))} return 1})()`);
  await wait(700);
  console.log('⑩ 空态出现:', await ev(`!!document.getElementById('cqEmpty')`));
  console.log('   空态文案:', await ev(`(document.getElementById('cqEmptyHint')||{}).textContent`));
  console.log('异常:', errs.slice(0,2).join(' | ') || '(无)');
  ws.close(); chrome.kill(); process.exit(0);
})();
