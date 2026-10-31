// AnyBoot 远程管理（hub）演示 shim —— 必须在 hub 页面自身脚本之前加载。
// 伪造 window.__TAURI__（invoke / event / app），让 hub*.html 在浏览器里完整运行。
// 数据全部来自 hub-demo-data.js 的演示数据，写操作只改演示内存，不触达任何机器。
(function () {
  "use strict";
  var DEMO = window.ANYBOOT_HUB_DEMO;
  if (!DEMO) { console.error("hub-demo-data.js 未加载"); return; }

  function clone(o) { return JSON.parse(JSON.stringify(o)); }
  function now() { return Date.now(); }

  // ---- 可变演示状态 ----
  var servers = [clone(DEMO.server)];
  var keys = clone(DEMO.keys);
  var tunnels = [];        // {id, serverId, kind, spec, running}
  var termSessions = {};   // id -> {meta, buf}
  var sessionsLog = [];    // 会话页记录（终端 + 隧道）
  var transfers = [];      // 传输页记录
  var pairs = [];          // 远程接入配对（演示空）

  // ---- 事件系统（window.__TAURI__.event）----
  var listeners = {};
  function emit(name, payload) {
    (listeners[name] || []).slice().forEach(function (fn) {
      try { fn({ event: name, id: -1, payload: payload }); } catch (e) { console.error(e); }
    });
  }

  // ---- 模拟终端 ----
  function b64decode(s) {
    try { return decodeURIComponent(escape(atob(s))); } catch (e) { return ""; }
  }
  function b64encode(s) {
    try { return btoa(unescape(encodeURIComponent(s))); } catch (e) { return ""; }
  }

  var FS = {
    "~": ["anyboot", "documents", "logs", "readme.md"],
    "~/documents": ["notes.txt"],
    "~/logs": ["anyboot.log", "sol.log"],
  };

  function fakeShell(session, input) {
    var out = "";
    session.line = (session.line || "") + input;
    while (session.line.indexOf("\r") !== -1 || session.line.indexOf("\n") !== -1) {
      var idx = session.line.indexOf("\r");
      if (idx === -1) idx = session.line.indexOf("\n");
      var raw = session.line.slice(0, idx);
      session.line = session.line.slice(idx + 1);
      var cmdline = raw.replace(/.*\$\s*/, "").trim(); // 去掉粘贴进来的提示符
      out += "\r\n";
      if (cmdline) {
        var parts = cmdline.split(/\s+/);
        var cmd = parts[0];
        if (cmd === "ls") {
          var dir = session.cwd === "~" ? "~" : session.cwd;
          var names = FS[dir] || ["bin", "boot", "etc", "home", "usr", "var"];
          out += names.join("  ") + "\r\n";
        } else if (cmd === "pwd") {
          out += (session.cwd === "~" ? "/home/demo" : "/home/demo/" + session.cwd.slice(2)) + "\r\n";
        } else if (cmd === "whoami") {
          out += "demo\r\n";
        } else if (cmd === "uname") {
          out += "Linux anyboot-demo 5.15.0-140-generic #156-Ubuntu x86_64 GNU/Linux\r\n";
        } else if (cmd === "cat") {
          out += (parts[1] === "readme.md"
            ? "# 演示服务器\r\n这是 AnyBoot 在线演示的模拟终端。\r\n"
            : "cat: " + (parts[1] || "") + ": 没有那个文件（演示终端只内置了少量命令）\r\n");
        } else if (cmd === "cd") {
          var target = parts[1] || "~";
          session.cwd = target === ".." ? "~" : (target === "/" ? "/" : (target.indexOf("~/") === 0 ? target : (FS[target] ? target : session.cwd)));
          if (target !== "~" && !FS[target] && target !== "/" && target.indexOf("~/") !== 0) {
            out += "cd: " + target + ": 没有那个文件或目录\r\n";
          }
        } else if (cmd === "clear") {
          out = "\x1b[2J\x1b[H";
        } else if (cmd === "echo") {
          out += parts.slice(1).join(" ") + "\r\n";
        } else if (cmd === "uptime") {
          out += " 07:02:11 up 3:50,  1 user,  load average: 0.08, 0.12, 0.09\r\n";
        } else if (cmd === "df") {
          out += "Filesystem      Size  Used Avail Use% Mounted on\r\n/dev/sda1        40G   12G   26G  32% /\r\n";
        } else if (cmd === "help") {
          out += "演示终端支持的命令：ls cd pwd cat echo whoami uname uptime df clear help exit\r\n";
        } else if (cmd === "exit") {
          out += "logout\r\n";
          emit("hub-term-closed", { id: session.meta.id, code: 0 });
          return out;
        } else {
          out += "bash: " + cmd + ": command not found（演示终端，试试 help）\r\n";
        }
      }
      out += DEMO.term.prompt;
    }
    return out;
  }

  // ---- invoke 路由 ----
  function invoke(cmd, args) {
    args = args || {};
    switch (cmd) {
      // ---- 本机电脑页 ----
      case "get_ui_state":
        return Promise.resolve(clone(window.ANYBOOT_DEMO.uiState.result));
      case "get_settings":
        return Promise.resolve(clone((window.ANYBOOT_DEMO.uiState.result || {}).settings || {}));
      case "local_http_endpoint":
        return Promise.resolve({ enabled: true, listening: true, port: 12300, addr: "0.0.0.0:12300", tokenSet: true, tokenHint: "de…mo!" });
      case "local_service_health":
        return Promise.resolve({ ok: true, service: "anyboot", listening: true, port: 12300 });
      case "power_action":
        return Promise.resolve(null);
      case "set_theme":
      case "set_grub_list_format":
        try { localStorage.setItem("anyboot.theme-resolved", args.theme || ""); } catch (e) {}
        emit("theme-changed", { theme: args.theme });
        return Promise.resolve(null);
      case "hub_page_ping":
        return Promise.resolve({ page: location.pathname, ok: true });
      case "hub_open_window":
        return Promise.resolve(null);
      case "hub_focus_window":
        return Promise.resolve(null);
      case "hub_about_open":
        return Promise.resolve(null);

      // ---- 远程电脑（手机/网页客户端连的那台）----
      case "remote_list":
        return Promise.resolve(clone(DEMO.remotes));
      case "remote_save": {
        // 与真实端同语义的内存持久化：改到哪台就更新哪台（含 show_probe /
        // route_probe_hidden 这些新字段，原样存原样取），否则演示里开关不生效
        var srv = clone(args.server || {});
        if (!srv.id) srv.id = "srv-" + now();
        var idx = -1;
        for (var i = 0; i < DEMO.remotes.servers.length; i++) {
          if (DEMO.remotes.servers[i].id === srv.id) { idx = i; break; }
        }
        if (idx >= 0) DEMO.remotes.servers[idx] = srv; else DEMO.remotes.servers.push(srv);
        return Promise.resolve(clone(srv));
      }
      case "remote_set_connect_all":
        return Promise.resolve(null);
      case "remote_delete":
        return Promise.resolve(null);
      case "remote_test":
        return Promise.resolve("延迟 12 ms（演示）");
      case "remote_test_via":
        return Promise.resolve("延迟 12 ms（演示）");
      case "remote_endpoint":
        return Promise.resolve({ enabled: true, listening: true, port: 12300 });
      case "remote_system_info":
        return Promise.resolve(clone(window.ANYBOOT_DEMO.apiSystem));
      case "remote_note_machine":
        return Promise.resolve(null);
      case "remote_promote_address":
        return Promise.resolve(null);
      case "remote_boot_info":
        // 电脑列表的引导快照：直接给本机快照的精简版
        return Promise.resolve({
          ok: true,
          nextBoot: "rEFInd（演示）",
          refindEntries: 22,
          grubEntries: 68,
          channels: ["rEFInd", "GRUB", "OpenCore"],
        });
      case "remote_boot_switch":
        return Promise.resolve({ ok: true, message: "演示模式：已模拟切换" });

      // ---- SSH 服务器档案 ----
      case "hub_list_servers":
        return Promise.resolve(clone(servers));
      case "hub_save_server": {
        var saved = clone(args.server || {});
        if (!saved.id) saved.id = "srv-" + now();
        var found = false;
        for (var i = 0; i < servers.length; i++) {
          if (servers[i].id === saved.id) { servers[i] = saved; found = true; break; }
        }
        if (!found) servers.push(saved);
        return Promise.resolve(clone(saved));
      }
      case "hub_delete_server":
        servers = servers.filter(function (s) { return s.id !== args.serverId; });
        return Promise.resolve(null);
      case "hub_test_server":
        return Promise.resolve("可连接 · 延迟 14 ms（演示）");
      case "hub_pick_directory":
        return Promise.resolve("/home/demo");
      case "hub_report_error":
        console.warn("[demo] hub_report_error", args);
        return Promise.resolve(null);

      // ---- 打开子窗：演示里改为页内跳转 ----
      case "hub_open_term_window":
        location.href = "hub-term-demo.html";
        return Promise.resolve(null);
      case "hub_open_files_window":
        location.href = "hub-files-demo.html";
        return Promise.resolve(null);
      case "hub_open_keys_window":
        location.href = "hub-keys-demo.html";
        return Promise.resolve(null);

      // ---- 终端 ----
      case "hub_term_sessions":
        return Promise.resolve(Object.keys(termSessions).map(function (id) { return clone(termSessions[id].meta); }));
      case "hub_term_take_pending":
        return Promise.resolve(clone(DEMO.server));
      case "hub_term_open": {
        var id = "term-" + now();
        var meta = {
          id: id,
          server_name: (args.server && args.server.name) || DEMO.server.name,
          host_label: ((args.server && args.server.username) || "demo") + "@" + ((args.server && args.server.host) || "192.168.1.50") + ":" + ((args.server && args.server.port) || 22),
          protocol: "ssh",
        };
        termSessions[id] = { meta: meta, line: "", cwd: "~" };
        sessionsLog.push({ kind: "term", serverId: args.server && args.server.id, name: meta.server_name, at: now() });
        setTimeout(function () {
          emit("hub-term-data", { id: id, data: b64encode(DEMO.term.banner + DEMO.term.prompt) });
        }, 350);
        return Promise.resolve(meta);
      }
      case "hub_term_write": {
        var s = termSessions[args.sessionId];
        if (s) {
          var text = b64decode(args.data || "");
          var outp = fakeShell(s, text);
          if (outp) {
            setTimeout(function () {
              emit("hub-term-data", { id: args.sessionId, data: b64encode(outp) });
            }, 60);
          }
        }
        return Promise.resolve(null);
      }
      case "hub_term_resize":
        return Promise.resolve(null);
      case "hub_term_close": {
        var closed = termSessions[args.sessionId];
        delete termSessions[args.sessionId];
        emit("hub-term-closed", { id: args.sessionId, code: 0 });
        return Promise.resolve(null);
      }

      // ---- SFTP 文件 ----
      case "hub_sftp_take_pending":
        return Promise.resolve(clone(DEMO.server));
      case "hub_sftp_home":
        return Promise.resolve("/home/demo");
      case "hub_sftp_list": {
        var path = (args.path || "/home/demo").replace(/\/+$/, "") || "/";
        var norm = path === "/home/demo" ? "/home/demo" : path;
        var entries = DEMO.sftp[norm];
        if (!entries) {
          // 未建模的目录：给一个空目录
          entries = [];
        }
        return Promise.resolve(clone(entries));
      }
      case "hub_sftp_mkdir":
      case "hub_sftp_rename":
      case "hub_sftp_copy":
      case "hub_sftp_remove":
        return Promise.reject("演示模式：文件修改操作未开放（只能浏览）");
      case "hub_sftp_readlink":
        return Promise.reject("演示模式：未模拟");
      case "hub_sftp_download":
        return Promise.reject("演示模式：下载未开放");
      case "hub_sftp_upload":
        return Promise.reject("演示模式：上传未开放");
      case "hub_sftp_transfer_list":
        return Promise.resolve(clone(transfers));
      case "hub_sftp_transfer_ctl":
        return Promise.resolve(null);
      case "hub_download_dir_default":
        return Promise.resolve("/Users/demo/Downloads");

      // ---- 密钥 ----
      case "hub_keys_list":
        return Promise.resolve(clone(keys));
      case "hub_key_public_key":
        return Promise.resolve("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIDemoDemoDemoDemoDemoDemoDemoDemoDemo demo@anyboot-demo");
      case "hub_key_generate": {
        var kid = "id_ed25519_" + now().toString(36);
        keys.push({ id: kid, kind: "Ed25519", fingerprint: "SHA256:Demo" + now().toString(16), has_passphrase: false, updated_at: now(), used_by: 0 });
        return Promise.resolve({ id: kid });
      }
      case "hub_key_import":
        return Promise.reject("演示模式：导入未开放");
      case "hub_key_delete":
        keys = keys.filter(function (k) { return k.id !== args.keyId; });
        return Promise.resolve(null);

      // ---- 隧道 ----
      case "hub_list_tunnels":
        return Promise.resolve(clone(tunnels));
      case "hub_save_tunnel":
        return Promise.resolve(null);
      case "hub_tunnel_start": {
        var t = { id: "tn-" + now(), serverId: args.serverId, kind: args.kind || "local", spec: args.spec || "", running: true, startedAt: now() };
        tunnels.push(t);
        sessionsLog.push({ kind: "tunnel", serverId: args.serverId, name: (args.kind || "local") + " 隧道", at: now() });
        return Promise.resolve(clone(t));
      }
      case "hub_tunnel_stop":
        tunnels = tunnels.filter(function (t) { return t.id !== args.tunnelId; });
        return Promise.resolve(null);
      case "hub_delete_tunnel":
        tunnels = tunnels.filter(function (t) { return t.id !== args.tunnelId; });
        return Promise.resolve(null);

      // ---- 网关 / Tailscale / 备份：未配置状态 ----
      case "gateway_config":
        return Promise.resolve(clone(DEMO.gatewayConfig));
      case "gateway_save_config":
        return Promise.resolve(null);
      case "gateway_set_autostart":
        return Promise.resolve(null);
      case "gateway_binary_status":
        return Promise.resolve({ installed: false, version: null, path: null, manual: false });
      case "gateway_tailscale_status":
        return Promise.resolve({ installed: false, running: false, version: null, ip: null, hostname: null });
      case "gateway_tailscale_service":
        return Promise.resolve({ installed: false, running: false });
      case "gateway_tailscale_funnel_status":
        return Promise.resolve({ enabled: false, running: false, url: null });
      case "gateway_pair_list":
        return Promise.resolve([]);
      case "gateway_tunnel_orphans":
        return Promise.resolve([]);
      case "backup_sync_get_state":
        return Promise.resolve(clone(DEMO.backupSyncState));
      case "backup_preview":
        return Promise.reject("演示模式：备份预览未提供真实配置数据");
      case "backup_export":
      case "backup_import":
      case "backup_pick_export_path":
      case "backup_pick_import_file":
      case "backup_webdav_test":
      case "backup_webdav_mkdir":
        return Promise.reject("演示模式：该操作未开放");

      // ---- tsnet / gateway 长尾：一律未启用 ----
      default:
        if (/^tsnet_/.test(cmd)) {
          if (cmd === "tsnet_status" || cmd === "tsnet_logs") return Promise.resolve(cmd === "tsnet_status" ? { installed: false, running: false } : []);
          if (cmd === "tsnet_logs_clear" || cmd === "tsnet_disable" || cmd === "tsnet_logout") return Promise.resolve(null);
          return Promise.reject("演示模式：tsnet 未启用");
        }
        if (/^gateway_/.test(cmd)) {
          if (/logs$/.test(cmd)) return Promise.resolve([]);
          if (/status$/.test(cmd)) return Promise.resolve({ installed: false, running: false });
          return Promise.resolve(null);
        }
        console.info("[demo] 未模拟的命令：", cmd, args);
        return Promise.reject("演示模式：「" + cmd + "」未模拟");
    }
  }

  // ---- window.__TAURI__ ----
  window.__TAURI__ = {
    core: { invoke: invoke },
    app: { getVersion: function () { return Promise.resolve("0.2.0（演示）"); } },
    event: {
      listen: function (name, handler) {
        (listeners[name] = listeners[name] || []).push(handler);
        return Promise.resolve(function () {
          var list = listeners[name] || [];
          var i = list.indexOf(handler);
          if (i >= 0) list.splice(i, 1);
        });
      },
      emit: function (name, payload) { emit(name, payload); return Promise.resolve(null); },
    },
  };
})();
