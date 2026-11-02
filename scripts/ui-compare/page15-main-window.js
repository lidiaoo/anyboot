const { spawn } = require('child_process'); const http = require('http');
const WebSocket = require('/tmp/ab-measure/node_modules/ws');
const wait = (ms) => new Promise(r => setTimeout(r, ms));
const getJson = (u) => new Promise((res, rej) => http.get(u, r => { let d=''; r.on('data',c=>d+=c); r.on('end',()=>{try{res(JSON.parse(d))}catch(e){rej(e)}}); }).on('error', rej));
(async () => {
  const chrome = spawn('/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', ['--headless=new','--disable-gpu','--no-sandbox','--remote-debugging-port=9629','--user-data-dir=/tmp/pm-'+Date.now(),'about:blank'], { stdio: 'ignore' });
  let t=null;
  for(let i=0;i<60;i++){ await wait(300); try{const j=await getJson('http://127.0.0.1:9629/json'); t=j.find(x=>x.type==='page'); if(t)break; }catch(e){} }
  const ws=new WebSocket(t.webSocketDebuggerUrl); let id=0; const pend=new Map(); const errs=[];
  ws.on('message',d=>{const m=JSON.parse(d);
    if(m.method==='Runtime.exceptionThrown'){const e=m.params.exceptionDetails; errs.push(String((e.exception&&e.exception.description)||e.text).slice(0,120));}
    if(m.id&&pend.has(m.id)){pend.get(m.id)(m);pend.delete(m.id);}});
  const send=(me,pa={})=>new Promise(r=>{const i=++id;pend.set(i,r);ws.send(JSON.stringify({id:i,method:me,params:pa}));});
  await new Promise(r=>ws.on('open',r));
  await send('Runtime.enable'); await send('Page.enable'); await send('Network.enable'); await send('Network.setCacheDisabled',{cacheDisabled:true});
  await send('Emulation.setDeviceMetricsOverride',{width:1000,height:760,deviceScaleFactor:1,mobile:false});
  await send('Page.navigate',{url:'http://127.0.0.1:8770/new/index.html?t='+Date.now()});
  await wait(5000);
  const ev=async(e)=>{const r=await send('Runtime.evaluate',{expression:e,returnByValue:true,awaitPromise:true}); return r.result&&r.result.result?r.result.result.value:null;};
  await ev(`window.alert=function(){};window.confirm=function(){return false};window.prompt=function(){return null};`);
  // 主窗代表性 id
  const ids = ['tabs','tab-boot','tab-adv-boot','tab-settings','tab-sol','grub','grub-list','grub-install-actions','grub-install','grub-uninstall','grub-log-box','grub-script','grub-script-save','grub-script-reset','grub-advice','grub-error','privCard','privBadge','privState','privPaths','privInstall','privRemove','privPwBox','privPw','privPwGo','privPwCancel','privNote','next-boot-name','next-boot-last','next-boot-rows','boot-adv-mount','boot-channel-collapsed-names','sol-progress','sol-progress-fill','sol-progress-text','sol-status-privileged','status-bar','theme','opacity'];
  let f=0, miss=[];
  for (const i of ids) { if (await ev(`!!document.getElementById('${i}')`)) f++; else miss.push(i); }
  console.log(`主窗代表 id: ${f}/${ids.length}`, miss.length?('缺 '+miss.join(' ')):'✅');
  console.log('  4 个页签:', await ev(`document.querySelectorAll('.tabs .tab').length`));
  // 切到基本设置检查 login-items / hotzone
  await ev(`(function(){var b=[].slice.call(document.querySelectorAll('.tabs .tab')).find(function(x){return /基本设置/.test(x.textContent)}); if(b)b.click(); return 1})()`);
  await wait(700);
  console.log('  基本设置 tab active:', await ev(`document.getElementById('tab-settings')?.classList.contains('active')`));
  console.log('  main-hotzone-border-color:', await ev(`!!document.getElementById('main-hotzone-border-color')`));
  console.log('  pending-card 容器逻辑:', await ev(`!!document.querySelector('.pending, .settings')`));
  console.log('异常:', errs.slice(0,3).join(' | ') || '(无)');
  ws.close(); chrome.kill(); process.exit(0);
})();
