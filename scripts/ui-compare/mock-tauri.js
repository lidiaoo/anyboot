/*
 * 给旧版前端注入一个假 __TAURI__，让它能在无头浏览器里完整渲染。
 *
 * 目的：**只用于视觉对比**。数据用真实的 /api/ui-state 快照，
 * 所有 invoke 都返回合理的空值或快照里的对应段。
 */
(function () {
  var SNAP = window.__MOCK_SNAPSHOT__ || {};
  var listeners = {};

  function ok(value) { return Promise.resolve(value); }
  function empty() { return Promise.resolve(null); }

  // 按命令名分派：能从快照里取的就取，取不到的给保守空值
  var HANDLERS = {
    get_ui_state: function () { return SNAP; },
    remote_list: function () { return { servers: window.__MOCK_REMOTES__ || [], connect_all: false }; },
    hub_list_servers: function () { return window.__MOCK_SERVERS__ || []; },
    hub_keys_list: function () { return window.__MOCK_KEYS__ || []; },
    local_http_endpoint: function () {
      return { port: 12300, token: 'mock', enabled: true, listening: true, addr: '0.0.0.0:12300', error: '' };
    },
    remote_system_info: function () { return { os: 'linux', osName: 'Ubuntu', osVersion: '24.04 LTS', kernel: '6.8.0-45-generic', arch: 'x86_64', hostname: 'pc-ubuntu', cpuModel: 'AMD Ryzen 9', cpuCores: 16, memTotalMb: 32768, memFreeMb: 16384, uptimeSecs: 123456, lanIps: ['192.168.0.10'], httpPort: 12300, appVersion: '0.2.5' }; },
    gateway_pair_issue: function (args) { return 'anyboot-pair://tunnel?u=https://xxx.trycloudflare.com&t=tok123&n=' + ((args && args.name) || ''); },
    gateway_tunnel_logs: function () { return ['2026-11-03 10:00:01 [info] tunnel started', '2026-11-03 10:00:05 [info] connected']; },
    gateway_config: function () {
      window.__MOCK_GW__ = window.__MOCK_GW__ || { mode: 'quick', namedUrl: '', namedTokenSet: false, autostart: false, binary: { installed: false, version: '', path: '', source: '' }, tunnel: { running: false, mode: 'quick', url: '', state: '', error: '' } };
      return window.__MOCK_GW__;
    },
    gateway_save_config: function (args) {
      window.__MOCK_GW__ = window.__MOCK_GW__ || {};
      if (args && args.mode) window.__MOCK_GW__.mode = args.mode;
      if (args && typeof args.named_url === 'string') window.__MOCK_GW__.namedUrl = args.named_url;
      return null;
    },
    gateway_pair_list: function () { return []; },
    backup_sync_get_state: function () {
      // 与真实后端同构；enabled 用模块级变量模拟「切换后持久」
      window.__MOCK_SYNC__ = window.__MOCK_SYNC__ || { enabled: false };
      return {
        enabled: window.__MOCK_SYNC__.enabled,
        url: '', username: '', hasPassword: false, remoteDir: '/', lastSyncAt: '', lastError: '',
      };
    },
    backup_sync_set_enabled: function (args) {
      window.__MOCK_SYNC__ = window.__MOCK_SYNC__ || { enabled: false };
      window.__MOCK_SYNC__.enabled = Boolean(args && args.enabled);
      return null;
    },
    hub_sftp_transfer_list: function () {
      window.__MOCK_XFER__ = window.__MOCK_XFER__ !== undefined ? window.__MOCK_XFER__ : [
        { key: 'tx1', dir: 'down', name: 'backup-2026.tar.gz', total: 10485760, bytes: 4194304, state: 'running' }
      ];
      return window.__MOCK_XFER__;
    },
    hub_term_sessions: function () { return []; },
    update_state: function () { return { phase: 'idle', currentVersion: '0.2.5', latestVersion: '', notes: '', publishedAt: '', assetName: '', assetSize: 0, downloaded: 0, total: 0, error: '', checkedAt: '', source: '' }; },
    remote_boot_info: function (args) {
      window.__RBI_ARGS__ = JSON.stringify(args);
      var rid = (args && (args.serverId || args.server_id)) || 'local';
      var m = window.__MOCK_BOOTINFO__ || {};
      return m[rid] || m.local || null;
    }
  };

  function invoke(cmd, args) {
    var h = HANDLERS[cmd];
    if (h) { try { return ok(h(args)); } catch (e) { return ok(null); } }
    // 未覆盖的命令：给空值，避免 Promise 挂起导致页面卡住
    return ok(null);
  }

  window.__TAURI__ = {
    core: { invoke: invoke },
    invoke: invoke,
    event: {
      listen: function (name, cb) { (listeners[name] = listeners[name] || []).push(cb); return Promise.resolve(function () {}); },
      emit: function () { return Promise.resolve(); },
      once: function (name, cb) { return window.__TAURI__.event.listen(name, cb); }
    },
    app: {
      getVersion: function () { return Promise.resolve('0.2.5'); }
    },
    window: {
      getCurrentWindow: function () { return { label: 'mock', setTitle: empty, show: empty, hide: empty, close: empty }; }
    },
    webviewWindow: {
      getCurrentWebviewWindow: function () { return { label: 'mock' }; }
    },
    tauri: { invoke: invoke }
  };
  // 旧版 app.js 用 `typeof window.__TAURI__ !== 'undefined'` 判断
  // @tauri-apps/api 的 event 模块要用这两个内部函数
  var cbId = 1;
  var callbacks = {};
  window.__TAURI_INTERNALS__ = {
    transformCallback: function (cb) {
      var id = cbId++;
      callbacks[id] = cb;
      return id;
    },
    invoke: invoke,
    unregisterListener: function () { return Promise.resolve(); },
    metadata: { currentWindow: { label: 'mock' }, currentWebview: { windowLabel: 'mock', label: 'mock' } },
    convertFileSrc: function (p) { return p; },
    unregisterCallback: function (id) { delete callbacks[id]; },
  };
})();

/* Tauri v2 的事件插件用的是这个独立命名空间 */
window.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
  unregisterListener: function () { return Promise.resolve(); },
};

// ===== 严格参数校验（第 44 轮）：核对前端 invoke 传的参数是否满足 Rust 必填 =====
(function () {
  var REQ = window.__MOCK_CMD_REQUIRED__ || {};
  window.__STRICT_VIOLATIONS__ = [];
  if (!window.__TAURI__ || !window.__TAURI__.core) return;
  var orig = window.__TAURI__.core.invoke;
  window.__TAURI__.core.invoke = function (cmd, args) {
    var req = REQ[cmd];
    if (req && req.length) {
      var a = args || {};
      var missing = [];
      for (var i = 0; i < req.length; i++) {
        var k = req[i];
        // 兼容 snake/camel：两种都没传才算缺
        var camel = k.replace(/_([a-z])/g, function (_, c) { return c.toUpperCase(); });
        if (a[k] === undefined && a[camel] === undefined) missing.push(k);
      }
      if (missing.length) {
        window.__STRICT_VIOLATIONS__.push(cmd + " 缺参数: " + missing.join(","));
      }
    }
    return orig.apply(this, arguments);
  };
})();
