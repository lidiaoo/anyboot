/*
 * 弹层比对：在新旧两版里打开同一个弹层，截图 + 比对结构。
 *
 *   node dialog-diff.js
 */
const { spawn } = require('child_process');
const http = require('http');
const fs = require('fs');
const WebSocket = require('/tmp/ab-measure/node_modules/ws');
const wait = (ms) => new Promise(r => setTimeout(r, ms));
const getJson = (u) => new Promise((res, rej) => http.get(u, r => { let d=''; r.on('data',c=>d+=c); r.on('end',()=>{try{res(JSON.parse(d))}catch(e){rej(e)}}); }).on('error', rej));

// 每个弹层：怎么打开它（在新版里的操作路径 / 在旧版里的 id）
const DIALOGS = [
  { name: 'editOverlay',   page: 'hub.html', tab: 'servers', open: `document.getElementById('addBtn')?.click()`, oldId: 'editOverlay' },
  { name: 'deleteOverlay', page: 'hub.html', tab: 'servers', open: `(function(){var b=[].slice.call(document.querySelectorAll('.server-row'));if(b[0])b[0].click();setTimeout(function(){var d=document.querySelector('.btn.ghost.danger, button.danger');if(d)d.click();},300);return 1})()`, oldId: 'deleteOverlay' },
  { name: 'gwPairOverlay', page: 'hub.html', tab: 'gateway', open: `(function(){var b=[].slice.call(document.querySelectorAll('.gw-tab'));var t=b.find(function(x){return /配对管理/.test(x.textContent)});if(t)t.click();setTimeout(function(){var p=[].slice.call(document.querySelectorAll('button')).find(function(x){return /签发新配对码/.test(x.textContent)});if(p)p.click();},300);return 1})()`, oldId: 'gwPairOverlay' },
  { name: 'fabHelpOverlay',page: 'hub.html', tab: 'home', open: `(function(){var ball=document.querySelector('.pc-fab-ball');if(ball)ball.click();setTimeout(function(){var b=document.querySelector('.pc-fab-help');if(b)b.click();},200);return 1})()`, oldId: 'fabHelpOverlay' },
  { name: 'cqViewOverlay', page: 'hub.html', tab: 'computers', open: `(function(){var b=[].slice.call(document.querySelectorAll('.cq-remote-act')).find(function(x){return /打开页面/.test(x.textContent)});if(b)b.click();return 1})()`, oldId: 'cqViewOverlay' },
  { name: 'tunnelOverlay', page: 'hub.html', tab: 'computers', open: `(function(){var b=[].slice.call(document.querySelectorAll('.link-btn')).find(function(x){return x.textContent.trim()==='隧道'});if(b)b.click();return 1})()`, oldId: 'tunnelOverlay' },
  { name: 'rsEditOverlay', page: 'hub.html', tab: 'computers', open: `(function(){var b=document.querySelector('#serverList .server-row')||document.querySelector('.server-row');return 'skip'})()`, oldId: 'rsEditOverlay' },
  { name: 'fbMkdirModal',  page: 'hub-files.html', tab: null, open: `(function(){var b=document.getElementById('fbMkdir')||[].slice.call(document.querySelectorAll('.btn')).find(function(x){return /新建目录/.test(x.textContent)});if(b)b.click();return 1})()`, oldId: 'fbMkdirModal' },
  { name: 'fbRenameModal', page: 'hub-files.html', tab: null, open: `(function(){var b=[].slice.call(document.querySelectorAll('.link-btn')).find(function(x){return /改名/.test(x.textContent)});if(b)b.click();return 1})()`, oldId: 'fbRenameModal' },
  { name: 'fbDelModal',    page: 'hub-files.html', tab: null, open: `(function(){var b=[].slice.call(document.querySelectorAll('.link-btn')).find(function(x){return /删除/.test(x.textContent)});if(b)b.click();return 1})()`, oldId: 'fbDelModal' },
  { name: 'fbInfoModal',   page: 'hub-files.html', tab: null, open: `(function(){var b=[].slice.call(document.querySelectorAll('.link-btn')).find(function(x){return /属性/.test(x.textContent)});if(b)b.click();return 1})()`, oldId: 'fbInfoModal' },
  { name: 'genModal',      page: 'hub-keys.html', tab: null, open: `(function(){var b=[].slice.call(document.querySelectorAll('button')).find(function(x){return /生成密钥/.test(x.textContent)});if(b)b.click();return 1})()`, oldId: 'genModal' },
  { name: 'gwUniOverlay',  page: 'hub.html', tab: 'gateway', open: `(function(){var b=[].slice.call(document.querySelectorAll('button')).find(function(x){return x.textContent.trim()==='卸载'});if(b)b.click();return 1})()`, oldId: 'gwUniOverlay' },
];

