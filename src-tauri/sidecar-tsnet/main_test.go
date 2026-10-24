package main

import (
	"errors"
	"testing"
)

// 这些函数决定「什么链接可以交给系统浏览器打开」，是唯一的白名单关口——
// 收紧容易放松难，这里逐条钉死。
func TestOfficialLoginURL(t *testing.T) {
	cases := []struct {
		name string
		in   string
		want string
	}{
		{"正常授权链接", "https://login.tailscale.com/a/1dbb9bb401", "https://login.tailscale.com/a/1dbb9bb401"},
		{"带查询串", "https://login.tailscale.com/a/x?foo=1&bar=%20", "https://login.tailscale.com/a/x?foo=1&bar=%20"},
		{"首尾空白", "  https://login.tailscale.com/a/x  ", "https://login.tailscale.com/a/x"},
		{"http 不放行", "http://login.tailscale.com/a/x", ""},
		{"子域不放行", "https://evil.login.tailscale.com/a/x", ""},
		{"仿冒域不放行", "https://login.tailscale.com.evil.example/a/x", ""},
		{"带端口不放行", "https://login.tailscale.com:8443/a/x", ""},
		{"带凭据不放行", "https://user:pw@login.tailscale.com/a/x", ""},
		{"空串", "", ""},
	}
	for _, c := range cases {
		if got := officialLoginURL(c.in); got != c.want {
			t.Errorf("%s: officialLoginURL(%q) = %q, want %q", c.name, c.in, got, c.want)
		}
	}

	long := "https://login.tailscale.com/a/" + string(make([]byte, maxLinkBytes))
	if got := officialLoginURL(long); got != "" {
		t.Errorf("超长链接应拒绝，得到 %q", got)
	}
}

func TestOfficialSetupURL(t *testing.T) {
	cases := []struct{ in, want string }{
		{"https://tailscale.com/s/no-funnel", "https://tailscale.com/s/no-funnel"},
		{"https://tailscale.com/s/https/", "https://tailscale.com/s/https"},
		{"https://login.tailscale.com/f/funnel?node=abc", "https://login.tailscale.com/f/funnel?node=abc"},
		{"https://example.com/s/no-funnel", ""},
		{"http://tailscale.com/s/https", ""},
		{"", ""},
	}
	for _, c := range cases {
		if got := officialSetupURL(c.in); got != c.want {
			t.Errorf("officialSetupURL(%q) = %q, want %q", c.in, got, c.want)
		}
	}
}

func TestPublicOrigin(t *testing.T) {
	cases := []struct{ in, want string }{
		{"anyboot.tail1a2b.ts.net.", "https://anyboot.tail1a2b.ts.net"},
		{"AnyBoot.Tail1A2B.TS.NET", "https://anyboot.tail1a2b.ts.net"},
		{"https://anyboot.tail1a2b.ts.net", "https://anyboot.tail1a2b.ts.net"},
		{"anyboot.tail1a2b.ts.net/evil", ""},
		{"example.com", ""},
		{"ts.net", ""},
		{"", ""},
	}
	for _, c := range cases {
		if got := publicOrigin(c.in); got != c.want {
			t.Errorf("publicOrigin(%q) = %q, want %q", c.in, got, c.want)
		}
	}
}

func TestClassifyFunnelFailure(t *testing.T) {
	code, setup, hint := classifyFunnelFailure(errors.New(`Funnel not available; "funnel" node attribute not set (https://tailscale.com/s/no-funnel)`))
	if code != "funnel_permission_required" || setup != "https://tailscale.com/s/no-funnel" || hint == "" {
		t.Errorf("权限未开分类错误: %q %q %q", code, setup, hint)
	}
	code, setup, _ = classifyFunnelFailure(errors.New(`Funnel not available; "https" not enabled (https://tailscale.com/s/https)`))
	if code != "funnel_https_required" || setup != "https://tailscale.com/s/https" {
		t.Errorf("HTTPS 未开分类错误: %q %q", code, setup)
	}
	code, setup, hint = classifyFunnelFailure(errors.New("listen tcp :443: bind: address already in use"))
	if code != "funnel_start_failed" || setup != "" || hint == "" {
		t.Errorf("兜底分类错误: %q %q %q", code, setup, hint)
	}
}

// observe 的语义：同一链接只认一次（不刷屏），链接换了要重新认（过期轮换）。
func TestLoginTrackerDedupesButFollowsRotation(t *testing.T) {
	tracker := &loginTracker{}
	line := "To start this tsnet server, restart with TS_AUTHKEY set, or go to: https://login.tailscale.com/a/old"
	if !tracker.observe(line) {
		t.Fatal("登录提示行应被识别（返回 true 以便调用方不再写日志）")
	}
	if tracker.last != "https://login.tailscale.com/a/old" {
		t.Fatalf("链接未记录: %q", tracker.last)
	}
	tracker.observe(line) // 重复行：不发第二条事件，但仍算登录提示行
	if tracker.last != "https://login.tailscale.com/a/old" {
		t.Fatalf("重复行不该改动链接: %q", tracker.last)
	}
	tracker.observe("... go to: https://login.tailscale.com/a/new")
	if tracker.last != "https://login.tailscale.com/a/new" {
		t.Fatalf("轮换后的新链接未生效: %q", tracker.last)
	}
	if tracker.observe("普通日志，没有任何链接") {
		t.Fatal("无链接行不该被识别为登录提示")
	}
}
