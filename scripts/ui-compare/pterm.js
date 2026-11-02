const { spawn } = require('child_process'); const http = require('http');
const WebSocket = require('/tmp/ab-measure/node_modules/ws');
const wait = (ms) => new Promise(r => setTimeout(r, ms));
const getJson = (u) => new Promise((res, rej) => http.get(u, r => { let d=''; r.on('data',c=>d+=c); r.on('end',()=>{try{res(JSON.parse(d))}catch(e){rej(e)}}); }).on('error', rej));
(async () => {
  const chrome = spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', ['--headless=new','--disable-gpu','--no-sandbox','--remote-debugging-port=9635','--user-data-dir=/tmp/pt-'+Date.now(),'about:blank'], { stdio: 'ignore' });
  let t=null;
  for(let i=0;i<60;i++){ await wait(300); try{const j=await getJson('http://127.0.0.1:9635/json'); t=j.find(x=>x.type==='page'); if(t)break; }catch(e){} }
  const ws=new WebSocket(t.webSocketDebuggerUrl); let id=0; const pend=new Map(); const errs=[];
  ws.on('message',d=>{const m=JSON.parse(d);
    if(m.method==='Runtime.exceptionThrown'){const e=m.params.exceptionDetails; errs.push(String((e.exception&&e.exception.description)||e.text).slice(0,120));}
    if(m.id&&pend.has(m.id)){pend.get(m.id)(m);pend.delete(m.id);}});
  const send=(me,pa={})=>new Promise(r=>{const i=++id;pend.set(i,r);ws.send(JSON.stringify({id:i,method:me,params:pa}));});
  await new Promise(r=>ws.on('open',r));
  await send('Runtime.enable'); await send('Page.enable'); await send('Network.enable'); await send('Network.setCacheDisabled',{cacheDisabled:true});
  await send('Emulation.setDeviceMetricsOverride',{width:900,height:700,deviceScaleFactor:1,mobile:false});
  await send('Page.navigate',{url:'http://127.0.0.1:8770/new/hub-term.html?t='+Date.now()});
  await wait(4500);
  const ev=async(e)=>{const r=await send('Runtime.evaluate',{expression:e,returnByValue:true,awaitPromise:true}); return r.result&&r.result.result?r.result.result.value:null;};
  await ev(`window.alert=function(){};window.confirm=function(){return false};window.prompt=function(){return null};`);
  const ids = ['title','connPill','connText','hubBtn','themeBtn','fontBtn','tabbar','termHost','empty','footDot','footHost','footSize','footLat','pick','pickTitle','pickClose','pickList','toast'];
  let f=0, miss=[];
  for (const i of ids) { if (await ev(`!!document.getElementById('${i}')`)) f++; else miss.push(i); }
  console.log(`终端窗 id: ${f}/${ids.length}`, miss.length?('缺 '+miss.join(' ')):'✅');
  console.log('  empty 显示:', await ev(`(function(){var e=document.getElementById('empty'); return e?!e.hidden:'无'})()`));
  // 点「＋」打开 pick 弹层
  await ev(`(function(){var b=document.querySelector('.tabbar-add'); if(b)b.click(); return 1})()`);
  await wait(600);
  console.log('  pick 弹层:', await ev(`!!document.getElementById('pick')`));
  console.log('  异常:', errs.slice(0,2).join(' | ') || '(无)');
  ws.close(); chrome.kill(); process.exit(0);
})();
