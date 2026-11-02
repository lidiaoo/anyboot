const { spawn } = require('child_process'); const http = require('http');
const WebSocket = require('/tmp/ab-measure/node_modules/ws');
const wait = (ms) => new Promise(r => setTimeout(r, ms));
const getJson = (u) => new Promise((res, rej) => http.get(u, r => { let d=''; r.on('data',c=>d+=c); r.on('end',()=>{try{res(JSON.parse(d))}catch(e){rej(e)}}); }).on('error', rej));
(async () => {
  const page = process.argv[2] || 'hub-keys.html';
  const chrome = spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', ['--headless=new','--disable-gpu','--no-sandbox','--remote-debugging-port=9637','--user-data-dir=/tmp/p12-'+Date.now(),'about:blank'], { stdio: 'ignore' });
  let t=null;
  for(let i=0;i<60;i++){ await wait(300); try{const j=await getJson('http://127.0.0.1:9637/json'); t=j.find(x=>x.type==='page'); if(t)break; }catch(e){} }
  const ws=new WebSocket(t.webSocketDebuggerUrl); let id=0; const pend=new Map(); const errs=[];
  ws.on('message',d=>{const m=JSON.parse(d);
    if(m.method==='Runtime.exceptionThrown'){const e=m.params.exceptionDetails; errs.push(String((e.exception&&e.exception.description)||e.text).slice(0,120));}
    if(m.id&&pend.has(m.id)){pend.get(m.id)(m);pend.delete(m.id);}});
  const send=(me,pa={})=>new Promise(r=>{const i=++id;pend.set(i,r);ws.send(JSON.stringify({id:i,method:me,params:pa}));});
  await new Promise(r=>ws.on('open',r));
  await send('Runtime.enable'); await send('Page.enable'); await send('Network.enable'); await send('Network.setCacheDisabled',{cacheDisabled:true});
  await send('Emulation.setDeviceMetricsOverride',{width:900,height:700,deviceScaleFactor:1,mobile:false});
  await send('Page.navigate',{url:`http://127.0.0.1:8770/new/${page}?t=`+Date.now()});
  await wait(4500);
  const ev=async(e)=>{const r=await send('Runtime.evaluate',{expression:e,returnByValue:true,awaitPromise:true}); return r.result&&r.result.result?r.result.result.value:null;};
  await ev(`window.alert=function(){};window.confirm=function(){return false};window.prompt=function(){return null};`);
  console.log(`=== ${page}`);
  console.log('  密钥卡片数:', await ev(`document.querySelectorAll('.key-card').length`));
  // 打开生成弹层
  await ev(`document.getElementById('genBtn')?.click()`); await wait(600);
  console.log('  生成弹层出现:', await ev(`!!document.getElementById('genModal')`));
  console.log('  类型 2 个:', await ev(`document.querySelectorAll('#genKind button').length`));
  // 打开删除弹层（需要先有关键行；mock 无密钥时跳过）
  const ids = ['count','hubBtn','themeBtn','genBtn','importBtn','list','emptyTip','genModal','genTitle','genClose','genForm','genKind','genName','genPass','genCancel'];
  let f=0, miss=[];
  for (const i of ids) { if (await ev(`!!document.getElementById('${i}')`)) f++; else miss.push(i); }
  console.log(`静态+生成弹层 id: ${f}/${ids.length}`, miss.length?('缺 '+miss.join(' ')):'✅');
  console.log('异常:', errs.slice(0,2).join(' | ') || '(无)');
  ws.close(); chrome.kill(); process.exit(0);
})();
