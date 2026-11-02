const { spawn } = require('child_process');
const http = require('http');
const WebSocket = require('/tmp/ab-measure/node_modules/ws');
const wait = (ms) => new Promise(r => setTimeout(r, ms));
const getJson = (u) => new Promise((res, rej) => http.get(u, r => { let d=''; r.on('data',c=>d+=c); r.on('end',()=>{try{res(JSON.parse(d))}catch(e){rej(e)}}); }).on('error', rej));
(async () => {
  const chrome = spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', [
    '--headless=new','--disable-gpu','--no-sandbox','--remote-debugging-port=9677',
    '--user-data-dir=/tmp/cmp-f-'+Date.now(),'about:blank'], { stdio: 'ignore' });
  let t=null;
  for(let i=0;i<60;i++){ await wait(400); try{const j=await getJson('http://127.0.0.1:9677/json'); t=j.find(x=>x.type==='page'); if(t)break; }catch(e){} }
  const ws=new WebSocket(t.webSocketDebuggerUrl); let id=0; const pend=new Map();
  ws.on('message',d=>{const m=JSON.parse(d); if(m.id&&pend.has(m.id)){pend.get(m.id)(m);pend.delete(m.id);}});
  const send=(me,pa={})=>new Promise(r=>{const i=++id;pend.set(i,r);ws.send(JSON.stringify({id:i,method:me,params:pa}));});
  await new Promise(r=>ws.on('open',r));
  await send('Runtime.enable'); await send('Page.enable'); await send('Network.enable');
  await send('Emulation.setDeviceMetricsOverride',{width:1280,height:820,deviceScaleFactor:1,mobile:false});
  await send('Network.setCacheDisabled',{cacheDisabled:true});
  await send('Page.navigate',{url:'http://127.0.0.1:8770/new/hub.html?t='+Date.now()});
  await wait(6000);
  const ev=async(e)=>{const r=await send('Runtime.evaluate',{expression:e,returnByValue:true,awaitPromise:true});
    return r.result&&r.result.result?r.result.result.value:null;};
  console.log('innerWidth/Height:', await ev('window.innerWidth + 'x' + window.innerHeight'));
  console.log('footer 存在:', await ev(`!!document.querySelector('footer')`));
  console.log('footer 盒子:', await ev(`(function(){var f=document.querySelector('footer'); if(!f)return null; var r=f.getBoundingClientRect(); var s=getComputedStyle(f); return JSON.stringify({w:Math.round(r.width),h:Math.round(r.height),y:Math.round(r.top),display:s.display,pos:s.position});})()`));
  console.log('body 盒子:', await ev(`(function(){var b=document.body; var r=b.getBoundingClientRect(); var s=getComputedStyle(b); return JSON.stringify({w:Math.round(r.width),h:Math.round(r.height),display:s.display,flexDir:s.flexDirection,overflow:s.overflow});})()`));
  console.log('app 盒子:', await ev(`(function(){var a=document.getElementById('app'); if(!a)return null; var r=a.getBoundingClientRect(); var s=getComputedStyle(a); return JSON.stringify({w:Math.round(r.width),h:Math.round(r.height),display:s.display});})()`));
  console.log('body 子元素:', await ev(`[].slice.call(document.body.children).map(function(c){return c.tagName+'.'+c.className;}).join(' | ')`));
  ws.close(); chrome.kill(); process.exit(0);
})();
