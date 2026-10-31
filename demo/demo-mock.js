// AnyBoot 在线演示 mock 层 —— 在 app.js 之前加载。
// 拦截 fetch / WebSocket，用真机抓取的快照与命令响应做回放；
// 写操作只改演示快照，绝不触达任何真机。
(function () {
  "use strict";
  var DEMO = window.ANYBOOT_DEMO;
  if (!DEMO) { console.error("demo-data.js 未加载"); return; }

  // 预先种一个假 token，应用启动读到后就不会弹「输入访问令牌」的登录框
  try {
    document.cookie = "anyboot_token=demo-session; path=/; max-age=31536000; SameSite=Strict";
  } catch (e) { /* ignore */ }

  function clone(o) { return JSON.parse(JSON.stringify(o)); }
  function snapshot() { return clone(DEMO.uiState.result); }

  // 可变演示状态：下次启动的系统 / rEFInd 选中项 / GRUB 选中项
  var state = {
    system: null,          // "linux" | "windows" | "macos"
    refindTitle: null,     // refind_sim_switch 写入的标题
    grubSelector: null,    // linux_grub_switch 写入的 selector
  };

  var SYSTEM_LABEL = { linux: "Linux", windows: "Windows", macos: "macOS" };

  function respond(obj, status) {
    return new Response(JSON.stringify(obj), {
      status: status || 200,
      headers: { "Content-Type": "application/json" },
    });
  }

  // ---- 快照变更：让「切换」在界面上有真实回显 ----

  function applySystemSwitch(sys) {
    state.system = sys;
    var snap = DEMO.uiState.result;
    if (snap.status) {
      snap.status.message = "下次启动：" + (SYSTEM_LABEL[sys] || sys) + "（演示）";
      if (Array.isArray(snap.status.variables)) {
        snap.status.variables.forEach(function (v) {
          v.active = v.system === sys;
        });
      }
    }
  }

  function simView() {
    var sim = clone(DEMO.refind_sim_scan.result || DEMO.refind_sim_scan);
    if (state.refindTitle) {
      sim.previousBoot = state.refindTitle;
      (sim.entries || []).forEach(function (e) {
        e.lastBoot = (e.title || "").trim() === state.refindTitle;
        (e.children || []).forEach(function (c) {
          c.lastBoot = (c.title || "").trim() === state.refindTitle;
        });
      });
    }
    return sim;
  }

  function grubView() {
    var view = clone(DEMO.linux_grub_scan.result || DEMO.linux_grub_scan);
    if (state.grubSelector) {
      view.savedEntry = state.grubSelector;
      view.nextEntry = state.grubSelector;
    }
    return view;
  }

  function refindSwitchResult(title) {
    state.refindTitle = (title || "").trim();
    // 同步快照里的 refindSim（轮询重画时用）
    var snap = DEMO.uiState.result;
    if (snap.refindSim) {
      snap.refindSim.previousBoot = state.refindTitle;
      (snap.refindSim.entries || []).forEach(function (e) {
        e.lastBoot = (e.title || "").trim() === state.refindTitle;
        (e.children || []).forEach(function (c) {
          c.lastBoot = (c.title || "").trim() === state.refindTitle;
        });
      });
    }
    if (snap.status) snap.status.message = "下次启动：" + state.refindTitle + "（演示）";
    return { message: "已写入 PreviousBoot：" + state.refindTitle + "（演示模式，不作用于真机）" };
  }

  function grubSwitchResult(selector) {
    state.grubSelector = selector;
    var snap = DEMO.uiState.result;
    if (snap.linuxGrub) {
      snap.linuxGrub.savedEntry = selector;
      snap.linuxGrub.nextEntry = selector;
    }
    if (snap.status) snap.status.message = "下次启动：" + selector + "（演示）";
    var view = grubView();
    view.via = "demo";
    return view;
  }

  // ---- fetch 拦截 ----

  var realFetch = window.fetch ? window.fetch.bind(window) : null;
  window.fetch = function (input, init) {
    var url = typeof input === "string" ? input : (input && input.url) || "";
    var path = url.split("?")[0];
    var method = ((init && init.method) || "GET").toUpperCase();

    if (path.indexOf("/health", path.length - 7) !== -1) {
      return Promise.resolve(respond({
        ok: true, service: "anyboot",
        machineId: "demo0000demo0000demo0000demo0000",
        machineName: "演示机（模拟数据）",
      }));
    }
    if (path.indexOf("/api/ui-state", path.length - 14) !== -1) {
      return Promise.resolve(respond(snapshot()));
    }
    if (path.indexOf("/api/ui-command", path.length - 15) !== -1 && method === "POST") {
      var cmd = "", args = {};
      try {
        var body = JSON.parse((init && init.body) || "{}");
        cmd = body.cmd || ""; args = body.args || {};
      } catch (e) { /* ignore */ }

      switch (cmd) {
        case "get_ui_state":
          return Promise.resolve(respond({ cmd: cmd, ok: true, result: snapshot() }));
        case "refresh":
          return Promise.resolve(respond(DEMO.refresh));
        case "http_access_hint":
          return Promise.resolve(respond(DEMO.http_access_hint));
        case "refind_sim_scan":
          return Promise.resolve(respond({ cmd: cmd, ok: true, result: simView() }));
        case "linux_grub_scan":
          return Promise.resolve(respond({ cmd: cmd, ok: true, result: grubView() }));
        case "switch_system": {
          var sys = args.system || "";
          applySystemSwitch(sys);
          return Promise.resolve(respond({
            cmd: cmd, ok: true,
            result: { pending: false, message: "下次启动：" + (SYSTEM_LABEL[sys] || sys) + "（演示）" },
          }));
        }
        case "refind_sim_switch":
          return Promise.resolve(respond({ cmd: cmd, ok: true, result: refindSwitchResult(args.title) }));
        case "linux_grub_switch":
          return Promise.resolve(respond({ cmd: cmd, ok: true, result: grubSwitchResult(args.selector) }));
        case "linux_grub_pick":
          return Promise.resolve(respond({ cmd: cmd, ok: true, result: grubView() }));
        case "power_action":
          var act = { reboot: "重启", shutdown: "关机", sleep: "睡眠" }[args.action] || args.action;
          return Promise.resolve(respond({
            cmd: cmd, ok: true,
            result: { pending: false, message: "演示模式：「" + act + "」已模拟，不会作用于任何真机" },
          }));
        default:
          // set_* / sol_* / 其余写命令：一律「已模拟」，成功但不落盘
          return Promise.resolve(respond({
            cmd: cmd, ok: true, result: null,
            message: "演示模式：「" + cmd + "」已模拟（设置不会真实保存）",
          }));
      }
    }
    if (realFetch) return realFetch(input, init);
    return Promise.reject(new Error("demo: unhandled fetch " + url));
  };

  // ---- WebSocket 拦截：连上即推一帧快照，之后每 8 秒推一帧（模拟推送） ----
  // 应用用 addEventListener("open"/"message") 监听，必须完整支持事件监听 API。

  function MockWebSocket(url) {
    this.url = url;
    this.readyState = 0;
    this._listeners = {};
    var self = this;
    setTimeout(function () {
      if (self._closed) return;
      self.readyState = 1;
      self._fire("open", { type: "open" });
      self._fire("message", { data: JSON.stringify(snapshot()) });
      self._timer = setInterval(function () {
        if (self._closed || document.hidden) return;
        self._fire("message", { data: JSON.stringify(snapshot()) });
      }, 8000);
    }, 120);
  }
  MockWebSocket.prototype._fire = function (type, ev) {
    (this._listeners[type] || []).slice().forEach(function (fn) {
      try { fn(ev); } catch (e) { console.error(e); }
    });
    var handler = this["on" + type];
    if (handler) handler(ev);
  };
  MockWebSocket.prototype.addEventListener = function (type, fn) {
    (this._listeners[type] = this._listeners[type] || []).push(fn);
  };
  MockWebSocket.prototype.removeEventListener = function (type, fn) {
    var list = this._listeners[type] || [];
    var i = list.indexOf(fn);
    if (i >= 0) list.splice(i, 1);
  };
  MockWebSocket.prototype.send = function () {};
  MockWebSocket.prototype.close = function () {
    this._closed = true;
    if (this._timer) clearInterval(this._timer);
    this.readyState = 3;
    var self = this;
    setTimeout(function () { self._fire("close", { type: "close" }); }, 0);
  };
  window.WebSocket = MockWebSocket;

  // ---- 演示横幅 ----

  function addBanner() {
    var bar = document.createElement("div");
    bar.style.cssText =
      "position:fixed;left:0;right:0;bottom:0;z-index:99999;" +
      "background:linear-gradient(90deg,#1f6feb,#8957e5);color:#fff;" +
      "font:12px/1.4 -apple-system,'PingFang SC','Microsoft YaHei',sans-serif;" +
      "padding:7px 14px;text-align:center;box-shadow:0 -2px 10px rgba(0,0,0,.35);";
    bar.innerHTML =
      "🎭 在线演示 —— 界面与真实应用一致，数据为真实快照回放；所有操作只在演示里生效，不会触达任何机器" +
      ' · <a href="https://github.com/lidiaoo/anyboot" target="_blank" style="color:#fff;text-decoration:underline">GitHub</a>';
    (document.body || document.documentElement).appendChild(bar);
  }
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", addBanner);
  } else {
    addBanner();
  }
})();
