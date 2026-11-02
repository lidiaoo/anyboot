const { spawn } = require('child_process'); const http = require('http');
const WebSocket = require('/tmp/ab-measure/node_modules/ws');
const wait = (ms) => new Promise(r => setTimeout(r, ms));
const getJson = (u) => new Promise((res, rej) => http.get(u, r => { let d=''; r.on('data',c=>d+=c); r.on('end',()=>{try{res(JSON.parse(d))}catch(e){rej(e)}}); }).on('error', rej));
(async () => {
  const chrome = spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', ['--headless=new','--disable-gpu','--no-sandbox','--remote-debugging-port=9653','--user-data-dir=/tmp/p4-'+Date.now(),'about:blank'], { stdio: 'ignore' });
  let t=null;
  for(let i=0;i<60;i++){ await wait(300); try{const j=await getJson('http://127.0.0.1:9653/json'); t=j.find(x=>x.type==='page'); if(t)break; }catch(e){} }
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
  await ev(`(function(){document.querySelector('[data-page="gateway"]')?.click();return 1})()`);
  await wait(1200);
  const ids = ['gwWarnQuick','gwModeTabs','gwUninstallBtn','gwLogBtn','gwRefreshBtn','gwToggleWrap','gwToggleSw',
    'gwPulse','gwStateText','gwUrl','gwCopyUrl','gwPairBtn','gwMeta','gwErrRow','gwOrphanBox','gwOrphanList',
    'gwNamedForm','gwNamedToken','gwNamedUrl','gwAutostart','gwBinaryText','gwDownloadBtn','gwManualBtn',
    'gwManualRow','gwManualPath','gwManualCancel','gwManualOk','gwProgressRow','gwProgressBar','gwProgressText',
    'gwLogoutRow','gwLogoutBtn'];
  let found = 0, missing = [];
  for (const i of ids) {
    const has = await ev(`!!document.getElementById('${i}')`);
    if (has) found++; else missing.push(i);
  }
  console.log(`cf 分页 id: ${found}/${ids.length}`);
  if (missing.length) console.log('缺:', missing.join(' '));
  console.log('① 四个分页 tab:', await ev(`document.querySelectorAll('.gw-tab').length`));
  console.log('② 模式切换 2 项:', await ev(`document.querySelectorAll('#gwModeTabs button[data-mode]').length`));
  console.log('③ 通道开关:', await ev(`!!document.getElementById('gwToggleSw')`));
  console.log('④ 自启可勾:', await ev(`(function(){var e=document.getElementById('gwAutostart'); return e?(e.disabled?'禁用':'可勾'):'无'})()`));
  console.log('⑤ 切「命名隧道」显示配置区:', await ev(`(function(){var b=document.querySelector('#gwModeTabs [data-mode="named"]'); if(b)b.click(); return 1})()`));
  await wait(700);
  console.log('   命名表单可见:', await ev(`(function(){var f=document.getElementById('gwNamedForm'); return f?!f.hidden:'无'})()`));
  console.log('   快速隧道提示隐藏:', await ev(`(function(){var w=document.getElementById('gwWarnQuick'); return w?w.hidden:'无'})()`));
  console.log('异常:', errs.slice(0,2).join(' | ') || '(无)');
  ws.close(); chrome.kill(); process.exit(0);
})();
