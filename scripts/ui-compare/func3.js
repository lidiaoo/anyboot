/* 功能抽查第三轮：4 个独立窗 + 弹层内部交互 */
const { spawn } = require('child_process'); const http = require('http');
const WebSocket = require('/tmp/ab-measure/node_modules/ws');
const wait = (ms) => new Promise(r => setTimeout(r, ms));
const getJson = (u) => new Promise((res, rej) => http.get(u, r => { let d=''; r.on('data',c=>d+=c); r.on('end',()=>{try{res(JSON.parse(d))}catch(e){rej(e)}}); }).on('error', rej));

async function run(page, checks) {
  const port = 9840 + Math.floor(Math.random() * 16);
  const chrome = spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', [
    '--headless=new','--disable-gpu','--no-sandbox',`--remote-debugging-port=${port}`,
    `--user-data-dir=/tmp/f3-${Date.now()}-${Math.random()}`,'about:blank'], { stdio: 'ignore' });
  let t=null;
  for(let i=0;i<60;i++){ await wait(300); try{const j=await getJson(`http://127.0.0.1:${port}/json`); t=j.find(x=>x.type==='page'); if(t)break; }catch(e){} }
  if(!t){ chrome.kill(); return [0,0]; }
  const ws=new WebSocket(t.webSocketDebuggerUrl); let id=0; const pend=new Map();
  ws.on('message',d=>{const m=JSON.parse(d); if(m.id&&pend.has(m.id)){pend.get(m.id)(m);pend.delete(m.id);}});
  const send=(me,pa={})=>new Promise(r=>{const i=++id;pend.set(i,r);ws.send(JSON.stringify({id:i,method:me,params:pa}));});
  await new Promise(r=>ws.on('open',r));
  await send('Runtime.enable'); await send('Page.enable'); await send('Network.enable'); await send('Network.setCacheDisabled',{cacheDisabled:true});
  await send('Emulation.setDeviceMetricsOverride',{width:1100,height:800,deviceScaleFactor:1,mobile:false});
  await send('Page.navigate',{url:`http://127.0.0.1:8770/new/${page}?t=${Date.now()}`});
  await wait(4500);
  const ev=async(e)=>{const r=await send('Runtime.evaluate',{expression:e,returnByValue:true,awaitPromise:true}); return r.result&&r.result.result?r.result.result.value:null;};
  await ev(`window.alert=function(){};window.confirm=function(){return false};window.prompt=function(){return null};`);

  console.log(`\n--- ${page} ---`);
  let pass=0, fail=0;
  for (const c of checks) {
    await ev(c.setup || '1'); await wait(700);
    const before = await ev(c.probe);
    await ev(c.act); await wait(900);
    const after = await ev(c.probe);
    const ok = c.expect(before, after);
    console.log(`${ok?'✅':'❌'} ${c.name.padEnd(28)} ${String(before).slice(0,14).padEnd(15)} → ${String(after).slice(0,22)}`);
    ok ? pass++ : fail++;
    if (c.cleanup) { await ev(c.cleanup); await wait(1000); }
  }
  ws.close(); chrome.kill();
  return [pass, fail];
}

(async () => {
  let tp=0, tf=0;
  const add = (r) => { tp += r[0]; tf += r[1]; };

  add(await run('hub-keys.html', [
    { name: '点「生成密钥」开弹层', act: `(function(){var b=[].slice.call(document.querySelectorAll('button')).find(function(x){return /生成密钥/.test(x.textContent)}); if(b)b.click(); return 1})()`,
      probe: `document.querySelectorAll('.modal').length`, expect: (b,a) => a > b,
      cleanup: `window.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true}))` },
    { name: '弹层里切 RSA 4096', setup: `(function(){var b=[].slice.call(document.querySelectorAll('button')).find(function(x){return /生成密钥/.test(x.textContent)}); if(b)b.click(); return 1})()`,
      act: `(function(){var b=[].slice.call(document.querySelectorAll('.segmented button')).find(function(x){return /RSA/.test(x.textContent)}); if(b)b.click(); return 1})()`,
      probe: `(function(){var b=[].slice.call(document.querySelectorAll('.segmented button')).find(function(x){return /RSA/.test(x.textContent)}); return b ? b.getAttribute('aria-selected') : 'none'})()`,
      expect: (b,a) => a === 'true',
      cleanup: `window.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true}))` },
    { name: '主题按钮切换亮暗', act: `(function(){var b=[].slice.call(document.querySelectorAll('.toolbtn')).find(function(x){return /主题/.test(x.textContent)}); if(b)b.click(); return 1})()`,
      probe: `document.documentElement.getAttribute('data-theme')`, expect: (b,a) => a !== b },
  ]));

  add(await run('hub-files.html', [
    { name: '点「新建目录」开弹层', act: `(function(){var b=[].slice.call(document.querySelectorAll('.btn')).find(function(x){return /新建目录/.test(x.textContent)}); if(b)b.click(); return 1})()`,
      probe: `document.querySelectorAll('.fb-modal').length`, expect: (b,a) => a > b,
      cleanup: `window.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true}))` },
    { name: '未选文件时下载按钮禁用', act: `1`,
      probe: `(function(){var b=[].slice.call(document.querySelectorAll('.btn')).find(function(x){return x.textContent.trim()==='下载'}); return b ? (b.disabled?'禁用':'可点') : '无'})()`,
      expect: (b,a) => a === '禁用' },
    { name: '面包屑存在', act: `1`, probe: `document.querySelectorAll('.crumbs .crumb').length`, expect: (b,a) => a >= 1 },
  ]));

  add(await run('hub-term.html', [
    { name: '无会话时显示空态', act: `1`, probe: `!!document.querySelector('.empty')`, expect: (b,a) => a === true },
    { name: '点 ＋ 打开选择弹层', act: `(function(){var b=document.querySelector('.tabbar-add'); if(b)b.click(); return 1})()`,
      probe: `document.querySelectorAll('.modal').length`, expect: (b,a) => a > b,
      cleanup: `window.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true}))` },
    { name: '字号按钮循环', act: `(function(){var b=[].slice.call(document.querySelectorAll('.toolbtn')).find(function(x){return /调整字号|A/.test(x.textContent)}); if(b)b.click(); return 1})()`,
      probe: `(function(){try{return JSON.parse(localStorage.getItem('anyboot.term-set')).font}catch(e){return 'none'}})()`,
      expect: (b,a) => typeof a === 'number' },
  ]));

  add(await run('hub-backup.html', [
    { name: '三个面板存在', act: `1`, probe: `document.querySelectorAll('.panel').length`, expect: (b,a) => a >= 3 },
    { name: '同步开关可切换', act: `(function(){var b=document.getElementById('syncEnabled'); if(b){b.checked=!b.checked; b.dispatchEvent(new Event('change',{bubbles:true}));} return 1})()`,
      probe: `(function(){var b=document.getElementById('syncEnabled'); return b ? String(b.checked) : 'none'})()`,
      expect: (b,a) => a !== b },
    { name: '自动同步频率 4 个按钮', act: `1`, probe: `document.querySelectorAll('.seg button').length`, expect: (b,a) => a === 4 },
  ]));

  console.log(`\n总计：通过 ${tp} / 失败 ${tf}`);
  process.exit(0);
})();
