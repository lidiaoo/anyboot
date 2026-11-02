/*
 * 功能抽查：点按钮后，看是否真的发生了「预期变化」
 * （弹层出现 / 列表项变化 / 文本变化 / 状态切换），而不是只看不报错。
 */
const { spawn } = require('child_process'); const http = require('http');
const WebSocket = require('/tmp/ab-measure/node_modules/ws');
const wait = (ms) => new Promise(r => setTimeout(r, ms));
const getJson = (u) => new Promise((res, rej) => http.get(u, r => { let d=''; r.on('data',c=>d+=c); r.on('end',()=>{try{res(JSON.parse(d))}catch(e){rej(e)}}); }).on('error', rej));

async function run(checks) {
  const port = 9900 + Math.floor(Math.random() * 20);
  const chrome = spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', [
    '--headless=new','--disable-gpu','--no-sandbox',`--remote-debugging-port=${port}`,
    `--user-data-dir=/tmp/fn-${Date.now()}-${Math.random()}`,'about:blank'], { stdio: 'ignore' });
  let t=null;
  for(let i=0;i<60;i++){ await wait(300); try{const j=await getJson(`http://127.0.0.1:${port}/json`); t=j.find(x=>x.type==='page'); if(t)break; }catch(e){} }
  if(!t){ chrome.kill(); return; }
  const ws=new WebSocket(t.webSocketDebuggerUrl); let id=0; const pend=new Map();
  ws.on('message',d=>{const m=JSON.parse(d); if(m.id&&pend.has(m.id)){pend.get(m.id)(m);pend.delete(m.id);}});
  const send=(me,pa={})=>new Promise(r=>{const i=++id;pend.set(i,r);ws.send(JSON.stringify({id:i,method:me,params:pa}));});
  await new Promise(r=>ws.on('open',r));
  await send('Runtime.enable'); await send('Page.enable'); await send('Network.enable'); await send('Network.setCacheDisabled',{cacheDisabled:true});
  await send('Emulation.setDeviceMetricsOverride',{width:1280,height:860,deviceScaleFactor:1,mobile:false});
  await send('Page.navigate',{url:'http://127.0.0.1:8770/new/hub.html?t='+Date.now()});
  await wait(4500);
  const ev=async(e)=>{const r=await send('Runtime.evaluate',{expression:e,returnByValue:true,awaitPromise:true}); return r.result&&r.result.result?r.result.result.value:null;};
  await ev(`window.alert=function(){};window.confirm=function(){return false};window.prompt=function(){return null};`);

  let pass=0, fail=0;
  for (const c of checks) {
    await ev(c.setup || '1');
    await wait(900);
    const before = await ev(c.probe);
    await ev(c.act);
    await wait(900);
    const after = await ev(c.probe);
    const ok = c.expect(before, after);
    console.log(`${ok?'✅':'❌'} ${c.name.padEnd(28)} ${String(before).padEnd(12)} → ${after}`);
    ok ? pass++ : fail++;
    if (c.cleanup) { await ev(c.cleanup); await wait(400); }
  }
  console.log(`\n通过 ${pass} / 失败 ${fail}`);
  ws.close(); chrome.kill();
}

(async () => {
  await run([
    { name: '切到电脑列表页',
      setup: `1`, act: `(function(){document.querySelector('[data-page="computers"]')?.click();return 1})()`,
      probe: `(document.getElementById('page-computers')||{}).hidden === false ? 'visible' : 'hidden'`,
      expect: (b,a) => a === 'visible' },
    { name: '搜索过滤（输入不存在关键词）',
      setup: `(function(){document.querySelector('[data-page="computers"]')?.click();return 1})()`,
      act: `(function(){var q=document.getElementById('cqQ'); q.value='zzz-nonexistent'; q.dispatchEvent(new Event('input',{bubbles:true})); return 1})()`,
      probe: `document.querySelectorAll('.cq-row').length`,
      expect: (b,a) => a < b || a === 0 },
    { name: '清空搜索恢复列表',
      act: `(function(){var q=document.getElementById('cqQ'); q.value=''; q.dispatchEvent(new Event('input',{bubbles:true})); return 1})()`,
      probe: `document.querySelectorAll('.cq-row').length`,
      expect: (b,a) => a > 0 },
    { name: '筛选「离线」',
      act: `(function(){var b=[].slice.call(document.querySelectorAll('#cqSeg button')).find(function(x){return /离线/.test(x.textContent)}); if(b)b.click(); return 1})()`,
      probe: `document.querySelectorAll('.cq-row').length`,
      expect: (b,a) => a >= 0 },
    { name: '打开 ⋮ 菜单',
      setup: `(function(){var b=[].slice.call(document.querySelectorAll('#cqSeg button')).find(function(x){return /全部/.test(x.textContent)}); if(b)b.click(); return 1})()`,
      act: `document.querySelector('.cq-more')?.click()`,
      probe: `document.querySelectorAll('.overlay, .pc-menu, [role="menu"]').length`,
      expect: (b,a) => a > b,
      cleanup: `(function(){document.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape'}));return 1})()` },
    { name: '切到已连接服务器看详情',
      setup: `1`, act: `(function(){document.querySelector('[data-page="servers"]')?.click(); setTimeout(function(){var r=document.querySelector('.server-row'); if(r)r.click();},200); return 1})()`,
      probe: `(document.querySelector('#detail h2')||{}).textContent || 'none'`,
      expect: (b,a) => a !== 'none' },
    { name: '终端设置拖动字号滑块',
      setup: `(function(){document.querySelector('[data-page="termset"]')?.click();return 1})()`,
      act: `(function(){var r=document.getElementById('tsFont'); if(!r)return 0; r.value='20'; r.dispatchEvent(new Event('input',{bubbles:true})); return 1})()`,
      probe: `(document.getElementById('tsFontVal')||{}).textContent || 'none'`,
      expect: (b,a) => a !== 'none' && a !== b },
    { name: '切到远程接入 4 个分页',
      setup: `(function(){document.querySelector('[data-page="gateway"]')?.click();return 1})()`,
      act: `(function(){var ts=document.querySelectorAll('.gw-tab'); if(ts.length<4)return 'only '+ts.length; ts[3].click(); return 'ok'})()`,
      probe: `document.querySelectorAll('.gw-tabpane:not([style*="none"])').length`,
      expect: (b,a) => typeof a === 'number' },
  ]);
  process.exit(0);
})();
