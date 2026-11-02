const { spawn } = require('child_process'); const http = require('http');
const WebSocket = require('/tmp/ab-measure/node_modules/ws');
const wait = (ms) => new Promise(r => setTimeout(r, ms));
const getJson = (u) => new Promise((res, rej) => http.get(u, r => { let d=''; r.on('data',c=>d+=c); r.on('end',()=>{try{res(JSON.parse(d))}catch(e){rej(e)}}); }).on('error', rej));
(async () => {
  const chrome = spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', ['--headless=new','--disable-gpu','--no-sandbox','--remote-debugging-port=9631','--user-data-dir=/tmp/pb-'+Date.now(),'about:blank'], { stdio: 'ignore' });
  let t=null;
  for(let i=0;i<60;i++){ await wait(300); try{const j=await getJson('http://127.0.0.1:9631/json'); t=j.find(x=>x.type==='page'); if(t)break; }catch(e){} }
  const ws=new WebSocket(t.webSocketDebuggerUrl); let id=0; const pend=new Map(); const errs=[];
  ws.on('message',d=>{const m=JSON.parse(d);
    if(m.method==='Runtime.exceptionThrown'){const e=m.params.exceptionDetails; errs.push(String((e.exception&&e.exception.description)||e.text).slice(0,120));}
    if(m.id&&pend.has(m.id)){pend.get(m.id)(m);pend.delete(m.id);}});
  const send=(me,pa={})=>new Promise(r=>{const i=++id;pend.set(i,r);ws.send(JSON.stringify({id:i,method:me,params:pa}));});
  await new Promise(r=>ws.on('open',r));
  await send('Runtime.enable'); await send('Page.enable'); await send('Network.enable'); await send('Network.setCacheDisabled',{cacheDisabled:true});
  await send('Emulation.setDeviceMetricsOverride',{width:900,height:760,deviceScaleFactor:1,mobile:false});
  await send('Page.navigate',{url:'http://127.0.0.1:8770/new/hub-backup.html?t='+Date.now()});
  await wait(4500);
  const ev=async(e)=>{const r=await send('Runtime.evaluate',{expression:e,returnByValue:true,awaitPromise:true}); return r.result&&r.result.result?r.result.result.value:null;};
  await ev(`window.alert=function(){};window.confirm=function(){return false};window.prompt=function(){return null};`);
  const statics = ['exportPanel','exportCats','exportKeyHint','exportPwZone','expPw1','expPw2','expStrength','expPwErr','exportBtn','exportNote','importPanel','importBtn','syncPanel','syncEnabled','syncForm','syncUrl','syncDir','syncUser','syncPass','syncPeriod','syncOnChange','syncCats','syncEncryptZone','syncEncrypt','syncEncryptPwField','syncEncryptPw','syncEncryptErr','syncTestBtn','syncSaveBtn','syncNowBtn','syncStatus','syncLockInfo','hubBtn','themeBtn'];
  let f=0, miss=[];
  for (const i of statics) { if (await ev(`!!document.getElementById('${i}')`)) f++; else miss.push(i); }
  console.log(`静态 id: ${f}/${statics.length}`, miss.length?('缺 '+miss.join(' ')):'✅');
  console.log('  导出分类:', await ev(`[].slice.call(document.querySelectorAll('#exportCats .radio-line')).map(function(x){return x.textContent.trim()}).join('/')`));
  console.log('  同步默认勾选:', await ev(`document.querySelectorAll('#syncCats input:checked').length`));
  // 勾选加密
  await ev(`(function(){var e=document.getElementById('syncEncrypt'); if(e){e.checked=true; e.dispatchEvent(new Event('change',{bubbles:true}))} return 1})()`);
  await wait(400);
  console.log('  加密后口令框:', await ev(`(function(){var f=document.getElementById('syncEncryptPwField'); return f?!f.hidden:'无'})()`));
  console.log('异常:', errs.slice(0,2).join(' | ') || '(无)');
  ws.close(); chrome.kill(); process.exit(0);
})();
