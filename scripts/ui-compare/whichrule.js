const { spawn } = require('child_process');
const http = require('http');
const WebSocket = require('/tmp/ab-measure/node_modules/ws');
const wait = (ms) => new Promise(r => setTimeout(r, ms));
const getJson = (u) => new Promise((res, rej) => http.get(u, r => { let d=''; r.on('data',c=>d+=c); r.on('end',()=>{try{res(JSON.parse(d))}catch(e){rej(e)}}); }).on('error', rej));
(async () => {
  const chrome = spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', [
    '--headless=new','--disable-gpu','--no-sandbox','--remote-debugging-port=9675',
    '--user-data-dir=/tmp/cmp-w-'+Date.now(),'about:blank'], { stdio: 'ignore' });
  let t=null;
  for(let i=0;i<60;i++){ await wait(400); try{const j=await getJson('http://127.0.0.1:9675/json'); t=j.find(x=>x.type==='page'); if(t)break; }catch(e){} }
  const ws=new WebSocket(t.webSocketDebuggerUrl); let id=0; const pend=new Map();
  ws.on('message',d=>{const m=JSON.parse(d); if(m.id&&pend.has(m.id)){pend.get(m.id)(m);pend.delete(m.id);}});
  const send=(me,pa={})=>new Promise(r=>{const i=++id;pend.set(i,r);ws.send(JSON.stringify({id:i,method:me,params:pa}));});
  await new Promise(r=>ws.on('open',r));
  await send('Runtime.enable'); await send('Page.enable'); await send('Network.enable');
  await send('Network.setCacheDisabled',{cacheDisabled:true});
  await send('Page.navigate',{url:'http://127.0.0.1:8770/new/hub.html?t='+Date.now()});
  await wait(6000);
  const ev=async(e)=>{const r=await send('Runtime.evaluate',{expression:e,returnByValue:true,awaitPromise:true});
    return r.result&&r.result.result?r.result.result.value:null;};
  console.log('ui-alpha =', await ev(`getComputedStyle(document.documentElement).getPropertyValue('--ui-alpha')`));
  console.log('body 内联 style =', await ev(`document.body.getAttribute('style')`));
  console.log('html 内联 style =', await ev(`document.documentElement.getAttribute('style')`));
  console.log('body 背景 =', await ev(`getComputedStyle(document.body).backgroundColor`));
  console.log('html 背景 =', await ev(`getComputedStyle(document.documentElement).backgroundColor`));
  // 哪条规则给了 body 背景
  console.log('匹配的 body 规则 =', await ev(`(function(){
    var out=[];
    for (var i=0;i<document.styleSheets.length;i++){
      var ss=document.styleSheets[i];
      try {
        for (var j=0;j<ss.cssRules.length;j++){
          var r=ss.cssRules[j];
          if (r.selectorText && /^(body|html)$/.test(r.selectorText.replace(/\\[.*?\\]/g,'')) && r.style.background){
            out.push(r.selectorText + ' => ' + r.style.background);
          }
        }
      } catch(e){}
    }
    return JSON.stringify(out.slice(0,6),null,1);
  })()`));
  ws.close(); chrome.kill(); process.exit(0);
})();
