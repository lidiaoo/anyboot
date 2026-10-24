// anyboot-tsnet —— AnyBoot 内置 Tailscale 引擎（tsnet sidecar）。
//
// 设计约定（本项目自定义，clean-room 实现：只依据 tailscale.com/tsnet 公开 API
// 与其文档注释）：
//   - 单进程单职责：把本机 AnyBoot HTTP 服务经 tsnet 的 Funnel 发布成
//     https://<机器名>.<tailnet>.ts.net 公网地址，反向代理到本地 upstream。
//   - stdout：每行一个 JSON 事件（协议见 event 结构），Rust 父进程逐行解析。
//   - stdin：首行为可选配置 {"authkey":"..."}（走管道，不进进程命令行，
//     避免密钥在任务管理器/进程列表里可见）；之后每行一个命令，quit 收尾退出；
//     父进程死亡 → stdin EOF → 自动自杀（不留孤儿进程）。
//   - 状态目录由父进程给（%LOCALAPPDATA%\AnyBoot\tsnet\state），登录凭据
//     首次登录后持久化在那里，之后无需再登录。
//
// 登录链接的唯一权威来源是 Server.UserLogf（tsnet 文档第 6 条认证途径：
// "An interactive login URL printed to [Server.UserLogf]"）：未登录时
// printAuthURLLoop 每 5 秒把 "To start this tsnet server, ... go to: <URL>"
// 打进 UserLogf。Server.Up 此时不会失败，会一直阻塞到浏览器授权完成。
// 因此这里只做两件事：把 UserLogf 里的 URL 抠出来发 login 事件（一次/每换一次），
// 其余日志按需转发给父进程的日志框；**不**去轮询本地状态 API 猜链接。
package main

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/httputil"
	"net/url"
	"os"
	"regexp"
	"strings"
	"sync"
	"time"

	"tailscale.com/tsnet"
)

// protoVersion：stdout 事件协议版本。加字段向后兼容；改语义才升版本。
const protoVersion = 1

const sidecarID = "anyboot-tsnet/1.0.0"

// 链接长度上限：父进程会把链接直接交给系统浏览器打开，超长一律不认。
const maxLinkBytes = 2048

// certWaitSeconds：等 netmap 里出现 CertDomains 的宽限（有就是立刻有，没有就是
// tailnet 没开 HTTPS 证书，别让用户干等）。
const certWaitSeconds = 10

// 官方登录页 URL（UserLogf 里唯一要捞的东西；其余日志不参与链接提取）。
var loginURLPattern = regexp.MustCompile(`https://login\.tailscale\.com/[A-Za-z0-9?&=_./%-]+`)

// event：stdout 事件（一次一行 JSON）。
//
//	hello  ：进程起来就发（proto/sidecar 自报家门）
//	state  ：状态机流转（state = starting / login_needed / ready / error）
//	         error 可能带 code + setupUrl（funnel 权限/证书未开时给官方开通页）
//	login  ：拿到（或换到新的）Tailscale 登录链接，loginUrl 为官方页
//	log    ：运行日志（UserLogf 非链接行 + 内部日志去重后转发；UI 日志框消费）
//	bye    ：正常收尾（父进程看到即知是优雅退出）
type event struct {
	Event    string  `json:"event"`
	Proto    int     `json:"proto,omitempty"`
	Sidecar  string  `json:"sidecar,omitempty"`
	State    string  `json:"state,omitempty"`
	URL      string  `json:"url,omitempty"`
	LoginURL string  `json:"loginUrl,omitempty"`
	Code     string  `json:"code,omitempty"`
	SetupURL string  `json:"setupUrl,omitempty"`
	Detail   string  `json:"detail,omitempty"`
	T        float64 `json:"t,omitempty"` // 进程启动后的秒数（UI 日志前缀用）
}

// startAt：进程启动时刻，emit 统一打相对时间戳（避免父/子两端各搞一套时钟）。
var startAt = time.Now()

var outMu sync.Mutex

func emit(e event) {
	if e.Event != "hello" {
		e.T = time.Since(startAt).Seconds() // 必须在 Marshal 之前打（曾放后面，t 恒为 0）
	}
	b, err := json.Marshal(e)
	if err != nil {
		return
	}
	outMu.Lock()
	fmt.Fprintln(os.Stdout, string(b))
	outMu.Unlock()
}

