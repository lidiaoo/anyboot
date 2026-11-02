#!/usr/bin/env python3
"""
对比服务器：同时提供「旧版」和「新版」前端，并注入 mock __TAURI__。

  http://127.0.0.1:8770/old/hub.html     旧版 hub
  http://127.0.0.1:8770/new/hub.html     新版 hub
  http://127.0.0.1:8770/old/index.html   旧版主窗
  http://127.0.0.1:8770/new/index.html   新版主窗

注入方式：所有 .html 响应前插一段 <script>，内容 = mockdata.json + mock-tauri.js。
"""
import http.server, socketserver, json, pathlib, urllib.parse

ROOT = pathlib.Path("/tmp/cmp")
OLD = ROOT / "old"
NEW = pathlib.Path("/Users/jianghh/IdeaProjects/anyboot/ui-vue/dist")
MOCK_JS = (ROOT / "mock-tauri.js").read_text(encoding="utf-8")
MOCK_DATA = json.loads((ROOT / "mockdata.json").read_text(encoding="utf-8"))

INJECT = """<script>
window.__MOCK_SNAPSHOT__ = %s;
window.__MOCK_REMOTES__ = %s;
window.__MOCK_SERVERS__ = %s;
</script>
<script>%s</script>
"""

def data_script():
    return INJECT % (
        json.dumps(MOCK_DATA["snapshot"], ensure_ascii=False),
        json.dumps(MOCK_DATA["remotes"], ensure_ascii=False),
        json.dumps(MOCK_DATA["servers"], ensure_ascii=False),
        MOCK_JS,
    )

class Handler(http.server.SimpleHTTPRequestHandler):
    def translate_path(self, path):
        path = urllib.parse.unquote(path.split("?")[0].split("#")[0])
        if path.startswith("/old/"):
            base, rel = OLD, path[5:]
        elif path.startswith("/new/"):
            base, rel = NEW, path[5:]
        else:
            return str(ROOT / "__none__")
        target = (base / rel.lstrip("/")).resolve()
        if not str(target).startswith(str(base.resolve())):
            return str(ROOT / "__none__")
        if target.is_dir():
            target = target / "index.html"
        return str(target)

    def end_headers(self):
        self.send_header("Cache-Control", "no-store")
        super().end_headers()

    def do_GET(self):
        p = self.translate_path(self.path)
        f = pathlib.Path(p)
        if not f.is_file():
            self.send_error(404, "not found")
            return
        body = f.read_bytes()
        if f.suffix == ".html":
            text = body.decode("utf-8", "replace")
            inject = data_script()
            # 注入到 <head> 之后（越早越好，旧版脚本一进来就读 __TAURI__）
            if "<head>" in text:
                text = text.replace("<head>", "<head>\n" + inject, 1)
            else:
                text = inject + text
            body = text.encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
        else:
            ctype = {
                ".js": "text/javascript; charset=utf-8",
                ".css": "text/css; charset=utf-8",
                ".svg": "image/svg+xml",
                ".png": "image/png",
                ".woff2": "font/woff2",
            }.get(f.suffix, "application/octet-stream")
            self.send_response(200)
            self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass

if __name__ == "__main__":
    socketserver.TCPServer.allow_reuse_address = True
    with socketserver.TCPServer(("127.0.0.1", 8770), Handler) as httpd:
        print("对比服务: http://127.0.0.1:8770/old/hub.html  |  /new/hub.html")
        httpd.serve_forever()
