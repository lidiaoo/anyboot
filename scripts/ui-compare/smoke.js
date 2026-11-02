/*
 * 运行时冒烟测试：连到真实应用窗口（或浏览器对比环境），
 * 依次点每页的按钮，抓 IPC 失败与 JS 异常。
 *
 *   node smoke.js            # 对 mock 环境（对比服务器）跑
 */
const { spawn } = require('child_process');
const http = require('http');
const WebSocket = require('/tmp/ab-measure/node_modules/ws');
const wait = (ms) => new Promise(r => setTimeout(r, ms));
const getJson = (u) => new Promise((res, rej) => http.get(u, r => { let d=''; r.on('data',c=>d+=c); r.on('end',()=>{try{res(JSON.parse(d))}catch(e){rej(e)}}); }).on('error', rej));

async function run(which, page, tabs) {
  const port = 9950 + Math.floor(Math.random() * 40);
  const chrome = spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', [
    '--headless=new','--disable-gpu','--no-sandbox',`--remote-debugging-port=${port}`,
    `--user-data-dir=/tmp/sm-${Date.now()}-${Math.random()}`,'about:blank'], { stdio: 'ignore' });
  let t = null;
  for (let i = 0; i < 60; i++) { await wait(300);
    try { const j = await getJson(`http://127.0.0.1:${port}/json`); t = j.find(x => x.type === 'page'); if (t) break; } catch (e) {} }
  if (!t) { chrome.kill(); return []; }
  const ws = new WebSocket(t.webSocketDebuggerUrl); let id = 0; const pend = new Map(); const errs = [];
  ws.on('message', d => { const m = JSON.parse(d);
    if (m.method === 'Runtime.exceptionThrown') {
      const d = m.params.exceptionDetails;
      const desc = d.exception && (d.exception.description || d.exception.value);
      errs.push('EXC ' + String(desc || d.text || '').slice(0, 400));
    }
    if (m.method === 'Runtime.consoleAPICalled' && m.params.type === 'error') {
      errs.push('CONSOLE ' + (m.params.args || []).map(a => String(a.value || a.description || '')).join(' ').slice(0, 150));
    }
    if (m.id && pend.has(m.id)) { pend.get(m.id)(m); pend.delete(m.id); } });
  const send = (me, pa = {}) => new Promise(r => { const i = ++id; pend.set(i, r); ws.send(JSON.stringify({ id: i, method: me, params: pa })); });
  await new Promise(r => ws.on('open', r));
  await send('Runtime.enable'); await send('Page.enable'); await send('Network.enable');
  // 用 Runtime.evaluate 覆盖原生对话框：否则点「删除」类按钮会阻塞脚本
  await send('Runtime.evaluate', { expression: `
    window.alert = function(){};
    window.confirm = function(){ return false; };
    window.prompt = function(){ return null; };
  ` });
  await send('Network.setCacheDisabled', { cacheDisabled: true });
  await send('Emulation.setDeviceMetricsOverride', { width: 1280, height: 860, deviceScaleFactor: 1, mobile: false });
  await send('Page.navigate', { url: `http://127.0.0.1:8770/${which}/${page}?t=${Date.now()}` });
  await wait(4500);
  const ev = async (e) => { const r = await send('Runtime.evaluate', { expression: e, returnByValue: true, awaitPromise: true });
    return r.result && r.result.result ? r.result.result.value : null; };

  for (const tab of tabs) {
    if (tab) { await ev(`(function(){var n=document.querySelector('[data-page="${tab}"]')||document.querySelector('[data-tab="${tab}"]'); if(n)n.click(); return 1;})()`); await wait(1000); }
    // 点当前可见页里所有 button（跳过危险的卸载/删除/退出类）
    const clicked = await Promise.race([
      ev(`(function(){
      var SKIP = /卸载|删除|退出|放弃|清空|重置|wipe|uninstall|delete|quit|abandon/i;
      var root = document.querySelector('.page:not([hidden])') || document.querySelector('.tab-panel.active') || document.body;
      var btns = [].slice.call(root.querySelectorAll('button'));
      var n = 0;
      btns.forEach(function(b){
        var t = (b.textContent||'').trim();
        if (SKIP.test(t)) return;
        try { b.click(); n++; } catch(e) {}
      });
      return n;
    })()`),
      new Promise(r => setTimeout(() => r('TIMEOUT'), 8000)),
    ]);
    await wait(900);
    errs.push(`  [${tab || 'default'}] 点击 ${clicked} 个按钮`);
  }
  ws.close(); chrome.kill();
  return errs;
}

(async () => {
  const only = process.env.ONLY; // 例：ONLY=hub.html:computers
  let jobs = [
    ['new', 'hub.html', ['computers','servers','gateway','term','sessions','transfers','files','keys','termset','backup','about']],
    ['new', 'index.html', ['boot','adv-boot','settings','sol']],
  ];
  if (only) {
    const [pg, tab] = only.split(':');
    jobs = jobs.filter(j => j[1] === pg).map(j => [j[0], j[1], [tab || null]]);
  }
  let total = 0;
  for (const [which, page, tabs] of jobs) {
    const errs = await run(which, page, tabs);
    const real = errs.filter(e => !e.startsWith('  ['));
    console.log(`\n=== ${page}: ${real.length} 个异常`);
    for (const e of errs) console.log('   ' + e);
    total += real.length;
  }
  console.log(`\n总计异常: ${total}`);
  process.exit(0);
})();
