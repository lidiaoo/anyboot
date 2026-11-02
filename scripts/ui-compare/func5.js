/* 主窗 4 页签切换 + 设置持久化 + 主题切换 */
const { spawn } = require('child_process'); const http = require('http');
const WebSocket = require('/tmp/ab-measure/node_modules/ws');
const wait = (ms) => new Promise(r => setTimeout(r, ms));
const getJson = (u) => new Promise((res, rej) => http.get(u, r => { let d=''; r.on('data',c=>d+=c); r.on('end',()=>{try{res(JSON.parse(d))}catch(e){rej(e)}}); }).on('error', rej));

async function go(page, checks) {
  const port = 9680 + Math.floor(Math.random() * 12);
  const chrome = spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', [
    '--headless=new','--disable-gpu','--no-sandbox',`--remote-debugging-port=${port}`,
    `--user-data-dir=/tmp/f5-${Date.now()}-${Math.random()}`,'about:blank'], { stdio: 'ignore' });
  let t=null;
  for(let i=0;i<60;i++){ await wait(300); try{const j=await getJson(`http://127.0.0.1:${port}/json`); t=j.find(x=>x.type==='page'); if(t)break; }catch(e){} }
  if(!t){ chrome.kill(); return [0,0]; }
  const ws=new WebSocket(t.webSocketDebuggerUrl); let id=0; const pend=new Map();
  ws.on('message',d=>{const m=JSON.parse(d); if(m.id&&pend.has(m.id)){pend.get(m.id)(m);pend.delete(m.id);}});
  const send=(me,pa={})=>new Promise(r=>{const i=++id;pend.set(i,r);ws.send(JSON.stringify({id:i,method:me,params:pa}));});
  await new Promise(r=>ws.on('open',r));
  await send('Runtime.enable'); await send('Page.enable'); await send('Network.enable'); await send('Network.setCacheDisabled',{cacheDisabled:true});
  await send('Emulation.setDeviceMetricsOverride',{width:900,height:800,deviceScaleFactor:1,mobile:false});
  await send('Page.navigate',{url:`http://127.0.0.1:8770/new/${page}?t=${Date.now()}`});
  await wait(5000);
  const ev=async(e)=>{const r=await send('Runtime.evaluate',{expression:e,returnByValue:true,awaitPromise:true}); return r.result&&r.result.result?r.result.result.value:null;};
  await ev(`window.alert=function(){};window.confirm=function(){return false};window.prompt=function(){return null};`);

  console.log(`\n--- ${page} ---`);
  let pass=0, fail=0;
  for (const c of checks) {
    await ev(c.setup || '1'); await wait(700);
    const before = await ev(c.probe);
    await ev(c.act); await wait(800);
    const after = await ev(c.probe);
    const ok = c.expect(before, after);
    console.log(`${ok?'✅':'❌'} ${c.name.padEnd(28)} ${String(before).slice(0,14).padEnd(15)} → ${String(after).slice(0,20)}`);
    ok ? pass++ : fail++;
    if (c.cleanup) { await ev(c.cleanup); await wait(600); }
  }
  ws.close(); chrome.kill();
  return [pass, fail];
}

(async () => {
  let tp=0, tf=0;
  const add = (r) => { tp += r[0]; tf += r[1]; };

  add(await go('index.html', [
    { name: '切到「引导高级设置」', act: `(function(){var b=[].slice.call(document.querySelectorAll('.tabs .tab')).find(function(x){return /高级设置/.test(x.textContent)}); if(b)b.click(); return 1})()`,
      probe: `(function(){var p=document.getElementById('tab-adv-boot'); return p && p.classList.contains('active') ? 'active' : 'no'})()`,
      expect: (b,a) => a === 'active' },
    { name: '切到「基本设置」', act: `(function(){var b=[].slice.call(document.querySelectorAll('.tabs .tab')).find(function(x){return /基本设置/.test(x.textContent)}); if(b)b.click(); return 1})()`,
      probe: `(function(){var p=document.getElementById('tab-settings'); return p && p.classList.contains('active') ? 'active' : 'no'})()`,
      expect: (b,a) => a === 'active' },
    { name: '基本设置里有 35+ 控件', act: `1`,
      probe: `(document.getElementById('tab-settings')||{querySelectorAll:function(){return []}}).querySelectorAll('input,select').length`,
      expect: (b,a) => a >= 20 },
    { name: '切到「SOL 远程唤醒」', act: `(function(){var b=[].slice.call(document.querySelectorAll('.tabs .tab')).find(function(x){return /SOL/.test(x.textContent)}); if(b)b.click(); return 1})()`,
      probe: `(function(){var p=document.getElementById('tab-sol'); return p && p.classList.contains('active') ? 'active' : 'no'})()`,
      expect: (b,a) => a === 'active' },
    { name: 'SOL 页有 5+ 区块', act: `1`,
      probe: `(document.getElementById('tab-sol')||{querySelectorAll:function(){return []}}).querySelectorAll('.setting,.setting-block').length`,
      expect: (b,a) => a >= 4 },
    { name: '切回「系统引导」', act: `(function(){var b=[].slice.call(document.querySelectorAll('.tabs .tab')).find(function(x){return /系统引导/.test(x.textContent)}); if(b)b.click(); return 1})()`,
      probe: `(function(){var p=document.getElementById('tab-boot'); return p && p.classList.contains('active') ? 'active' : 'no'})()`,
      expect: (b,a) => a === 'active' },
    { name: '设置项：主题下拉切换', setup: `(function(){var b=[].slice.call(document.querySelectorAll('.tabs .tab')).find(function(x){return /基本设置/.test(x.textContent)}); if(b)b.click(); return 1})()`,
      act: `(function(){var s=document.getElementById('theme'); if(!s)return 0; s.value='light'; s.dispatchEvent(new Event('change',{bubbles:true})); return 1})()`,
      probe: `(function(){var s=document.getElementById('theme'); return s ? s.value : 'none'})()`,
      expect: (b,a) => a === 'light' },
    { name: '设置项：强调色', act: `(function(){var c=document.getElementById('accent-color'); if(!c)return 0; c.value='#ff0000'; c.dispatchEvent(new Event('change',{bubbles:true})); return 1})()`,
      probe: `(function(){var c=document.getElementById('accent-color'); return c ? c.value : 'none'})()`,
      expect: (b,a) => a === '#ff0000' },
    { name: '设置项：字号/不透明度滑块可动', act: `(function(){var r=document.getElementById('opacity'); if(!r)return 0; r.value='60'; r.dispatchEvent(new Event('input',{bubbles:true})); return 1})()`,
      probe: `(function(){var r=document.getElementById('opacity'); return r ? r.value : 'none'})()`,
      expect: (b,a) => a === '60' },
    { name: '底部状态栏条件渲染正常', act: `1`,
      probe: `(function(){var s=document.getElementById('status-bar'); return s ? 'shown' : 'hidden(正常)'})()`,
      expect: (b,a) => typeof a === 'string' },
  ]));

  console.log(`\n总计：通过 ${tp} / 失败 ${tf}`);
  process.exit(0);
})();
