const { spawn } = require('child_process'); const http = require('http');
const WebSocket = require('/tmp/ab-measure/node_modules/ws');
const wait = (ms) => new Promise(r => setTimeout(r, ms));
const getJson = (u) => new Promise((res, rej) => http.get(u, r => { let d=''; r.on('data',c=>d+=c); r.on('end',()=>{try{res(JSON.parse(d))}catch(e){rej(e)}}); }).on('error', rej));
(async () => {
  const chrome = spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', ['--headless=new','--disable-gpu','--no-sandbox','--remote-debugging-port=9651','--user-data-dir=/tmp/p5-'+Date.now(),'about:blank'], { stdio: 'ignore' });
  let t=null;
  for(let i=0;i<60;i++){ await wait(300); try{const j=await getJson('http://127.0.0.1:9651/json'); t=j.find(x=>x.type==='page'); if(t)break; }catch(e){} }
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
  await ev(`(function(){var b=[].slice.call(document.querySelectorAll('.gw-tab')).find(function(x){return /^Tailscale$/.test(x.textContent.trim())}); if(b)b.click(); return 1})()`);
  await wait(1000);
  const ids = ['tsCard','tsBadge','tsMainSwGroup','tsMainSwWrap','tsMainSw','tsSvcSwGroup','tsSvcSwWrap','tsSvcSw',
    'tsUninstallBtn','tsLogBtn','tsRefreshBtn','tsStateRow','tsPulse','tsStateText','tsMeta',
    'tsDlRow','tsDlHint','tsDlText','tsDownloadBtn','tsManualLink','tsProgressRow','tsProgressBar','tsProgressText',
    'tsKeyRow','tsKey','tsKeySave','tsKeyHint','tsLoginRow','tsLoginOpen','tsLoginCopy','tsLoginRegen','tsLoginUrl',
    'tsAddrRow','tsAddr','tsCopyAddr','tsPairBtn','tsHealthDot','tsHealthText','tsStoppedRow','tsGuiRow','tsLaunchGuiBtn',
    'tsRebootRow','tsRebootRecheck','tsApprovalRow','tsApprovalBtn','tsfBlock','tsfBadge','tsfSwWrap','tsfSw',
    'tsfStateRow','tsfPulse','tsfStateText','tsfOffRow','tsfAddrRow','tsfAddr','tsfCopy','tsfPairBtn',
    'tsfAuthRow','tsfAuthBtn','tsfRetryBtn','tsfErrRow','tsfErrText','tsLogoutRow','tsLogoutBtn','tsAutostart'];
  let found=0, missing=[];
  for (const i of ids) { const ok = await ev(`!!document.getElementById('${i}')`); if(ok) found++; else missing.push(i); }
  console.log(`ts 分页 id: ${found}/${ids.length}`);
  if (missing.length) console.log('缺:', missing.join(' '));
  console.log('① tsCard 存在:', await ev(`!!document.getElementById('tsCard')`));
  console.log('② 状态文案:', await ev(`(document.getElementById('tsStateText')||{}).textContent`));
  console.log('③ 未安装时 tsDlRow 可见:', await ev(`(function(){var e=document.getElementById('tsDlRow'); return e?!e.hidden:'无'})()`));
  console.log('④ Funnel 区块隐藏(未安装):', await ev(`(function(){var e=document.getElementById('tsfBlock'); return e?e.hidden:'无'})()`));
  console.log('⑤ 自启复选框:', await ev(`!!document.getElementById('tsAutostart')`));
  console.log('异常:', errs.slice(0,2).join(' | ') || '(无)');
  ws.close(); chrome.kill(); process.exit(0);
})();
