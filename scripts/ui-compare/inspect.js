const { spawn } = require('child_process');
const http = require('http');
const WebSocket = require('/tmp/ab-measure/node_modules/ws');
const wait = (ms) => new Promise(r => setTimeout(r, ms));
const getJson = (u) => new Promise((res, rej) => http.get(u, r => { let d=''; r.on('data',c=>d+=c); r.on('end',()=>{try{res(JSON.parse(d))}catch(e){rej(e)}}); }).on('error', rej));
(async () => {
  const chrome = spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', [
    '--headless=new','--disable-gpu','--no-sandbox','--remote-debugging-port=9671',
    '--user-data-dir=/tmp/cmp-i-'+Date.now(),'about:blank'], { stdio: 'ignore' });
  let t=null;
  for(let i=0;i<60;i++){ await wait(400); try{const j=await getJson('http://127.0.0.1:9671/json'); t=j.find(x=>x.type==='page'); if(t)break; }catch(e){} }
  const ws=new WebSocket(t.webSocketDebuggerUrl); let id=0; const pend=new Map();
  ws.on('message',d=>{const m=JSON.parse(d); if(m.id&&pend.has(m.id)){pend.get(m.id)(m);pend.delete(m.id);}});
  const send=(me,pa={})=>new Promise(r=>{const i=++id;pend.set(i,r);ws.send(JSON.stringify({id:i,method:me,params:pa}));});
  await new Promise(r=>ws.on('open',r));
  await send('Runtime.enable'); await send('Page.enable');
  await send('Emulation.setDeviceMetricsOverride',{width:1280,height:820,deviceScaleFactor:1,mobile:false});
  await send('Page.navigate',{url:'http://127.0.0.1:8770/new/hub.html'});
  await wait(6000);
  const ev=async(e)=>{const r=await send('Runtime.evaluate',{expression:e,returnByValue:true,awaitPromise:true});
    if(r.result&&r.result.exceptionDetails) return 'ERR:'+String(r.result.exceptionDetails.text).slice(0,100);
    return r.result&&r.result.result?r.result.result.value:null;};
  console.log(await ev(`(function(){
    var rows = document.querySelectorAll('.cq-row');
    var r0 = rows[0];
    return JSON.stringify({
      rowCount: rows.length,
      firstRowHTML: r0 ? r0.outerHTML.slice(0, 500) : '(none)',
      childClasses: r0 ? [].slice.call(r0.children).map(function(c){return c.className;}) : [],
    }, null, 1);
  })()`));
  ws.close(); chrome.kill(); process.exit(0);
})();
