/* 主题渲染实测：切到每套主题，检查真的渲染出内容（而非空壳） */
const { spawn } = require('child_process'); const http = require('http');
const WebSocket = require('/tmp/ab-measure/node_modules/ws');
const wait = (ms) => new Promise(r => setTimeout(r, ms));
const getJson = (u) => new Promise((res, rej) => http.get(u, r => { let d=''; r.on('data',c=>d+=c); r.on('end',()=>{try{res(JSON.parse(d))}catch(e){rej(e)}}); }).on('error', rej));

async function checkTheme(theme) {
  const port = 9800 + Math.floor(Math.random() * 16);
  const chrome = spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', [
    '--headless=new','--disable-gpu','--no-sandbox',`--remote-debugging-port=${port}`,
    `--user-data-dir=/tmp/tr-${Date.now()}-${Math.random()}`,'about:blank'], { stdio: 'ignore' });
  let t=null;
  for(let i=0;i<60;i++){ await wait(300); try{const j=await getJson(`http://127.0.0.1:${port}/json`); t=j.find(x=>x.type==='page'); if(t)break; }catch(e){} }
  if(!t){ chrome.kill(); return null; }
  const ws=new WebSocket(t.webSocketDebuggerUrl); let id=0; const pend=new Map(); const errs=[];
  ws.on('message',d=>{const m=JSON.parse(d);
    if(m.method==='Runtime.exceptionThrown'){const e=m.params.exceptionDetails; errs.push(String((e.exception&&e.exception.description)||e.text).slice(0,150));}
    if(m.id&&pend.has(m.id)){pend.get(m.id)(m);pend.delete(m.id);}});
  const send=(me,pa={})=>new Promise(r=>{const i=++id;pend.set(i,r);ws.send(JSON.stringify({id:i,method:me,params:pa}));});
  await new Promise(r=>ws.on('open',r));
  await send('Runtime.enable'); await send('Page.enable'); await send('Network.enable'); await send('Network.setCacheDisabled',{cacheDisabled:true});
  await send('Emulation.setDeviceMetricsOverride',{width:1280,height:860,deviceScaleFactor:1,mobile:false});
  await send('Page.navigate',{url:'http://127.0.0.1:8770/new/index.html?t='+Date.now()});
  await wait(1500);
  // 设主题 + 启用全部引导器
  await send('Runtime.evaluate',{expression:`(function(){
    var o; try{o=JSON.parse(localStorage.getItem('anyboot.boot-adv')||'{}')}catch(e){o={}}
    o.theme='${theme}'; o.layout='default';
    o.visible={refind:true,grub:true,sdboot:true,opencore:true,clover:true};
    localStorage.setItem('anyboot.boot-adv',JSON.stringify(o)); return 1;})()`});
  await send('Page.reload');
  await wait(5000);
  const ev=async(e)=>{const r=await send('Runtime.evaluate',{expression:e,returnByValue:true,awaitPromise:true}); return r.result&&r.result.result?r.result.result.value:null;};
  const info = await ev(`(function(){
    var root = document.querySelector('.k-theme-fit') || document.querySelector('.k-stage');
    if (!root) return JSON.stringify({found:false});
    var txt = (root.textContent||'').replace(/\\s+/g,' ').trim();
    return JSON.stringify({
      found: true,
      rootChildCls: (root.firstElementChild && root.firstElementChild.className) || 'none',
      elements: root.querySelectorAll('*').length,
      buttons: root.querySelectorAll('button').length,
      textLen: txt.length,
      head: txt.slice(0, 36),
      emptyHint: /还没实装|没有启用的引导器/.test(txt),
      classes: [].slice.call(root.querySelectorAll('[class]')).map(function(e){return typeof e.className==='string'?e.className:''}).join(' ').slice(0, 400),
    });
  })()`);
  ws.close(); chrome.kill();
  return { info: info ? JSON.parse(info) : null, errs };
}

(async () => {
  console.log('主题  子元素类      元素  按钮  文字  判定');
  console.log('-'.repeat(66));
  let bad = 0;
  for (const th of ['A','B','C','G','I','J']) {
    const r = await checkTheme(th);
    if (!r?.info?.found) { console.log(`${th}    ❌ 找不到主题根`); bad++; continue; }
    const i = r.info;
    const ok = i.elements > 20 && i.textLen > 15 && !i.emptyHint && r.errs.length === 0;
    if (!ok) bad++;
    console.log(`${th}    ${i.rootChildCls.slice(0,12).padEnd(13)} ${String(i.elements).padEnd(5)} ${String(i.buttons).padEnd(5)} ${String(i.textLen).padEnd(5)} ${ok?'✅':'❌ '+(i.emptyHint?'空壳':(r.errs[0]||'内容过少'))}`);
  }
  console.log('-'.repeat(66));
  console.log(`不合格: ${bad} / 6`);
  process.exit(0);
})();
