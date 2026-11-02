const { spawn } = require('child_process'); const http = require('http');
const WebSocket = require('/tmp/ab-measure/node_modules/ws');
const wait = (ms) => new Promise(r => setTimeout(r, ms));
const getJson = (u) => new Promise((res, rej) => http.get(u, r => { let d=''; r.on('data',c=>d+=c); r.on('end',()=>{try{res(JSON.parse(d))}catch(e){rej(e)}}); }).on('error', rej));
(async () => {
  const chrome = spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', ['--headless=new','--disable-gpu','--no-sandbox','--remote-debugging-port=9647','--user-data-dir=/tmp/p7-'+Date.now(),'about:blank'], { stdio: 'ignore' });
  let t=null;
  for(let i=0;i<60;i++){ await wait(300); try{const j=await getJson('http://127.0.0.1:9647/json'); t=j.find(x=>x.type==='page'); if(t)break; }catch(e){} }
  const ws=new WebSocket(t.webSocketDebuggerUrl); let id=0; const pend=new Map(); const errs=[];
  ws.on('message',d=>{const m=JSON.parse(d);
    if(m.method==='Runtime.exceptionThrown'){const e=m.params.exceptionDetails; errs.push(String((e.exception&&e.exception.description)||e.text).slice(0,140));}
    if(m.id&&pend.has(m.id)){pend.get(m.id)(m);pend.delete(m.id);}});
  const send=(me,pa={})=>new Promise(r=>{const i=++id;pend.set(i,r);ws.send(JSON.stringify({id:i,method:me,params:pa}));});
  await new Promise(r=>ws.on('open',r));
  await send('Runtime.enable'); await send('Page.enable'); await send('Network.enable'); await send('Network.setCacheDisabled',{cacheDisabled:true});
  await send('Emulation.setDeviceMetricsOverride',{width:1280,height:860,deviceScaleFactor:1,mobile:false});
  await send('Page.navigate',{url:'http://127.0.0.1:8770/new/hub.html?t='+Date.now()});
  await wait(4500);
  const ev=async(e)=>{const r=await send('Runtime.evaluate',{expression:e,returnByValue:true,awaitPromise:true}); return r.result&&r.result.result?r.result.result.value:null;};
  await ev(`window.alert=function(){};window.confirm=function(){return false};window.prompt=function(){return null};`);
  await ev(`(function(){document.querySelector('[data-page="gateway"]')?.click();return 1})()`);
  await wait(1000);
  console.log('① 自动刷新暂停图标:', await ev(`!!document.getElementById('gwAutoIconPause')`));
  console.log('② 播放图标:', await ev(`!!document.getElementById('gwAutoIconPlay')`));
  console.log('③ 下拉菜单(点开):', await ev(`(function(){document.getElementById('gwAutoCaret')?.click(); return 1})()`));
  await wait(500);
  console.log('   gwAutoMenu:', await ev(`!!document.getElementById('gwAutoMenu')`));
  console.log('④ 三个分页圆点:', await ev(`['gwTabDotCf','gwTabDotTs','gwTabDotTsn'].filter(function(i){return document.getElementById(i)}).length`));
  // 配对弹层
  await ev(`(function(){var b=[].slice.call(document.querySelectorAll('.gw-tab')).find(function(x){return /配对管理/.test(x.textContent)}); if(b)b.click(); return 1})()`);
  await wait(600);
  await ev(`document.getElementById('gwIssueBtn')?.click()`);
  await wait(800);
  const pairIds = ['gwPairOverlay','gwPairX','gwPairTitle','gwPairNameRow','gwPairName','gwPairAddrRow','gwPairAddrTabs','gwPairAddrHint','gwPairGoRow','gwPairGo'];
  let pf=0, pm=[];
  for (const i of pairIds) { if (await ev(`!!document.getElementById('${i}')`)) pf++; else pm.push(i); }
  console.log(`⑤ 配对弹层 id: ${pf}/${pairIds.length}`, pm.length?('缺 '+pm.join(' ')):'✅');
  // 卸载弹层（需已安装才出按钮，直接调状态不可行，改为检查结构存在性——通过 ts 卡卸载按钮）
  // 日志弹层
  await ev(`(function(){var b=[].slice.call(document.querySelectorAll('.gw-tab')).find(function(x){return /Cloudflare/.test(x.textContent)}); if(b)b.click(); return 1})()`);
  await wait(600);
  await ev(`document.getElementById('gwLogBtn')?.click()`);
  await wait(800);
  console.log('⑥ 通道日志弹层:', await ev(`!!document.getElementById('gwLogOverlay')`), 'Body:', await ev(`!!document.getElementById('gwLogBody')`), 'Close:', await ev(`!!document.getElementById('gwLogClose')`));
  console.log('异常:', errs.slice(0,2).join(' | ') || '(无)');
  ws.close(); chrome.kill(); process.exit(0);
})();
