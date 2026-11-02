/* 检查每个页面/窗口是否真的渲染出了「有实质内容」的界面 */
const { spawn } = require('child_process'); const http = require('http');
const fs = require('fs');
const WebSocket = require('/tmp/ab-measure/node_modules/ws');
const wait = (ms) => new Promise(r => setTimeout(r, ms));
const getJson = (u) => new Promise((res, rej) => http.get(u, r => { let d=''; r.on('data',c=>d+=c); r.on('end',()=>{try{res(JSON.parse(d))}catch(e){rej(e)}}); }).on('error', rej));

const TARGETS = [
  ['index.html', null], ['hub.html', 'home'], ['hub.html', 'computers'], ['hub.html', 'servers'],
  ['hub.html', 'gateway'], ['hub.html', 'term'], ['hub.html', 'sessions'], ['hub.html', 'transfers'],
  ['hub.html', 'files'], ['hub.html', 'keys'], ['hub.html', 'termset'], ['hub.html', 'backup'],
  ['hub.html', 'about'], ['hub-term.html', null], ['hub-files.html', null],
  ['hub-backup.html', null], ['hub-keys.html', null],
];

async function check(page, tab) {
  const port = 9960 + Math.floor(Math.random() * 18);
  const chrome = spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', [
    '--headless=new','--disable-gpu','--no-sandbox',`--remote-debugging-port=${port}`,
    `--user-data-dir=/tmp/rc-${Date.now()}-${Math.random()}`,'about:blank'], { stdio: 'ignore' });
  let t=null;
  for(let i=0;i<60;i++){ await wait(300); try{const j=await getJson(`http://127.0.0.1:${port}/json`); t=j.find(x=>x.type==='page'); if(t)break; }catch(e){} }
  if(!t){ chrome.kill(); return null; }
  const ws=new WebSocket(t.webSocketDebuggerUrl); let id=0; const pend=new Map();
  ws.on('message',d=>{const m=JSON.parse(d); if(m.id&&pend.has(m.id)){pend.get(m.id)(m);pend.delete(m.id);}});
  const send=(me,pa={})=>new Promise(r=>{const i=++id;pend.set(i,r);ws.send(JSON.stringify({id:i,method:me,params:pa}));});
  await new Promise(r=>ws.on('open',r));
  await send('Runtime.enable'); await send('Page.enable'); await send('Network.enable'); await send('Network.setCacheDisabled',{cacheDisabled:true});
  await send('Emulation.setDeviceMetricsOverride',{width:1280,height:860,deviceScaleFactor:1,mobile:false});
  await send('Page.navigate',{url:`http://127.0.0.1:8770/new/${page}?t=${Date.now()}`});
  await wait(4500);
  const ev=async(e)=>{const r=await send('Runtime.evaluate',{expression:e,returnByValue:true,awaitPromise:true}); return r.result&&r.result.result?r.result.result.value:null;};
  if(tab) { await ev(`(function(){var n=document.querySelector('[data-page="${tab}"]')||document.querySelector('[data-tab="${tab}"]'); if(n)n.click(); return 1;})()`); await wait(1200); }
  const r = await ev(`(function(){
    var root = document.querySelector('.page:not([hidden])') || document.querySelector('.tab-panel.active') || document.body;
    var txt = (root.textContent||'').replace(/\\s+/g,' ').trim();
    return JSON.stringify({
      textLen: txt.length,
      elements: root.querySelectorAll('*').length,
      buttons: root.querySelectorAll('button').length,
      inputs: root.querySelectorAll('input,select,textarea').length,
      head: txt.slice(0, 50),
      placeholder: /正在迁移中|阶段 \\d 迁入|待补|TODO/.test(txt),
    });
  })()`);
  ws.close(); chrome.kill();
  return r ? JSON.parse(r) : null;
}

(async () => {
  console.log('页面'.padEnd(20), '元素'.padEnd(6), '按钮'.padEnd(5), '输入'.padEnd(5), '判定');
  console.log('-'.repeat(70));
  let bad = 0;
  for (const [page, tab] of TARGETS) {
    const r = await check(page, tab);
    const label = page + (tab ? ':' + tab : '');
    if (!r) { console.log(label.padEnd(20), '—', '', '', '❌ 打不开'); bad++; continue; }
    const ok = !r.placeholder && r.elements > 5 && r.textLen > 5;
    if (!ok) bad++;
    console.log(label.padEnd(20), String(r.elements).padEnd(6), String(r.buttons).padEnd(5), String(r.inputs).padEnd(5),
      (ok ? '✅' : '❌ ' + (r.placeholder ? '占位' : '内容过少') ), '|', r.head.slice(0, 30));
  }
  console.log('-'.repeat(70));
  console.log(`不合格: ${bad} / ${TARGETS.length}`);
  process.exit(0);
})();
