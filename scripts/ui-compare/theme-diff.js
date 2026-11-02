/*
 * 主题比对：主窗 6 套主题（A/B/C/G/I/J）+ K 原生态，
 * 在新旧两版里各渲染一遍，比对结构 + 截图。
 *
 * 旧版通过 ?abTheme= 指定主题；新版通过 localStorage 里的 bootAdv 主题字段。
 */
const { spawn } = require('child_process'); const http = require('http'); const fs = require('fs');
const WebSocket = require('/tmp/ab-measure/node_modules/ws');
const wait = (ms) => new Promise(r => setTimeout(r, ms));
const getJson = (u) => new Promise((res, rej) => http.get(u, r => { let d=''; r.on('data',c=>d+=c); r.on('end',()=>{try{res(JSON.parse(d))}catch(e){rej(e)}}); }).on('error', rej));

async function shot(which, theme) {
  const port = 9940 + Math.floor(Math.random() * 18);
  const chrome = spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', [
    '--headless=new','--disable-gpu','--no-sandbox',`--remote-debugging-port=${port}`,
    `--user-data-dir=/tmp/th-${Date.now()}-${Math.random()}`,'about:blank'], { stdio: 'ignore' });
  let t=null;
  for(let i=0;i<60;i++){ await wait(300); try{const j=await getJson(`http://127.0.0.1:${port}/json`); t=j.find(x=>x.type==='page'); if(t)break; }catch(e){} }
  if(!t){ chrome.kill(); return null; }
  const ws=new WebSocket(t.webSocketDebuggerUrl); let id=0; const pend=new Map(); const errs=[];
  ws.on('message',d=>{const m=JSON.parse(d);
    if(m.method==='Runtime.exceptionThrown'){const e=m.params.exceptionDetails; errs.push(String((e.exception&&e.exception.description)||e.text).slice(0,150));}
    if(m.id&&pend.has(m.id)){pend.get(m.id)(m);pend.delete(m.id);}});
  const send=(me,pa={})=>new Promise(r=>{const i=++id;pend.set(i,r);ws.send(JSON.stringify({id:i,method:me,params:pa}));});
  await new Promise(r=>ws.on('open',r));
  await send('Runtime.enable'); await send('Page.enable'); await send('Network.enable'); await send('Network.setCacheDisabled',{cacheDisabled:true});
  await send('Emulation.setDeviceMetricsOverride',{width:1280,height:860,deviceScaleFactor:1,mobile:false});
  // 先设主题（新版用 localStorage，旧版用 URL 参数）
  if (which === 'new') {
    await send('Page.navigate', { url: `http://127.0.0.1:8770/new/index.html?t=${Date.now()}` });
    await wait(1200);
    await send('Runtime.evaluate', { expression: `
      (function(){
        var raw = localStorage.getItem('anyboot.boot-adv') || '{}';
        var o; try { o = JSON.parse(raw); } catch(e) { o = {}; }
        o.theme = '${theme}';
        localStorage.setItem('anyboot.boot-adv', JSON.stringify(o));
        return 1;
      })()` });
    await send('Page.reload');
  } else {
    await send('Page.navigate', { url: `http://127.0.0.1:8770/old/index.html?abTheme=${theme}&t=${Date.now()}` });
  }
  await wait(5000);
  const ev=async(e)=>{const r=await send('Runtime.evaluate',{expression:e,returnByValue:true,awaitPromise:true}); return r.result&&r.result.result?r.result.result.value:null;};
  const info = await ev(`(function(){
    var root = document.querySelector('.ba-theme-root, .la, .lb, .lc, .lg, .li, .lj, #tab-boot');
    if (!root) return JSON.stringify({found:false});
    return JSON.stringify({
      found: true,
      rootCls: (typeof root.className === 'string' ? root.className : '').slice(0, 60),
      elements: root.querySelectorAll('*').length,
      buttons: root.querySelectorAll('button').length,
      textLen: (root.textContent||'').replace(/\\s+/g,' ').trim().length,
      head: (root.textContent||'').replace(/\\s+/g,' ').trim().slice(0, 40),
    });
  })()`);
  const png = await send('Page.captureScreenshot',{format:'png'});
  const dest = `/tmp/cmp/shots/theme-${which}-${theme}.png`;
  fs.writeFileSync(dest, Buffer.from(png.result.data,'base64'));
  ws.close(); chrome.kill();
  return { info: info ? JSON.parse(info) : null, errs, dest };
}

(async () => {
  const THEMES = ['A','B','C','G','I','J'];
  console.log('主题  旧版(元素/按钮/文字)          新版(元素/按钮/文字)         判定');
  console.log('-'.repeat(82));
  for (const th of THEMES) {
    const a = await shot('old', th);
    const b = await shot('new', th);
    const f = (x) => x?.info?.found ? `${x.info.elements}/${x.info.buttons}/${x.info.textLen}` : '未找到主题根';
    const bad = !b?.info?.found || b.errs.length > 0;
    console.log(th.padEnd(5), f(a).padEnd(28), f(b).padEnd(28), bad ? '❌' : '✅');
    if (b?.errs?.length) console.log('   异常:', b.errs[0]);
    if (a?.info?.found && b?.info?.found) {
      const ratio = a.info.elements ? (b.info.elements / a.info.elements).toFixed(2) : '—';
      console.log(`      根元素: 旧=${a.info.rootCls} 新=${b.info.rootCls}  元素比=${ratio}`);
    }
  }
  process.exit(0);
})();
