const { spawn } = require('child_process'); const http = require('http');
const WebSocket = require('/tmp/ab-measure/node_modules/ws');
const wait = (ms) => new Promise(r => setTimeout(r, ms));
const getJson = (u) => new Promise((res, rej) => http.get(u, r => { let d=''; r.on('data',c=>d+=c); r.on('end',()=>{try{res(JSON.parse(d))}catch(e){rej(e)}}); }).on('error', rej));

async function run(page, actions) {
  const port = 9580 + Math.floor(Math.random()*12);
  const chrome = spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', ['--headless=new','--disable-gpu','--no-sandbox',`--remote-debugging-port=${port}`,'--user-data-dir=/tmp/st-'+Date.now()+Math.random(),'about:blank'], { stdio: 'ignore' });
  let t=null;
  for(let i=0;i<60;i++){ await wait(300); try{const j=await getJson(`http://127.0.0.1:${port}/json`); t=j.find(x=>x.type==='page'); if(t)break; }catch(e){} }
  if(!t){chrome.kill();return [];}
  const ws=new WebSocket(t.webSocketDebuggerUrl); let id=0; const pend=new Map();
  ws.on('message',d=>{const m=JSON.parse(d); if(m.id&&pend.has(m.id)){pend.get(m.id)(m);pend.delete(m.id);}});
  const send=(me,pa={})=>new Promise(r=>{const i=++id;pend.set(i,r);ws.send(JSON.stringify({id:i,method:me,params:pa}));});
  await new Promise(r=>ws.on('open',r));
  await send('Runtime.enable'); await send('Page.enable'); await send('Network.enable'); await send('Network.setCacheDisabled',{cacheDisabled:true});
  await send('Page.navigate',{url:`http://127.0.0.1:8770/new/${page}?t=${Date.now()}`});
  await wait(5000);
  const ev=async(e)=>{const r=await send('Runtime.evaluate',{expression:e,returnByValue:true,awaitPromise:true}); return r.result&&r.result.result?r.result.result.value:null;};
  await ev(`window.alert=function(){};window.confirm=function(){return false};window.prompt=function(){return null};`);
  if (actions) await actions(ev, wait);
  const v = await ev(`JSON.stringify(window.__STRICT_VIOLATIONS__||[])`);
  ws.close(); chrome.kill();
  return JSON.parse(v || '[]');
}

(async () => {
  const allV = [];
  // hub 各页
  const hubV = await run('hub.html', async (ev, wait) => {
    for (const pg of ['computers','servers','gateway','files','keys','termset','backup','about','term','transfers']) {
      await ev(`(function(){document.querySelector('[data-page="${pg}"]')?.click();return 1})()`); await wait(500);
    }
  });
  allV.push(...hubV.map(x=>'hub: '+x));
  // 主窗
  const mainV = await run('index.html', async (ev, wait) => {
    for (const tab of ['系统引导','引导高级设置','基本设置','SOL']) {
      await ev(`(function(){var b=[].slice.call(document.querySelectorAll('.tabs .tab')).find(function(x){return x.textContent.indexOf('${tab}')>=0}); if(b)b.click(); return 1})()`); await wait(600);
    }
  });
  allV.push(...mainV.map(x=>'主窗: '+x));
  // 4 独立窗
  for (const w of ['hub-term.html','hub-files.html','hub-keys.html','hub-backup.html']) {
    const v = await run(w, async (ev, wait) => { await wait(1000); });
    allV.push(...v.map(x=>w+': '+x));
  }
  console.log('===== 严格模式违规（前端传的参数不满足 Rust 必填）=====');
  const uniq = [...new Set(allV)];
  uniq.forEach(x => console.log('  ❌ ' + x));
  console.log(`\n合计 ${uniq.length} 条违规`);
  process.exit(0);
})();