func emitState(state, detail string) {
	emit(event{Event: "state", State: state, Detail: detail})
}

// emitError：终态错误。code 给父进程做分类，setupUrl 是官方开通/修复页（可为空）。
func emitError(code, setupURL, detail string) {
	emit(event{Event: "state", State: "error", Code: code, SetupURL: setupURL, Detail: detail})
}

func emitLog(detail string) {
	detail = strings.TrimSpace(detail)
	if detail == "" {
		return
	}
	if runes := []rune(detail); len(runes) > 400 {
		detail = string(runes[:400]) + "…"
	}
	emit(event{Event: "log", Detail: detail})
}

// ==================== 链接白名单 ====================

// officialLoginURL 校验并规范化登录链接。父进程拿到后直接丢给系统浏览器，
// 所以这里必须白名单：https + 主机名精确等于 login.tailscale.com，且不带
// 端口/用户信息/超长内容。任何一项不满足就返回空（当作没抓到）。
func officialLoginURL(raw string) string {
	raw = strings.TrimSpace(raw)
	if raw == "" || len(raw) > maxLinkBytes {
		return ""
	}
	parsed, err := url.Parse(raw)
	if err != nil || parsed.Scheme != "https" || parsed.Hostname() != "login.tailscale.com" ||
		parsed.Port() != "" || parsed.User != nil {
		return ""
	}
	return parsed.String()
}

// funnelSetupLinks：Funnel 开通相关的两个静态官方页（固定文案页，无参数）。
var funnelSetupLinks = map[string]bool{
	"https://tailscale.com/s/no-funnel": true,
	"https://tailscale.com/s/https":     true,
}

// officialSetupURL 校验 Funnel 开通链接：只允许上面两个静态页，或 login.tailscale.com
// 的官方交互式开通页（管理台给的带 token 链接）。
func officialSetupURL(raw string) string {
	raw = strings.TrimSpace(raw)
	if raw == "" || len(raw) > maxLinkBytes {
		return ""
	}
	parsed, err := url.Parse(raw)
	if err != nil {
		return ""
	}
	normalized := strings.TrimSuffix(parsed.String(), "/")
	if funnelSetupLinks[normalized] {
		return normalized
	}
	if parsed.Scheme != "https" || parsed.Hostname() != "login.tailscale.com" ||
		parsed.Port() != "" || parsed.User != nil {
		return ""
	}
	return parsed.String()
}

// ==================== 登录链接捕获 ====================

// loginTracker：从日志行里抠登录链接。同一链接只发一次 login 事件；链接变化
// （授权链接过期后 control 会下发新的）重新发——父进程据此更新 UI 上的按钮。
// 这是"每 5 秒重打一遍"的日志刷屏与"链接会轮换"之间的平衡点。
type loginTracker struct {
	mu   sync.Mutex
	last string
}

// observe 扫描一行日志。返回 true 表示这行是登录提示（调用方不必再进日志框，
// 否则 printAuthURLLoop 每 5 秒刷一条同样内容）。链接不合法时当普通日志处理。
func (t *loginTracker) observe(line string) bool {
	raw := loginURLPattern.FindString(line)
	if raw == "" {
		return false
	}
	link := officialLoginURL(raw)
	if link == "" {
		return false
	}
	t.mu.Lock()
	changed := link != t.last
	if changed {
		t.last = link
	}
	t.mu.Unlock()
	if changed {
		emit(event{Event: "login", LoginURL: link})
	}
	return true
}

// ==================== stdin 配置 ====================

// stdinConfig：首行可选配置（父进程 spawn 后立即写入）。
type stdinConfig struct {
	AuthKey string `json:"authkey"`
}

// readStdinConfig：等首行配置最多 5 秒（老父进程不写也不至于挂死）。
func readStdinConfig(reader *bufio.Reader) stdinConfig {
	type result struct {
		line string
		ok   bool
	}
	ch := make(chan result, 1)
	go func() {
		line, err := reader.ReadString('\n')
		ch <- result{line: strings.TrimSpace(line), ok: err == nil}
	}()
	select {
	case r := <-ch:
		if !r.ok || r.line == "" {
			return stdinConfig{}
		}
		var cfg stdinConfig
		_ = json.Unmarshal([]byte(r.line), &cfg)
		return cfg
	case <-time.After(5 * time.Second):
		return stdinConfig{}
	}
}

