const { spawn } = require('child_process'); const http = require('http');
const WebSocket = require('/tmp/ab-measure/node_modules/ws');
const wait = (ms) => new Promise(r => setTimeout(r, ms));
const getJson = (u) => new Promise((res, rej) => http.get(u, r => { let d=''; r.on('data',c=>d+=c); r.on('end',()=>{try{res(JSON.parse(d))}catch(e){rej(e)}}); }).on('error', rej));
(async () => {
  const chrome = spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', ['--headless=new','--disable-gpu','--no-sandbox','--remote-debugging-port=9649','--user-data-dir=/tmp/p6-'+Date.now(),'about:blank'], { stdio: 'ignore' });
  let t=null;
  for(let i=0;i<60;i++){ await wait(300); try{const j=await getJson('http://127.0.0.1:9649/json'); t=j.find(x=>x.type==='page'); if(t)break; }catch(e){} }
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
  await ev(`(function(){document.querySelector('[data-page="gateway"]')?.click();return 1})()`);
  await wait(1000);
  // tsn 分页
  await ev(`(function(){var b=[].slice.call(document.querySelectorAll('.gw-tab')).find(function(x){return /内置/.test(x.textContent)}); if(b)b.click(); return 1})()`);
  await wait(800);
  const tsnIds = ['tsnCard','tsnBadge','tsnSwWrap','tsnSw','tsnUninstallBtn','tsnLogBtn','tsnRefreshBtn','tsnPulse','tsnStateText',
    'tsnOffRow','tsnKey','tsnKeySave','tsnAuthRow','tsnLoginBtn','tsnLoginCopy','tsnLoginRegen','tsnLoginUrl',
    'tsnAddrRow','tsnAddr','tsnCopy','tsnPairBtn','tsnErrRow','tsnErrText','tsnRetryBtn','tsnSetupBtn','tsnLogoutBtn','tsnAutostart'];
  let f=0, miss=[];
  for (const i of tsnIds) { if (await ev(`!!document.getElementById('${i}')`)) f++; else miss.push(i); }
  console.log(`tsn 分页 id: ${f}/${tsnIds.length}`, miss.length?('缺 '+miss.join(' ')):'✅');
  console.log('  状态文案:', await ev(`(document.getElementById('tsnStateText')||{}).textContent`));
  // pair 分页
  await ev(`(function(){var b=[].slice.call(document.querySelectorAll('.gw-tab')).find(function(x){return /配对管理/.test(x.textContent)}); if(b)b.click(); return 1})()`);
  await wait(800);
  console.log(`pair 分页: gwIssueBtn=${await ev(`!!document.getElementById('gwIssueBtn')`)} gwClientList=${await ev(`!!document.getElementById('gwClientList')`)}`);
  console.log('  空态文案:', await ev(`(document.querySelector('.gw-clients-empty')||{}).textContent`));
  console.log('  两张卡片:', await ev(`document.querySelectorAll('#page-gateway .gw-tabpane').length >= 4`));
  console.log('异常:', errs.slice(0,2).join(' | ') || '(无)');
  ws.close(); chrome.kill(); process.exit(0);
})();
