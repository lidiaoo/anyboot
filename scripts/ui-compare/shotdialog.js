const { spawn } = require('child_process');
const http = require('http');
const fs = require('fs');
const WebSocket = require('/tmp/ab-measure/node_modules/ws');
const wait = (ms) => new Promise(r => setTimeout(r, ms));
const getJson = (u) => new Promise((res, rej) => http.get(u, r => { let d=''; r.on('data',c=>d+=c); r.on('end',()=>{try{res(JSON.parse(d))}catch(e){rej(e)}}); }).on('error', rej));
(async () => {
  const [which, out] = process.argv.slice(2);
  const chrome = spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', [
    '--headless=new','--disable-gpu','--no-sandbox','--remote-debugging-port=9687',
    '--user-data-dir=/tmp/cmp-dlg-'+Date.now(),'about:blank'], { stdio: 'ignore' });
  let t=null;
  for(let i=0;i<60;i++){ await wait(400); try{const j=await getJson('http://127.0.0.1:9687/json'); t=j.find(x=>x.type==='page'); if(t)break; }catch(e){} }
  const ws=new WebSocket(t.webSocketDebuggerUrl); let id=0; const pend=new Map();
  ws.on('message',d=>{const m=JSON.parse(d); if(m.id&&pend.has(m.id)){pend.get(m.id)(m);pend.delete(m.id);}});
  const send=(me,pa={})=>new Promise(r=>{const i=++id;pend.set(i,r);ws.send(JSON.stringify({id:i,method:me,params:pa}));});
  await new Promise(r=>ws.on('open',r));
  await send('Runtime.enable'); await send('Page.enable'); await send('Network.enable');
  await send('Network.setCacheDisabled',{cacheDisabled:true});
  await send('Emulation.setDeviceMetricsOverride',{width:1280,height:820,deviceScaleFactor:1,mobile:false});
  await send('Page.navigate',{url:`http://127.0.0.1:8770/${which}/hub.html?t=${Date.now()}`});
  await wait(5000);
  const ev=async(e)=>{const r=await send('Runtime.evaluate',{expression:e,returnByValue:true,awaitPromise:true});
    return r.result&&r.result.result?r.result.result.value:null;};
  console.log('nav:', await ev(`(function(){var n=document.querySelector('[data-page="servers"]'); if(n){n.click();return 'ok';} return 'nope';})()`));
  await wait(1500);
  console.log('add:', await ev(`(function(){
    var b = document.getElementById('addBtn') || document.querySelector('.btn.primary');
    if (b) { b.click(); return 'clicked'; }
    return 'no-add';
  })()`));
  await wait(1200);
  const png = await send('Page.captureScreenshot',{format:'png'});
  fs.writeFileSync(out, Buffer.from(png.result.data,'base64'));
  console.log('saved', out);
  ws.close(); chrome.kill(); process.exit(0);
})();