// ==================== Funnel 就绪 / 失败诊断 ====================

// publicOrigin 从 tailnet DNS 名校验出公网地址（必须落在 .ts.net 下）。
func publicOrigin(dnsName string) string {
	host := strings.ToLower(strings.TrimSuffix(strings.TrimSpace(dnsName), "."))
	host = strings.TrimPrefix(host, "https://")
	if !strings.HasSuffix(host, ".ts.net") || len(host) <= len(".ts.net") {
		return ""
	}
	if strings.ContainsAny(host, "/:@ ") {
		return ""
	}
	return "https://" + host
}

// classifyFunnelFailure：把 ListenFunnel 的报错翻成「错误码 + 官方开通页 + 中文提示」。
// Tailscale 的失败文案里会带官方文档短链，靠它分类比匹配自然语言稳。
func classifyFunnelFailure(err error) (code, setupURL, hint string) {
	message := err.Error()
	switch {
	case strings.Contains(message, "https://tailscale.com/s/no-funnel"):
		return "funnel_permission_required", "https://tailscale.com/s/no-funnel",
			"这个 tailnet 还没开启 Funnel：点「去开通 Funnel」按官方引导开启，或在管理台 Access controls 里放行 funnel 节点属性"
	case strings.Contains(message, "https://tailscale.com/s/https"):
		return "funnel_https_required", "https://tailscale.com/s/https",
			"tailnet 还没开启 HTTPS 证书（Funnel 依赖它）：点「去开通」在管理台 DNS 页面开启 HTTPS Certificates"
	default:
		return "funnel_start_failed", "", "开启 Funnel 失败"
	}
}

// queryFunnelSetupURL 向本地状态 API 问「这个 tailnet 怎么开通 Funnel」，
// 返回的通常是登录态交互式链接（比静态文档页更好用）。拿不到就算了。
func queryFunnelSetupURL(ctx context.Context, srv *tsnet.Server) string {
	client, err := srv.LocalClient()
	if err != nil {
		return ""
	}
	info, err := client.QueryFeature(ctx, "funnel")
	if err != nil || info == nil || info.Complete {
		return ""
	}
	return officialSetupURL(info.URL)
}

// ==================== 公网自检 ====================

// probeFunnel：ready 之后做一次公网自检（GET <origin>/health），结果只写日志框、
// 不改状态机。理由：国内网络对 Tailscale 入站不一定通，自检失败是很有价值的排障
// 线索（比让用户拿手机试更快），但不能反过来把「本机访问不了」当成服务不可用。
func probeFunnel(ctx context.Context, origin string, timeout time.Duration) {
	probeCtx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()
	// 刚发 ready，给 ingress / 证书一点时间生效
	select {
	case <-probeCtx.Done():
		return
	case <-time.After(3 * time.Second):
	}
	req, err := http.NewRequestWithContext(probeCtx, http.MethodGet, origin+"/health", nil)
	if err != nil {
		return
	}
	// Proxy 显式置空：自检必须走本机出口，不能被系统代理环境变量截走
	client := &http.Client{
		Timeout:   timeout,
		Transport: &http.Transport{Proxy: nil},
	}
	resp, err := client.Do(req)
	if err != nil {
		emitLog("公网自检失败：" + err.Error() +
			"（本机 DNS/网络拦了 .ts.net，或 Funnel 入站尚未生效 —— 其他设备可能同样打不开）")
		return
	}
	defer resp.Body.Close()
	_, _ = io.Copy(io.Discard, io.LimitReader(resp.Body, 4096))
	if resp.StatusCode >= 200 && resp.StatusCode < 400 {
		emitLog(fmt.Sprintf("公网自检通过：%s/health → %d", origin, resp.StatusCode))
		return
	}
	emitLog(fmt.Sprintf("公网自检异常：%s/health → %d（隧道通了，但反代目标没响应？）", origin, resp.StatusCode))
}

