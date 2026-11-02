const { spawn } = require('child_process');
const http = require('http');
const WebSocket = require('/tmp/ab-measure/node_modules/ws');
const wait = (ms) => new Promise(r => setTimeout(r, ms));
const getJson = (u) => new Promise((res, rej) => http.get(u, r => { let d=''; r.on('data',c=>d+=c); r.on('end',()=>{try{res(JSON.parse(d))}catch(e){rej(e)}}); }).on('error', rej));
(async () => {
  const chrome = spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', [
    '--headless=new','--disable-gpu','--no-sandbox','--remote-debugging-port=9685',
    '--user-data-dir=/tmp/cmp-cr-'+Date.now(),'about:blank'], { stdio: 'ignore' });
  let t=null;
  for(let i=0;i<60;i++){ await wait(400); try{const j=await getJson('http://127.0.0.1:9685/json'); t=j.find(x=>x.type==='page'); if(t)break; }catch(e){} }
  const ws=new WebSocket(t.webSocketDebuggerUrl); let id=0; const pend=new Map();
  ws.on('message',d=>{const m=JSON.parse(d); if(m.id&&pend.has(m.id)){pend.get(m.id)(m);pend.delete(m.id);}});
  const send=(me,pa={})=>new Promise(r=>{const i=++id;pend.set(i,r);ws.send(JSON.stringify({id:i,method:me,params:pa}));});
  await new Promise(r=>ws.on('open',r));
  await send('Runtime.enable'); await send('Page.enable'); await send('Network.enable');
  await send('Network.setCacheDisabled',{cacheDisabled:true});
  await send('Emulation.setDeviceMetricsOverride',{width:1280,height:820,deviceScaleFactor:1,mobile:false});
  await send('Page.navigate',{url:'http://127.0.0.1:8770/old/hub.html?t='+Date.now()});
  await wait(5000);
  const ev=async(e)=>{const r=await send('Runtime.evaluate',{expression:e,returnByValue:true,awaitPromise:true});
    return r.result&&r.result.result?r.result.result.value:null;};
  await ev(`(function(){var n=document.querySelector('[data-page="files"]'); if(n)n.click(); return 1;})()`);
  await wait(2000);
  console.log(await ev(`(function(){
    var nav = document.querySelector('.fb-crumbs');
    var b = document.querySelector('.fb-crumb');
    if (!nav) return 'no .fb-crumbs';
    if (!b) return 'no .fb-crumb (nav 里是: ' + nav.innerHTML.slice(0,150) + ')';
    var s = getComputedStyle(b);
    return JSON.stringify({
      pageId: document.querySelector('.page') ? document.querySelector('.page').id : 'none',
      crumbBg: s.backgroundColor, crumbColor: s.color, crumbBorder: s.borderTopWidth,
      fbBlue: getComputedStyle(document.getElementById('page-files') || document.body).getPropertyValue('--fb-blue'),
      htmlClass: document.documentElement.className,
    }, null, 1);
  })()`));
  ws.close(); chrome.kill(); process.exit(0);
})();
