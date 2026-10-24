// anyboot-tsnet —— AnyBoot 内置 Tailscale 引擎（tsnet sidecar）。
//
// 设计约定（本项目自定义，clean-room 实现：只依据 tailscale.com/tsnet 公开 API）：
//   - 单进程单职责：把本机 AnyBoot HTTP 服务经 tsnet 的 Funnel 发布成
//     https://<机器名>.<tailnet>.ts.net 公网地址，反向代理到本地 upstream。
//   - stdout：每行一个 JSON 事件（协议见 emit/event），Rust 父进程逐行解析。
//   - stdin：首行为可选配置 {"authkey":"..."}（走管道，不进进程命令行，
//     避免密钥在任务管理器/进程列表里可见）；之后每行一个命令，quit 收尾退出；
//     父进程死亡 → stdin EOF → 自动自杀（不留孤儿进程）。
//   - 状态目录由父进程给（%LOCALAPPDATA%\AnyBoot\tsnet\state），登录凭据
//     首次登录后持久化在那里，之后无需再登录。
//
// tsnet v1.90 的登录语义：Server.Up 阻塞到 ipn.Running；未登录时它不会失败，
// 而是把「To authenticate, visit https://login.tailscale.com/a/…」打进日志流。
// 所以这里把 Logf/UserLogf 接成捕获器：从日志里捞官方登录 URL 发 login_needed，
// 其余日志静音（父进程要的是结构化状态，不是日志流）。
package main

import (
	"bufio"
	"context"
	"encoding/json"
	"flag"
	"fmt"
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

// 官方登录页 URL（日志流里唯一要捞的东西；其余日志一律忽略）。
var loginURLPattern = regexp.MustCompile(`https://login\.tailscale\.com/[A-Za-z0-9?&=_./%-]+`)

// event：stdout 事件（一次一行 JSON）。
//   hello  ：进程起来就发（proto/sidecar 自报家门）
//   state  ：状态机流转（state = starting / login_needed / ready / error）
//   bye    ：正常收尾（父进程看到即知是优雅退出）
type event struct {
	Event    string `json:"event"`
	Proto    int    `json:"proto,omitempty"`
	Sidecar  string `json:"sidecar,omitempty"`
	State    string `json:"state,omitempty"`
	URL      string `json:"url,omitempty"`
	LoginURL string `json:"loginUrl,omitempty"`
	Detail   string `json:"detail,omitempty"`
}

var outMu sync.Mutex

func emit(e event) {
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

func main() {
	stateDir := flag.String("state", "", "tsnet 状态目录（登录凭据持久化在这里）")
	hostname := flag.String("hostname", "anyboot", "tailnet 里的节点名（DNS 名的前缀）")
	upstream := flag.String("upstream", "http://127.0.0.1:12300", "反代目标（本机 AnyBoot HTTP 服务）")
	readyTimeout := flag.Duration("ready-timeout", 90*time.Second, "等登录/Funnel 域名就绪的最长时间")
	flag.Parse()

	emit(event{Event: "hello", Proto: protoVersion, Sidecar: sidecarID})
	emitState("starting", "")

	stdinBuf := bufio.NewReader(os.Stdin)

	// 日志捕获：登录 URL 只发一次事件，其余日志静音。
	var loginMu sync.Mutex
	loginSeen := false
	logf := func(format string, args ...any) {
		msg := fmt.Sprintf(format, args...)
		if u := loginURLPattern.FindString(msg); u != "" {
			loginMu.Lock()
			defer loginMu.Unlock()
			if !loginSeen {
				loginSeen = true
				emit(event{Event: "state", State: "login_needed", LoginURL: u})
			}
		}
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	// tsnet 引擎：状态目录 + 节点名 + 可选 AuthKey（来自 stdin 配置行）。
	cfg := readStdinConfig(stdinBuf)
	srv := &tsnet.Server{
		Dir:      *stateDir,
		Hostname: *hostname,
		AuthKey:  cfg.AuthKey,
		Logf:     logf,
		UserLogf: logf,
	}

	// Up 阻塞到 ipn.Running（AuthKey 生效 / 浏览器登录完成）。失败（如
	// AuthKey 无效、连不上协调服务器）= 终态，进程退出由父进程读到 error。
	upDone := make(chan error, 1)
	go func() {
		_, err := srv.Up(ctx)
		upDone <- err
	}()

	// 就绪链路：Up 成功 → 等 Funnel 域名 → 起 Funnel 监听 → 反代 → ready。
	go func() {
		if err := <-upDone; err != nil {
			emit(event{Event: "state", State: "error", Detail: "tsnet 上线失败: " + err.Error()})
			os.Exit(1)
		}
		var domain string
		deadline := time.Now().Add(*readyTimeout)
		for {
			if domains := srv.CertDomains(); len(domains) > 0 {
				domain = domains[0]
				break
			}
			if time.Now().After(deadline) {
				emit(event{Event: "state", State: "error",
					Detail: "上线成功但等不到 Funnel 域名（超时）"})
				os.Exit(1)
			}
			select {
			case <-ctx.Done():
				return
			case <-time.After(1 * time.Second):
			}
		}
		funnelURL := "https://" + domain

		// FunnelOnly：只开公网 Funnel 入口（:443），不在 tailnet 内部开监听
		// （本机服务走回环 upstream，P1 不需要 100.x 直连）。
		ln, err := srv.ListenFunnel("tcp", ":443", tsnet.FunnelOnly())
		if err != nil {
			emit(event{Event: "state", State: "error", Detail: "开启 Funnel 失败: " + err.Error()})
			os.Exit(1)
		}

		target, err := url.Parse(*upstream)
		if err != nil {
			emit(event{Event: "state", State: "error", Detail: "upstream 地址不合法: " + err.Error()})
			os.Exit(1)
		}
		proxy := httputil.NewSingleHostReverseProxy(target)
		httpSrv := &http.Server{
			Handler:     proxy,
			BaseContext: func(net.Listener) context.Context { return ctx },
		}
		go func() {
			if err := httpSrv.Serve(ln); err != nil && ctx.Err() == nil {
				emit(event{Event: "state", State: "error", Detail: "Funnel 服务中断: " + err.Error()})
				os.Exit(1)
			}
		}()

		emit(event{Event: "state", State: "ready", URL: funnelURL})
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