async function shot(which, d) {
  const port = 9970 + Math.floor(Math.random() * 25);
  const chrome = spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', [
    '--headless=new','--disable-gpu','--no-sandbox',`--remote-debugging-port=${port}`,
    `--user-data-dir=/tmp/dl-${Date.now()}-${Math.random()}`,'about:blank'], { stdio: 'ignore' });
  let t = null;
  for (let i = 0; i < 60; i++) { await wait(300);
    try { const j = await getJson(`http://127.0.0.1:${port}/json`); t = j.find(x => x.type === 'page'); if (t) break; } catch (e) {} }
  if (!t) { chrome.kill(); return null; }
  const ws = new WebSocket(t.webSocketDebuggerUrl); let id = 0; const pend = new Map(); const errs = [];
  ws.on('message', x => { const m = JSON.parse(x);
    if (m.method === 'Runtime.exceptionThrown') { const e = m.params.exceptionDetails; errs.push(String((e.exception && e.exception.description) || e.text).slice(0, 150)); }
    if (m.id && pend.has(m.id)) { pend.get(m.id)(m); pend.delete(m.id); } });
  const send = (me, pa = {}) => new Promise(r => { const i = ++id; pend.set(i, r); ws.send(JSON.stringify({ id: i, method: me, params: pa })); });
  await new Promise(r => ws.on('open', r));
  await send('Runtime.enable'); await send('Page.enable'); await send('Network.enable');
  await send('Network.setCacheDisabled', { cacheDisabled: true });
  await send('Emulation.setDeviceMetricsOverride', { width: 1280, height: 860, deviceScaleFactor: 1, mobile: false });
  await send('Page.navigate', { url: `http://127.0.0.1:8770/${which}/${d.page}?t=${Date.now()}` });
  await wait(4500);
  const ev = async (e) => { const r = await send('Runtime.evaluate', { expression: e, returnByValue: true, awaitPromise: true });
    return r.result && r.result.result ? r.result.result.value : null; };
  await ev(`window.alert=function(){};window.confirm=function(){return false};window.prompt=function(){return null};`);
  // 切页
  await ev(`(function(){var n=document.querySelector('[data-page="${d.tab}"]')||document.querySelector('[data-tab="${d.tab}"]'); if(n)n.click(); return 1;})()`);
  await wait(1200);
  // 打开弹层
  if (which === 'old') {
    // 旧版直接去掉 hidden 属性（比模拟点击稳）
    await ev(`(function(){var el=document.getElementById('${d.oldId}'); if(el){el.hidden=false; el.removeAttribute('hidden'); return 'shown';} return 'missing';})()`);
  } else {
    await ev(d.open);
  }
  await wait(1500);
  const info = await ev(`(function(){
    var vis = [].slice.call(document.querySelectorAll('.overlay,.token-modal-overlay,.fb-modal,.modal')).filter(function(el){
      var s = getComputedStyle(el); return s.display !== 'none' && el.getBoundingClientRect().height > 40;
    });
    if (!vis.length) return 'NO-DIALOG';
    var el = vis[0];
    var r = el.getBoundingClientRect();
    var dlg = el.querySelector('.dialog,.token-modal,.modal-card,.fb-modal-card') || el;
    var dr = dlg.getBoundingClientRect();
    return JSON.stringify({
      cls: el.className,
      outer: { w: Math.round(r.width), h: Math.round(r.height) },
      dialog: { w: Math.round(dr.width), h: Math.round(dr.height) },
      fields: dlg.querySelectorAll('input,select,textarea').length,
      buttons: dlg.querySelectorAll('button').length,
      labels: dlg.querySelectorAll('label').length,
      textLen: (dlg.textContent || '').trim().length,
    });
  })()`);
  const png = await send('Page.captureScreenshot', { format: 'png' });
  const dest = `/tmp/cmp/shots/dlg-${which}-${d.name}.png`;
  fs.writeFileSync(dest, Buffer.from(png.result.data, 'base64'));
  ws.close(); chrome.kill();
  return { info: info ? (info === 'NO-DIALOG' ? null : JSON.parse(info)) : null, errs, dest };
}

(async () => {
  console.log('弹层'.padEnd(18), '旧版(宽×高/控件)'.padEnd(26), '新版(宽×高/控件)');
  console.log('-'.repeat(78));
  for (const d of DIALOGS) {
    const a = await shot('old', d);
    const b = await shot('new', d);
    const f = (x) => x?.info ? `${x.info.dialog.w}x${x.info.dialog.h} / ${x.info.fields}填${x.info.buttons}钮` : '未打开';
    const flag = (!b?.info) ? '❌空' : (b.errs.length ? '⚠️异常' : '✅');
    console.log(d.name.padEnd(18), f(a).padEnd(26), f(b), flag);
    if (b?.errs.length) console.log('    异常:', b.errs[0]);
  }
  process.exit(0);
})();
