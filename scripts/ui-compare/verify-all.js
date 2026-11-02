const { spawn } = require('child_process');
const http = require('http');
const fs = require('fs');
const WebSocket = require('/tmp/ab-measure/node_modules/ws');
const wait = (ms) => new Promise(r => setTimeout(r, ms));
const getJson = (u) => new Promise((res, rej) => http.get(u, r => { let d=''; r.on('data',c=>d+=c); r.on('end',()=>{try{res(JSON.parse(d))}catch(e){rej(e)}}); }).on('error', rej));

const PAGES = ['hub.html', 'index.html', 'hub-term.html', 'hub-files.html', 'hub-backup.html', 'hub-keys.html'];
const TABS = [null, 'computers', 'servers', 'gateway', 'term', 'sessions', 'transfers', 'files', 'keys', 'termset', 'backup', 'about'];
const MAIN_TABS = ['boot', 'adv-boot', 'settings', 'sol'];

async function grab(which, page, tab, width, height) {
  const port = 9700 + Math.floor(Math.random() * 200);
  const chrome = spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', [
    '--headless=new','--disable-gpu','--no-sandbox',`--remote-debugging-port=${port}`,
    `--user-data-dir=/tmp/va-${Date.now()}-${Math.random()}`, 'about:blank'], { stdio: 'ignore' });
  let t = null;
  for (let i = 0; i < 60; i++) { await wait(300);
    try { const j = await getJson(`http://127.0.0.1:${port}/json`); t = j.find(x => x.type === 'page'); if (t) break; } catch (e) {} }
  if (!t) { chrome.kill(); return null; }
  const ws = new WebSocket(t.webSocketDebuggerUrl); let id = 0; const pend = new Map(); const errs = [];
  ws.on('message', d => { const m = JSON.parse(d);
    if (m.method === 'Runtime.exceptionThrown') errs.push(String(m.params.exceptionDetails.text || '').slice(0, 100));
    if (m.id && pend.has(m.id)) { pend.get(m.id)(m); pend.delete(m.id); } });
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
  const info = await ev(`(function(){
    var p = document.querySelector('.page:not([hidden])') || document.querySelector('.tab-panel.active') || document.querySelector('.tab-panel:not([hidden])');
    var r = p ? p.getBoundingClientRect() : null;
    return JSON.stringify({
      visiblePanel: p ? (p.id || p.className) : 'none',
      w: r ? Math.round(r.width) : 0, h: r ? Math.round(r.height) : 0,
      childCount: p ? p.querySelectorAll('*').length : 0,
      htmlLen: document.body.innerHTML.length,
    });
  })()`);
  ws.close(); chrome.kill();
  return { info: info ? JSON.parse(info) : null, errors: errs };
}

(async () => {
  console.log('页面'.padEnd(22), '旧版元素数'.padEnd(12), '新版元素数'.padEnd(12), '异常');
  console.log('-'.repeat(60));
  let mismatches = 0;
  for (const page of PAGES) {
    const tabs = page === 'hub.html' ? TABS : (page === 'index.html' ? MAIN_TABS : [null]);
    for (const tab of tabs) {
      const a = await grab('old', page, tab, 1280, 820);
      const b = await grab('new', page, tab, 1280, 820);
      const ca = a?.info?.childCount ?? 0, cb = b?.info?.childCount ?? 0;
      const label = `${page}${tab ? ':' + tab : ''}`;
      const bad = (b?.errors?.length ?? 0) > 0;
      const ratio = ca ? (cb / ca) : 1;
      const flag = bad ? '❌异常' : (ratio < 0.3 ? '⚠️缺元素' : '✅');
      if (flag !== '✅') mismatches++;
      console.log(label.padEnd(22), String(ca).padEnd(12), String(cb).padEnd(12), flag + (b?.errors?.length ? ' ' + b.errors[0] : ''));
    }
  }
  console.log('-'.repeat(60));
  console.log(`需要关注: ${mismatches} 项`);
  process.exit(0);
})();
