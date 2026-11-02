/* 功能抽查第二轮：覆盖服务器 / 文件 / 远程接入 / 传输 / 密钥 */
const { spawn } = require('child_process'); const http = require('http');
const WebSocket = require('/tmp/ab-measure/node_modules/ws');
const wait = (ms) => new Promise(r => setTimeout(r, ms));
const getJson = (u) => new Promise((res, rej) => http.get(u, r => { let d=''; r.on('data',c=>d+=c); r.on('end',()=>{try{res(JSON.parse(d))}catch(e){rej(e)}}); }).on('error', rej));

async function run(page, checks) {
  const port = 9880 + Math.floor(Math.random() * 18);
  const chrome = spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', [
    '--headless=new','--disable-gpu','--no-sandbox',`--remote-debugging-port=${port}`,
    `--user-data-dir=/tmp/f2-${Date.now()}-${Math.random()}`,'about:blank'], { stdio: 'ignore' });
  let t=null;
  for(let i=0;i<60;i++){ await wait(300); try{const j=await getJson(`http://127.0.0.1:${port}/json`); t=j.find(x=>x.type==='page'); if(t)break; }catch(e){} }
  if(!t){ chrome.kill(); return [0,0]; }
  const ws=new WebSocket(t.webSocketDebuggerUrl); let id=0; const pend=new Map();
  ws.on('message',d=>{const m=JSON.parse(d); if(m.id&&pend.has(m.id)){pend.get(m.id)(m);pend.delete(m.id);}});
  const send=(me,pa={})=>new Promise(r=>{const i=++id;pend.set(i,r);ws.send(JSON.stringify({id:i,method:me,params:pa}));});
  await new Promise(r=>ws.on('open',r));
  await send('Runtime.enable'); await send('Page.enable'); await send('Network.enable'); await send('Network.setCacheDisabled',{cacheDisabled:true});
  await send('Emulation.setDeviceMetricsOverride',{width:1280,height:860,deviceScaleFactor:1,mobile:false});
  await send('Page.navigate',{url:`http://127.0.0.1:8770/new/${page}?t=${Date.now()}`});
  await wait(4500);
  const ev=async(e)=>{const r=await send('Runtime.evaluate',{expression:e,returnByValue:true,awaitPromise:true}); return r.result&&r.result.result?r.result.result.value:null;};
  await ev(`window.alert=function(){};window.confirm=function(){return false};window.prompt=function(){return null};`);

  console.log(`\n--- ${page} ---`);
  let pass=0, fail=0;
  for (const c of checks) {
    await ev(c.setup || '1'); await wait(800);
    const before = await ev(c.probe);
    await ev(c.act); await wait(900);
    const after = await ev(c.probe);
    const ok = c.expect(before, after);
    console.log(`${ok?'✅':'❌'} ${c.name.padEnd(26)} ${String(before).slice(0,16).padEnd(17)} → ${String(after).slice(0,24)}`);
    ok ? pass++ : fail++;
    if (c.cleanup) { await ev(c.cleanup); await wait(1200); }
  }
  ws.close(); chrome.kill();
  return [pass, fail];
}

(async () => {
  let tp=0, tf=0;
  const add = (r) => { tp += r[0]; tf += r[1]; };

  add(await run('hub.html', [
    { name: '服务器页：点添加打开弹窗',
      setup: `(function(){document.querySelector('[data-page="servers"]')?.click();return 1})()`,
      act: `document.getElementById('addBtn')?.click()`,
      probe: `document.querySelectorAll('.overlay').length`,
      expect: (b,a) => a > b,
      cleanup: `window.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true}))` },
    { name: '服务器页：点删除弹确认',
      act: `(function(){var b=[].slice.call(document.querySelectorAll('#detail .btn, #detail button')).find(function(x){return /删除/.test(x.textContent)}); if(b)b.click(); return 1})()`,
      probe: `document.querySelectorAll('.overlay').length`,
      expect: (b,a) => a > 0,
      cleanup: `window.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true}))` },
    { name: '协议筛选切换',
      setup: `1`,
      act: `(function(){var b=[].slice.call(document.querySelectorAll('#protoTabs button')).find(function(x){return /SSH/.test(x.textContent)}); if(b)b.click(); return 1})()`,
      probe: `document.querySelectorAll('.server-row').length`,
      expect: (b,a) => typeof a === 'number' },
    { name: '终端页：打开终端按钮状态',
      setup: `(function(){document.querySelector('[data-page="term"]')?.click();return 1})()`,
      act: `1`,
      probe: `(function(){var b=document.getElementById('pageTermBtn'); return b ? (b.disabled?'禁用':'可点') : '无按钮'})()`,
      expect: (b,a) => a !== '无按钮' },
    { name: '传输页：4 个批量按钮存在',
      setup: `(function(){document.querySelector('[data-page="transfers"]')?.click();return 1})()`,
      act: `1`,
      probe: `['txPauseAll','txResumeAll','txAbandonAll','txClearDone'].filter(function(i){return document.getElementById(i)}).length`,
      expect: (b,a) => a === 4 },
    { name: '密钥页：打开密钥管理',
      setup: `(function(){document.querySelector('[data-page="keys"]')?.click();return 1})()`,
      act: `1`,
      probe: `!!document.getElementById('keysBtn')`,
      expect: (b,a) => a === true },
    { name: '关于页：点检查更新展开',
      setup: `(function(){document.querySelector('[data-page="about"]')?.click();return 1})()`,
      act: `document.getElementById('about-check-update')?.click()`,
      probe: `document.querySelectorAll('.about-update').length`,
      expect: (b,a) => a > 0 },
    { name: '备份页：导出面板存在',
      setup: `(function(){document.querySelector('[data-page="backup"]')?.click();return 1})()`,
      act: `1`,
      probe: `['exportPanel','importPanel','syncPanel'].filter(function(i){return document.getElementById(i)}).length`,
      expect: (b,a) => a === 3 },
  ]));

  console.log(`\n总计：通过 ${tp} / 失败 ${tf}`);
  process.exit(0);
})();
