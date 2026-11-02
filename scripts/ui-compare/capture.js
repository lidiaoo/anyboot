/*
 * 无头浏览器截图 + DOM 度量对比。
 *
 *   node capture.js old <page> [width] [height]
 *   node capture.js new <page> [width] [height]
 *   node capture.js diff <page>          → 并排对比 + 差异报告
 */
const { spawn } = require('child_process');
const http = require('http');
const fs = require('fs');
const path = require('path');
const WebSocket = require('/tmp/ab-measure/node_modules/ws');

const PORT = 9660;
const wait = (ms) => new Promise((r) => setTimeout(r, ms));
const getJson = (u) => new Promise((res, rej) =>
  http.get(u, (r) => { let d=''; r.on('data', c => d += c); r.on('end', () => { try { res(JSON.parse(d)); } catch (e) { rej(e); } }); }).on('error', rej));

async function shot(which, page, width = 1280, height = 820, outPath) {
  const chrome = spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', [
    '--headless=new', '--disable-gpu', '--no-sandbox', `--remote-debugging-port=${PORT}`,
    `--user-data-dir=/tmp/cmp-chrome-${Date.now()}`, 'about:blank'
  ], { stdio: 'ignore' });

  let target = null;
  for (let i = 0; i < 60; i++) {
    await wait(400);
    try { const t = await getJson(`http://127.0.0.1:${PORT}/json`); target = t.find(x => x.type === 'page'); if (target) break; } catch (e) {}
  }
  if (!target) { chrome.kill(); throw new Error('chrome 没起来'); }

  const ws = new WebSocket(target.webSocketDebuggerUrl);
  let id = 0; const pend = new Map(); const errors = [];
  ws.on('message', (d) => {
    const m = JSON.parse(d);
    if (m.method === 'Runtime.exceptionThrown') errors.push(String(m.params.exceptionDetails.text || '').slice(0, 160));
    if (m.id && pend.has(m.id)) { pend.get(m.id)(m); pend.delete(m.id); }
  });
  const send = (method, params = {}) => new Promise((res) => { const i = ++id; pend.set(i, res); ws.send(JSON.stringify({ id: i, method, params })); });
  await new Promise((r) => ws.on('open', r));

  await send('Runtime.enable'); await send('Page.enable');
  await send('Network.enable');
  await send('Network.setCacheDisabled', { cacheDisabled: true });
  await send('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: 1, mobile: false });
  await send('Page.navigate', { url: `http://127.0.0.1:8770/${which}/${page}?t=${Date.now()}` });
  await wait(6000);

  const ev = async (expr) => {
    const r = await send('Runtime.evaluate', { expression: expr, returnByValue: true, awaitPromise: true });
    if (r.result && r.result.exceptionDetails) return null;
    return r.result && r.result.result ? r.result.result.value : null;
  };

  // 度量：关键容器 + 首个可见 page
  const metrics = await ev(`(function(){
    function box(sel){
      var el = document.querySelector(sel);
      if (!el) return null;
      var r = el.getBoundingClientRect();
      var s = getComputedStyle(el);
      return { w: Math.round(r.width), h: Math.round(r.height), x: Math.round(r.left), y: Math.round(r.top),
               bg: s.backgroundColor, border: s.borderTopWidth, pad: s.padding, display: s.display };
    }
    var out = {};
    ['.shell','.side','.side-nav','.snav','.content','.page',
     '.cq-list-view','.cq-head','.cq-toolbar','.cq-card','.cq-grid','.cq-thead','.cq-rows','.cq-row',
     'body','footer','#app','.system-footer','.toolbar','.cols','.session-list','.tx-list','.fb-topbar','.act-card','.ts-wrap','.about-wrap'
    ].forEach(function(sel){ out[sel] = box(sel); });
    out.__visiblePages = [].slice.call(document.querySelectorAll('.page')).filter(function(p){
      var s = getComputedStyle(p); return s.display !== 'none' && !p.hidden;
    }).map(function(p){ return p.id || '(no-id)'; });
    return JSON.stringify(out);
  })()`);

  const png = await send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false });
  const dest = outPath || `/tmp/cmp/shots/${which}-${page.replace(/\.html$/, '')}-${width}.png`;
  fs.mkdirSync(path.dirname(dest), { recursive: true });
  fs.writeFileSync(dest, Buffer.from(png.result.data, 'base64'));

  ws.close(); chrome.kill();
  return { metrics: metrics ? JSON.parse(metrics) : null, errors, file: dest };
}

(async () => {
  const [mode, page = 'hub.html', w = '1280', h = '820'] = process.argv.slice(2);
  const width = parseInt(w, 10), height = parseInt(h, 10);

  if (mode === 'old' || mode === 'new') {
    const r = await shot(mode, page, width, height);
    console.log(JSON.stringify(r, null, 1));
    process.exit(0);
  }

  if (mode === 'diff') {
    const a = await shot('old', page, width, height, `/tmp/cmp/shots/old-${page.replace(/\.html$/, '')}.png`);
    const b = await shot('new', page, width, height, `/tmp/cmp/shots/new-${page.replace(/\.html$/, '')}.png`);
    console.log('=== 旧版 ===', a.file, '异常:', a.errors.length);
    console.log(JSON.stringify(a.metrics, null, 1));
    console.log('=== 新版 ===', b.file, '异常:', b.errors.length);
    console.log(JSON.stringify(b.metrics, null, 1));
    process.exit(0);
  }
  console.log('用法: node capture.js old|new|diff <page> [width] [height]');
})();
