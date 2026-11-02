/*
 * 计算样式比对：对「新旧两版都存在的同名 class 元素」逐项比较
 * getComputedStyle 的关键属性（尺寸/颜色/字体/边框/间距）。
 *
 *   node style-diff.js
 */
const { spawn } = require('child_process');
const http = require('http');
const WebSocket = require('/tmp/ab-measure/node_modules/ws');
const wait = (ms) => new Promise(r => setTimeout(r, ms));
const getJson = (u) => new Promise((res, rej) => http.get(u, r => { let d=''; r.on('data',c=>d+=c); r.on('end',()=>{try{res(JSON.parse(d))}catch(e){rej(e)}}); }).on('error', rej));

// 只比「样式意图」类属性；width/height/margin 受内容与布局影响，单独看会更准
const PROPS = ['backgroundColor','color','fontSize','fontWeight','borderRadius',
               'borderTopWidth','borderTopColor','display','flexDirection','gap',
               'lineHeight','textAlign','opacity'];

async function grab(which, page, tab) {
  const port = 9990 + Math.floor(Math.random() * 20);
  const chrome = spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', [
    '--headless=new','--disable-gpu','--no-sandbox',`--remote-debugging-port=${port}`,
    `--user-data-dir=/tmp/sd-${Date.now()}-${Math.random()}`,'about:blank'], { stdio: 'ignore' });
  let t = null;
  for (let i = 0; i < 60; i++) { await wait(300);
    try { const j = await getJson(`http://127.0.0.1:${port}/json`); t = j.find(x => x.type === 'page'); if (t) break; } catch (e) {} }
  if (!t) { chrome.kill(); return {}; }
  const ws = new WebSocket(t.webSocketDebuggerUrl); let id = 0; const pend = new Map();
  ws.on('message', d => { const m = JSON.parse(d); if (m.id && pend.has(m.id)) { pend.get(m.id)(m); pend.delete(m.id); } });
  const send = (me, pa = {}) => new Promise(r => { const i = ++id; pend.set(i, r); ws.send(JSON.stringify({ id: i, method: me, params: pa })); });
  await new Promise(r => ws.on('open', r));
  await send('Runtime.enable'); await send('Page.enable'); await send('Network.enable');
  await send('Network.setCacheDisabled', { cacheDisabled: true });
  await send('Emulation.setDeviceMetricsOverride', { width: 1280, height: 860, deviceScaleFactor: 1, mobile: false });
  await send('Page.navigate', { url: `http://127.0.0.1:8770/${which}/${page}?t=${Date.now()}` });
  await wait(4500);
  const ev = async (e) => { const r = await send('Runtime.evaluate', { expression: e, returnByValue: true, awaitPromise: true });
    return r.result && r.result.result ? r.result.result.value : null; };
  if (tab) { await ev(`(function(){var n=document.querySelector('[data-page="${tab}"]')||document.querySelector('[data-tab="${tab}"]'); if(n)n.click(); return 1;})()`); await wait(1200); }
  const data = await ev(`(function(){
    var out = {};
    document.querySelectorAll('*').forEach(function(el){
      if (el.closest('[hidden]')) return;
      // 表头 / 打印专用等「存在但不可见」的容器内的元素不参与比对（它们的样式本就不同）
      if (el.closest('.cq-thead, .cq-sr-only, [aria-hidden="true"]')) return;
      var st = getComputedStyle(el);
      if (st.display === 'none') return;
      var cls = (el.className && typeof el.className === 'string') ? el.className.trim().split(/\\s+/) : [];
      if (!cls.length) return;
      var key = cls.sort().join('.');
      // 只记录「真正参与布局」的元素：display:none 的祖先链已过滤，
      // 这里再排除 width/height 都为 0 的隐藏容器，避免旧版隐藏区被当代表
      var rect = el.getBoundingClientRect();
      if (rect.width === 0 && rect.height === 0 && st.position !== 'fixed') return;
      if (out[key]) return; // 同类只取第一个可见的
      var rec = {};
      ${JSON.stringify(PROPS)}.forEach(function(p){ rec[p] = st[p]; });
      out[key] = rec;
    });
    return JSON.stringify(out);
  })()`);
  ws.close(); chrome.kill();
  return data ? JSON.parse(data) : {};
}

(async () => {
  const jobs = [
    ['hub.html', ['computers','servers','gateway','files','keys','termset','backup','about','transfers']],
    ['index.html', ['boot','settings','sol']],
    ['hub-keys.html', [null]],
    ['hub-files.html', [null]],
    ['hub-backup.html', [null]],
    ['hub-term.html', [null]],
  ];
  let totalMismatch = 0, totalCompared = 0;
  for (const [page, tabs] of jobs) {
    for (const tab of tabs) {
      const a = await grab('old', page, tab);
      const b = await grab('new', page, tab);
      const common = Object.keys(a).filter(k => b[k]);
      let bad = [];
      for (const key of common) {
        const diffs = [];
        for (const p of PROPS) {
          if (a[key][p] !== b[key][p]) diffs.push(`${p}: ${a[key][p]} → ${b[key][p]}`);
        }
        if (diffs.length) bad.push({ key, diffs });
      }
      totalCompared += common.length;
      totalMismatch += bad.length;
      const pct = common.length ? Math.round((1 - bad.length / common.length) * 100) : 100;
      console.log(`${page}:${tab}  共同类 ${common.length}  不一致 ${bad.length}  (${pct}% 一致)`);
      // 只打印差异最大的前 5 条
      bad.slice(0, 5).forEach(x => console.log(`    ${x.key}: ${x.diffs.slice(0, 3).join(' | ')}`));
    }
  }
  console.log(`\n总计：比对 ${totalCompared} 个类，不一致 ${totalMismatch} 个`);
  process.exit(0);
})();
