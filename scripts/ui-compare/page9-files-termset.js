const { spawn } = require('child_process'); const http = require('http');
const WebSocket = require('/tmp/ab-measure/node_modules/ws');
const wait = (ms) => new Promise(r => setTimeout(r, ms));
const getJson = (u) => new Promise((res, rej) => http.get(u, r => { let d=''; r.on('data',c=>d+=c); r.on('end',()=>{try{res(JSON.parse(d))}catch(e){rej(e)}}); }).on('error', rej));
(async () => {
  const chrome = spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', ['--headless=new','--disable-gpu','--no-sandbox','--remote-debugging-port=9643','--user-data-dir=/tmp/p9-'+Date.now(),'about:blank'], { stdio: 'ignore' });
  let t=null;
  for(let i=0;i<60;i++){ await wait(300); try{const j=await getJson('http://127.0.0.1:9643/json'); t=j.find(x=>x.type==='page'); if(t)break; }catch(e){} }
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

  // 文件页
  await ev(`(function(){document.querySelector('[data-page="files"]')?.click();return 1})()`); await wait(900);
  const fbIds = ['fbServer','fbPill','fbPillText','fbOpenWin','fbEmpty','fbMain','fbUp','fbRefresh','fbCrumbs','fbPathDrop',
    'fbMkdir','fbUpload','fbDownload','fbTableWrap','fbTbody','fbEmptyTip','fbTransfer','fbTransferDir','fbTransferName',
    'fbTransferBar','fbTransferFill','fbTransferMeta','fbTransferPct','fbTransferAct','fbTransferGiveup','fbCount'];
  let f=0, miss=[];
  for (const i of fbIds) { if (await ev(`!!document.getElementById('${i}')`)) f++; else miss.push(i); }
  console.log(`文件页 id: ${f}/${fbIds.length}`, miss.length?('缺 '+miss.join(' ')):'✅');

  // 有传输任务时检查 fbTransfer
  const xferIds=['fbTransfer','fbTransferDir','fbTransferName','fbTransferBar','fbTransferFill','fbTransferMeta','fbTransferPct','fbTransferAct','fbTransferGiveup'];
  let xf=0, xm=[];
  for (const i of xferIds) { if (await ev(`!!document.getElementById('${i}')`)) xf++; else xm.push(i); }
  console.log(`传输条 id: ${xf}/${xferIds.length}`, xm.length?('缺 '+xm.join(' ')):'✅');

  // 终端设置页
  await ev(`(function(){document.querySelector('[data-page="termset"]')?.click();return 1})()`); await wait(900);
  const tsIds = ['tsFont','tsFontVal','tsCursor','tsCopy','tsBell','tsFontFamily','tsScroll','tsDownDir','tsDownBrowse','tsApply'];
  let tf=0, tmiss=[];
  for (const i of tsIds) { if (await ev(`!!document.getElementById('${i}')`)) tf++; else tmiss.push(i); }
  console.log(`终端设置 id: ${tf}/${tsIds.length}`, tmiss.length?('缺 '+tmiss.join(' ')):'✅');
  console.log('  字号默认:', await ev(`(document.getElementById('tsFontVal')||{}).textContent`));
  console.log('  回滚默认:', await ev(`(document.getElementById('tsScroll')||{}).value`));
  console.log('  光标闪烁默认勾选:', await ev(`(document.getElementById('tsCursor')||{}).checked`));
  // 应用设置 → 检查存储 key 和字段
  await ev(`document.getElementById('tsApply')?.click()`); await wait(600);
  console.log('  存储 key=ab-term-settings:', await ev(`(()=>{try{return JSON.stringify(JSON.parse(localStorage.getItem('ab-term-settings')))}catch(e){return '无'}})()`));
  console.log('异常:', errs.slice(0,2).join(' | ') || '(无)');
  ws.close(); chrome.kill(); process.exit(0);
})();