func main() {
	stateDir := flag.String("state", "", "tsnet 状态目录（登录凭据持久化在这里）")
	hostname := flag.String("hostname", "anyboot", "tailnet 里的节点名（DNS 名的前缀）")
	upstream := flag.String("upstream", "http://127.0.0.1:12300", "反代目标（本机 AnyBoot HTTP 服务）")
	readyTimeout := flag.Duration("ready-timeout", 90*time.Second, "上线后等 Funnel 域名就绪的最长时间")
	flag.Parse()

	emit(event{Event: "hello", Proto: protoVersion, Sidecar: sidecarID})
	emitState("starting", "")

	stdinBuf := bufio.NewReader(os.Stdin)

	login := &loginTracker{}

	// UserLogf：tsnet 认为「该给用户看」的日志——登录链接就在这条通道里，
	// 是链接的权威来源。非链接行原样转发（AuthLoop 之类，量很小）。
	userLogf := func(format string, args ...any) {
		msg := fmt.Sprintf(format, args...)
		if login.observe(msg) {
			return
		}
		emitLog(msg)
	}

	// Logf：内部调试日志（含重试/网络噪音）。这里也顺手扫一遍链接（双保险，
	// 万一版本改动让链接只走 Logf），其余做相邻重复抑制——同一内容连打只记
	// 首条，第 5/20/50… 次提示一句，避免 200 条环形缓冲被刷爆。
	var lastMu sync.Mutex
	lastMsg := ""
	lastDup := 0
	logf := func(format string, args ...any) {
		msg := fmt.Sprintf(format, args...)
		if login.observe(msg) {
			return
		}
		lastMu.Lock()
		if msg == lastMsg {
			lastDup++
			n := lastDup
			lastMu.Unlock()
			if n == 5 || n == 20 || (n > 20 && n%50 == 0) {
				emitLog(msg + fmt.Sprintf("（同内容已重复 %d 次，后续静默）", n))
			}
			return
		}
		lastMsg = msg
		lastDup = 0
		lastMu.Unlock()
		emitLog(msg)
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	// tsnet 引擎：状态目录 + 节点名 + 可选 AuthKey（来自 stdin 配置行）。
	cfg := readStdinConfig(stdinBuf)
	if cfg.AuthKey != "" {
		emitLog("收到配置行：AuthKey 已提供（内容不落日志）")
	} else {
		emitLog("收到配置行：无 AuthKey，走浏览器登录流程（或复用状态目录已有凭据）")
	}
	emitLog("状态目录: " + *stateDir)
	srv := &tsnet.Server{
		Dir:      *stateDir,
		Hostname: *hostname,
		AuthKey:  cfg.AuthKey,
		Logf:     logf,
		UserLogf: userLogf,
	}

	// Up 阻塞到 ipn.Running（AuthKey 生效 / 浏览器登录完成）。未登录期间它会
	// 一直等，登录链接由上面的 userLogf 送来。失败（AuthKey 无效、连不上协调
	// 服务器）= 终态。
	upDone := make(chan error, 1)
	upStatus := make(chan string, 1) // 上线成功时把 DNS 名带出来（空 = 没拿到）
	go func() {
		status, err := srv.Up(ctx)
		if err != nil {
			upDone <- err
			return
		}
		dnsName := ""
		if status != nil && status.Self != nil {
			dnsName = status.Self.DNSName
		}
		upStatus <- dnsName
		upDone <- nil
	}()

	// 就绪链路：Up 成功 → 定 Funnel 域名 → 起 Funnel 监听 → 反代 → ready。
	go func() {
		if err := <-upDone; err != nil {
			if ctx.Err() != nil {
				return
			}
			emitError("tailscale_start_failed", "", "tsnet 上线失败: "+err.Error())
			os.Exit(1)
		}
		emitLog("tsnet 上线成功（ipn.Running）")

		// 域名与「能不能对外提供 https」是两件事，必须分开判：
		// Self.DNSName 只是 MagicDNS 名，永远有值；而 Funnel 要对外服务，
		// 还需要 tailnet 开启 HTTPS 证书 —— netmap 的 DNS.CertDomains 里有名字
		// 才代表证书可签发、.ts.net 的 A 记录已发布。没开时域名在公网 DNS 里
		// 直接 NXDOMAIN，任何设备（包括本机）都打不开，但 ListenFunnel 不会报错
		//（证书是 TLS 握手时懒加载的）。这就是「UI 说已开启、浏览器却打不开」的真因。
		domain := publicOrigin(<-upStatus)
		certDeadline := time.Now().Add(certWaitSeconds * time.Second)
		certDomain := ""
		for {
			for _, candidate := range srv.CertDomains() {
				if origin := publicOrigin(candidate); origin != "" {
					certDomain = origin
					break
				}
			}
			if certDomain != "" || time.Now().After(certDeadline) {
				break
			}
			select {
			case <-ctx.Done():
				return
			case <-time.After(500 * time.Millisecond):
			}
		}
		if certDomain == "" {
			setupURL := "https://tailscale.com/s/https"
			if interactive := queryFunnelSetupURL(ctx, srv); interactive != "" {
				setupURL = interactive
			}
			emitError("funnel_https_required", setupURL,
				"这个 tailnet 还没开启 HTTPS 证书：.ts.net 域名不会发布，任何设备（含本机）都打不开。"+
					"点「去开启 HTTPS 证书」按官方引导在管理台 DNS 页面开启 HTTPS Certificates，再回来重试")
			os.Exit(1)
		}
		if domain == "" {
			domain = certDomain
		}
		emitLog("Funnel 域名: " + strings.TrimPrefix(domain, "https://"))

		// FunnelOnly：只开公网 Funnel 入口（:443），不在 tailnet 内部开监听
		// （本机服务走回环 upstream，P1 不需要 100.x 直连）。
		ln, err := srv.ListenFunnel("tcp", ":443", tsnet.FunnelOnly())
		if err != nil {
			code, setupURL, hint := classifyFunnelFailure(err)
			// 权限/证书没开时，本地状态 API 能给出官方交互式开通链接，比静态页好
			if interactive := queryFunnelSetupURL(ctx, srv); interactive != "" {
				setupURL = interactive
			}
			emitError(code, setupURL, hint+"（"+err.Error()+"）")
			os.Exit(1)
		}

		target, err := url.Parse(*upstream)
		if err != nil || target.Scheme != "http" || target.Host == "" {
			emitError("invalid_upstream", "", "反代目标地址不合法: "+*upstream)
			os.Exit(1)
		}
		emitLog("Funnel :443 监听已建立，反代 → " + *upstream)
		proxy := httputil.NewSingleHostReverseProxy(target)
		// 保留原始 Host 头（客户端看到的是 <机器名>.ts.net，不能被 localserver 改写）
		director := proxy.Director
		proxy.Director = func(request *http.Request) {
			publicHost := request.Host
			director(request)
			request.Host = publicHost
		}
		proxy.ErrorHandler = func(w http.ResponseWriter, _ *http.Request, _ error) {
			http.Error(w, "anyboot gateway unavailable", http.StatusBadGateway)
		}
		httpSrv := &http.Server{
			Handler:           proxy,
			ReadHeaderTimeout: 10 * time.Second,
			BaseContext:       func(net.Listener) context.Context { return ctx },
		}
		go func() {
			err := httpSrv.Serve(ln)
			if err != nil && !errors.Is(err, http.ErrServerClosed) && ctx.Err() == nil {
				emitError("funnel_serve_failed", "", "Funnel 服务中断: "+err.Error())
				os.Exit(1)
			}
		}()

		emit(event{Event: "state", State: "ready", URL: domain})
		// 自检超时取 --ready-timeout 与环境无关的上限（别让用户等太久）
		probeTimeout := *readyTimeout
		if probeTimeout > 20*time.Second {
			probeTimeout = 20 * time.Second
		}
		go probeFunnel(ctx, domain, probeTimeout)
	}()

	// 命令循环：quit（或任何 EOF）→ 优雅收尾。父进程死 = stdin EOF = 自杀。
	scanner := bufio.NewScanner(os.Stdin)
	for scanner.Scan() {
		line := strings.TrimSpace(scanner.Text())
		if line == "quit" || line == "stop" {
			break
		}
		// 忽略其它行（后续版本在这里扩命令）
	}

	emit(event{Event: "bye"})
	cancel()
	_ = srv.Close()
}
