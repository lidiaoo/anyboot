/* 弹层内部交互实测 */
const { spawn } = require('child_process'); const http = require('http');
const WebSocket = require('/tmp/ab-measure/node_modules/ws');
const wait = (ms) => new Promise(r => setTimeout(r, ms));
const getJson = (u) => new Promise((res, rej) => http.get(u, r => { let d=''; r.on('data',c=>d+=c); r.on('end',()=>{try{res(JSON.parse(d))}catch(e){rej(e)}}); }).on('error', rej));

async function run(page, checks) {
  const port = 9720 + Math.floor(Math.random() * 14);
  const chrome = spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', [
    '--headless=new','--disable-gpu','--no-sandbox',`--remote-debugging-port=${port}`,
    `--user-data-dir=/tmp/df-${Date.now()}-${Math.random()}`,'about:blank'], { stdio: 'ignore' });
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
    console.log(`${ok?'✅':'❌'} ${c.name.padEnd(30)} ${String(before).slice(0,12).padEnd(13)} → ${String(after).slice(0,20)}`);
    ok ? pass++ : fail++;
    if (c.cleanup) { await ev(c.cleanup); await wait(800); }
  }
  ws.close(); chrome.kill();
  return [pass, fail];
}

(async () => {
  let tp=0, tf=0;
  const add = (r) => { tp += r[0]; tf += r[1]; };
  const closeAll = `window.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true}))`;
  const openEdit = `(function(){document.querySelector('[data-page="servers"]')?.click(); setTimeout(function(){document.getElementById('addBtn')?.click()},300); return 1})()`;

  add(await run('hub.html', [
    { name: '编辑弹窗：协议切 Telnet', setup: openEdit,
      act: `(function(){var b=[].slice.call(document.querySelectorAll('#editOverlay .segmented button, .dialog .segmented button')).find(function(x){return /Telnet/.test(x.textContent)}); if(b)b.click(); return 1})()`,
      probe: `(function(){var b=[].slice.call(document.querySelectorAll('.dialog .segmented button')).find(function(x){return /Telnet/.test(x.textContent)}); return b?b.getAttribute('aria-selected'):'none'})()`,
      expect: (b,a) => a === 'true', cleanup: closeAll },
    { name: '编辑弹窗：认证切「密钥」显示选择框', setup: openEdit,
      act: `(function(){var b=[].slice.call(document.querySelectorAll('.dialog .segmented button')).find(function(x){return x.textContent.trim()==='密钥'}); if(b)b.click(); return 1})()`,
      probe: `document.querySelectorAll('#fKey').length`,
      expect: (b,a) => a === 1, cleanup: closeAll },
    { name: '编辑弹窗：认证切「密码」显示密码框', setup: openEdit,
      act: `(function(){var b=[].slice.call(document.querySelectorAll('.dialog .segmented button')).find(function(x){return x.textContent.trim()==='密码'}); if(b)b.click(); return 1})()`,
      probe: `(function(){var e=document.getElementById('fPassword'); return e ? (e.offsetParent?'可见':'隐藏') : '无'})()`,
      expect: (b,a) => a === '可见', cleanup: closeAll },
    { name: '编辑弹窗：名称空时保存被拦', setup: openEdit,
      act: `(function(){var f=document.getElementById('fName'); if(f){f.value=''; f.dispatchEvent(new Event('input',{bubbles:true}))} var b=[].slice.call(document.querySelectorAll('.dialog-foot button')).find(function(x){return /保存/.test(x.textContent)}); if(b)b.click(); return 1})()`,
      probe: `(function(){var e=document.querySelector('.form-error'); return e ? e.textContent.trim() : '无错误元素'})()`,
      expect: (b,a) => a !== '' && a !== '无错误元素', cleanup: closeAll },
    { name: '配对弹层：5 个地址类型页签', setup: `(function(){document.querySelector('[data-page="gateway"]')?.click(); setTimeout(function(){var t=[].slice.call(document.querySelectorAll('.gw-tab')).find(function(x){return /配对管理/.test(x.textContent)}); if(t)t.click(); setTimeout(function(){var p=[].slice.call(document.querySelectorAll('button')).find(function(x){return /签发新配对码/.test(x.textContent)}); if(p)p.click()},300)},300); return 1})()`,
      act: `1`, probe: `document.querySelectorAll('.gw-pair-modal .segmented button').length`,
      expect: (b,a) => a === 5, cleanup: closeAll },
  ]));

  console.log(`\n总计：通过 ${tp} / 失败 ${tf}`);
  process.exit(0);
})();
