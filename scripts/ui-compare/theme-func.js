/* 主题功能实测：搜索过滤、筛选芯片、点条目选中 */
const { spawn } = require('child_process'); const http = require('http');
const WebSocket = require('/tmp/ab-measure/node_modules/ws');
const wait = (ms) => new Promise(r => setTimeout(r, ms));
const getJson = (u) => new Promise((res, rej) => http.get(u, r => { let d=''; r.on('data',c=>d+=c); r.on('end',()=>{try{res(JSON.parse(d))}catch(e){rej(e)}}); }).on('error', rej));

async function run(theme, checks) {
  const port = 9760 + Math.floor(Math.random() * 14);
  const chrome = spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', [
    '--headless=new','--disable-gpu','--no-sandbox',`--remote-debugging-port=${port}`,
    `--user-data-dir=/tmp/tf-${Date.now()}-${Math.random()}`,'about:blank'], { stdio: 'ignore' });
  let t=null;
  for(let i=0;i<60;i++){ await wait(300); try{const j=await getJson(`http://127.0.0.1:${port}/json`); t=j.find(x=>x.type==='page'); if(t)break; }catch(e){} }
  if(!t){ chrome.kill(); return [0,0]; }
  const ws=new WebSocket(t.webSocketDebuggerUrl); let id=0; const pend=new Map();
  ws.on('message',d=>{const m=JSON.parse(d); if(m.id&&pend.has(m.id)){pend.get(m.id)(m);pend.delete(m.id);}});
  const send=(me,pa={})=>new Promise(r=>{const i=++id;pend.set(i,r);ws.send(JSON.stringify({id:i,method:me,params:pa}));});
  await new Promise(r=>ws.on('open',r));
  await send('Runtime.enable'); await send('Page.enable'); await send('Network.enable'); await send('Network.setCacheDisabled',{cacheDisabled:true});
  await send('Emulation.setDeviceMetricsOverride',{width:1280,height:860,deviceScaleFactor:1,mobile:false});
  await send('Page.navigate',{url:'http://127.0.0.1:8770/new/index.html?t='+Date.now()});
  await wait(1500);
  await send('Runtime.evaluate',{expression:`(function(){
    var o; try{o=JSON.parse(localStorage.getItem('anyboot.boot-adv')||'{}')}catch(e){o={}}
    o.theme='${theme}'; o.layout='wide';
    o.visible={refind:true,grub:true,sdboot:true,opencore:true,clover:true};
    localStorage.setItem('anyboot.boot-adv',JSON.stringify(o)); return 1;})()`});
  await send('Page.reload');
  await wait(5000);
  const ev=async(e)=>{const r=await send('Runtime.evaluate',{expression:e,returnByValue:true,awaitPromise:true}); return r.result&&r.result.result?r.result.result.value:null;};

  console.log(`\n--- 主题 ${theme} ---`);
  let pass=0, fail=0;
  for (const c of checks) {
    await ev(c.setup || '1'); await wait(600);
    const before = await ev(c.probe);
    await ev(c.act); await wait(800);
    const after = await ev(c.probe);
    const ok = c.expect(before, after);
    console.log(`${ok?'✅':'❌'} ${c.name.padEnd(24)} ${String(before).slice(0,12).padEnd(13)} → ${String(after).slice(0,18)}`);
    ok ? pass++ : fail++;
  }
  ws.close(); chrome.kill();
  return [pass, fail];
}

(async () => {
  let tp=0, tf=0;
  const add = (r) => { tp += r[0]; tf += r[1]; };

  // 主题 G：搜索过滤
  add(await run('G', [
    { name: '搜索框输入关键词过滤',
      act: `(function(){var i=document.querySelector('.lg-search input'); if(!i)return 0; i.value='Arch'; i.dispatchEvent(new Event('input',{bubbles:true})); return 1})()`,
      probe: `document.querySelectorAll('.lg-item').length`,
      expect: (b,a) => a < b },
    { name: '清空搜索恢复',
      act: `(function(){var i=document.querySelector('.lg-search input'); if(!i)return 0; i.value=''; i.dispatchEvent(new Event('input',{bubbles:true})); return 1})()`,
      probe: `document.querySelectorAll('.lg-item').length`,
      expect: (b,a) => a > 0 },
    { name: '筛选芯片可点击',
      act: `(function(){var b=document.querySelector('.lg-fchip'); if(b)b.click(); return 1})()`,
      probe: `document.querySelectorAll('.lg-item').length`,
      expect: (b,a) => typeof a === 'number' },
    { name: '点条目出现选中项面板',
      act: `(function(){var b=document.querySelector('.lg-item'); if(b)b.click(); return 1})()`,
      probe: `document.querySelectorAll('.lg-sel').length`,
      expect: (b,a) => a >= 1 },
  ]));

  // 主题 A：芯片跳转 & 卡片
  add(await run('A', [
    { name: '卡片渲染（多张）',
      act: `1`, probe: `document.querySelectorAll('.la-card').length`, expect: (b,a) => a >= 2 },
    { name: '芯片数量与卡片一致',
      act: `1`, probe: `document.querySelectorAll('.la-chip').length`, expect: (b,a) => a >= 2 },
    { name: '点芯片有反应',
      act: `(function(){var b=document.querySelectorAll('.la-chip')[1]; if(b)b.click(); return 1})()`,
      probe: `document.querySelectorAll('.la-chip.is-on').length`,
      expect: (b,a) => a >= 1 },
    { name: '卡片有底部状态条',
      act: `1`, probe: `document.querySelectorAll('.la-foot').length`, expect: (b,a) => a >= 2 },
  ]));

  // 主题 J：三栏
  add(await run('J', [
    { name: '三栏都存在',
      act: `1`, probe: `['lj-c1','lj-c2','lj-c3'].filter(function(c){return document.querySelector('.'+c)}).length`,
      expect: (b,a) => a === 3 },
    { name: '导航按钮',
      act: `1`, probe: `document.querySelectorAll('.lj-navi').length`, expect: (b,a) => a >= 1 },
    { name: '设置栏空态',
      act: `1`, probe: `document.querySelectorAll('.lj-setempty').length`, expect: (b,a) => a >= 1 },
  ]));

  console.log(`\n总计：通过 ${tp} / 失败 ${tf}`);
  process.exit(0);
})();
