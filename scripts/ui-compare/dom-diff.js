/*
 * 深度 DOM 比对：对每个页签，列出「旧版有、新版没有」的 class / id。
 *
 *   node dom-diff.js
 */
const { spawn } = require('child_process');
const http = require('http');
const WebSocket = require('/tmp/ab-measure/node_modules/ws');
const wait = (ms) => new Promise(r => setTimeout(r, ms));
const getJson = (u) => new Promise((res, rej) => http.get(u, r => { let d=''; r.on('data',c=>d+=c); r.on('end',()=>{try{res(JSON.parse(d))}catch(e){rej(e)}}); }).on('error', rej));

const PAGES = [
  ['hub.html', ['computers','servers','gateway','term','sessions','transfers','files','keys','termset','backup','about']],
  ['index.html', ['boot','adv-boot','settings','sol']],
];

async function collect(which, page, tab, width = 1280, height = 820) {
  const port = 9900 + Math.floor(Math.random() * 90);
  const chrome = spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', [
    '--headless=new','--disable-gpu','--no-sandbox',`--remote-debugging-port=${port}`,
    `--user-data-dir=/tmp/dd-${Date.now()}-${Math.random()}`,'about:blank'], { stdio: 'ignore' });
  let t = null;
  for (let i = 0; i < 60; i++) { await wait(300);
    try { const j = await getJson(`http://127.0.0.1:${port}/json`); t = j.find(x => x.type === 'page'); if (t) break; } catch (e) {} }
  if (!t) { chrome.kill(); return { classes: [], ids: [] }; }
  const ws = new WebSocket(t.webSocketDebuggerUrl); let id = 0; const pend = new Map();
  ws.on('message', d => { const m = JSON.parse(d); if (m.id && pend.has(m.id)) { pend.get(m.id)(m); pend.delete(m.id); } });
  const send = (me, pa = {}) => new Promise(r => { const i = ++id; pend.set(i, r); ws.send(JSON.stringify({ id: i, method: me, params: pa })); });
  await new Promise(r => ws.on('open', r));
  await send('Runtime.enable'); await send('Page.enable'); await send('Network.enable');
  await send('Network.setCacheDisabled', { cacheDisabled: true });
  await send('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: 1, mobile: false });
  await send('Page.navigate', { url: `http://127.0.0.1:8770/${which}/${page}?t=${Date.now()}` });
  await wait(4500);
  const ev = async (e) => { const r = await send('Runtime.evaluate', { expression: e, returnByValue: true, awaitPromise: true });
    return r.result && r.result.result ? r.result.result.value : null; };
  if (tab) { await ev(`(function(){var n=document.querySelector('[data-page="${tab}"]')||document.querySelector('[data-tab="${tab}"]'); if(n)n.click(); return 1;})()`); await wait(1200); }
  const data = await ev(`(function(){
    var cls = {}, ids = {};
    document.querySelectorAll('*').forEach(function(el){
      // 跳过 hidden 属性与 display:none 的整个子树（旧版用 hidden 藏页，
      // 新版用 v-if 不挂载 —— 必须同口径比较）
      if (el.closest('[hidden]')) return;
      var st = getComputedStyle(el);
      if (st.display === 'none') return;
      (el.classList||[]).forEach(function(c){ cls[c] = (cls[c]||0)+1; });
      if (el.id) ids[el.id] = 1;
    });
    return JSON.stringify({ classes: Object.keys(cls), ids: Object.keys(ids) });
  })()`);
  ws.close(); chrome.kill();
  return data ? JSON.parse(data) : { classes: [], ids: [] };
}

// 忽略 Vue 自己加的 scope 属性类与纯装饰 id
const IGN = /^(data-v-|_)/;

(async () => {
  for (const [page, tabs] of PAGES) {
    for (const tab of tabs) {
      const a = await collect('old', page, tab);
      const b = await collect('new', page, tab);
      const missCls = a.classes.filter(c => !b.classes.includes(c) && !IGN.test(c));
      const missIds = a.ids.filter(i => !b.ids.includes(i) && !IGN.test(i));
      if (!missCls.length && !missIds.length) { console.log(`${page}:${tab}  ✅ 完全覆盖`); continue; }
      console.log(`\n${page}:${tab}  ❌ 缺 ${missCls.length} 类 / ${missIds.length} id`);
      if (missCls.length) console.log('   缺类:', missCls.slice(0, 30).join(' '));
      if (missIds.length) console.log('   缺id:', missIds.slice(0, 30).join(' '));
    }
  }
  process.exit(0);
})();
