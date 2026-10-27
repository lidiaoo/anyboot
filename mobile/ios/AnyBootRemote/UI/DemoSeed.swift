#if DEBUG
import Foundation

/// **仅 Debug**：用启动参数种一条样例档案，供模拟器验收界面用。
///
/// 为什么不用「直接写 UserDefaults plist」：模拟器上的 cfprefsd 会缓存并回滚外部写入，
/// 写进去读不到（实测）。启动参数由 App 自己在 `ServerStore` 首次读取前写入，稳。
///
/// 用法：
/// ```
/// xcrun simctl launch booted com.anyboot.AnyBootRemote \
///   -ABSeedDemo -ABSeedHost=192.168.0.120:12300 -ABSeedToken=Aa123456.
/// ```
///
/// 截图核对时再加两档直达参数（同样只在 Debug 包里有效）：
/// - `-ABSeedJumpHost=127.0.0.1:22` —— 「家里 NAS」（密钥认证那台）的地址
/// - `-ABSeedPwdHost=127.0.0.1:22`  —— 「东京 VPS」（密码认证那台）的地址，
///   验收 TOFU 主机指纹 / 密码策略时指向本机 sshd 即可走真实握手
///   （`-ABSeedSavedPassword=1` 才会预写钥匙串密码，否则先进应急密码框）
/// - `-ABPage=computers|computer|servers|about` —— 落哪个一级页
///   （`computers` = 电脑页/多标签，`computer` = 电脑列表；名字是历史包袱，别按字面理解）
/// - `-ABServerPage=servers|keys|files|sessions|transfers` —— 服务器模块内的页
///
/// 对齐 PC/Android 侧的 `ANYBOOT_TEST_OPEN_*` 钩子思路：验证入口只存在于调试包。
enum DemoSeed {

    static func applyIfRequested() {
        let args = ProcessInfo.processInfo.arguments
        let demo = args.contains("-ABSeedDemo")

        func value(_ key: String, _ fallback: String) -> String {
            let prefix = "-\(key)="
            return args.first { $0.hasPrefix(prefix) }?
                .replacingOccurrences(of: prefix, with: "") ?? fallback
        }

        if demo {
            seedComputers(host: value("ABSeedHost", "192.168.0.120:12300"),
                          token: value("ABSeedToken", ""),
                          viaJump: value("ABSeedViaJump", "") == "1")
            // 验收「存了密码就不再弹应急框」：给密码认证的那台写一条钥匙串条目
            seedServers(host: value("ABSeedJumpHost", "192.168.0.9:22"),
                        pwdHost: value("ABSeedPwdHost", "203.0.113.7:22"),
                        savedPassword: value("ABSeedSavedPassword", "") == "1")
            // 真连接验收：种一台密钥认证的服务器（私钥 base64 由启动参数带进来）
            let pem = value("ABSeedKeyPEM", "")
            if !pem.isEmpty {
                seedKeyLogin(host: value("ABSeedKeyHost", "127.0.0.1:2222"),
                             user: value("ABSeedKeyUser", "jianghh"),
                             pemBase64: pem,
                             resetTofu: value("ABSeedResetTofu", "") == "1")
            }
        }
        // App.init 跑在主线程上；导航状态是 @MainActor，这里断言一下更明确
        MainActor.assumeIsolated {
            applyNavigation(page: value("ABPage", ""),
                            serverPage: value("ABServerPage", ""),
                            terminal: value("ABTerminal", ""),
                            drawer: value("ABDrawer", ""))
        }
    }

    /// 服务器模块的样例：两台跳板机 + 一把密钥（服务器页 / 密钥页不空着）
    ///
    /// `savedPassword` = true 时给「东京 VPS」预先写一条钥匙串密码
    /// （验收「已存密码 → 进终端不再弹应急框」，对齐 Android needsManualPassword 的口径）。
    private static func seedServers(host: String, pwdHost: String, savedPassword: Bool) {
        let defaults = UserDefaults.standard

        var key = SshKeyEntry()
        key.id = "key-demo"
        key.name = "MacBook · ed25519"
        key.algorithm = "ssh-ed25519"
        key.fingerprint = "SHA256:AbCdEfGhIjKlMnOpQrStUvWxYz0123456789abcd"
        key.publicKey = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIDemoKeyMaterialOnly anyboot"
        key.privateRef = ""

        var nas = JumpServer()
        nas.id = "srv-nas"
        nas.name = "家里 NAS"
        nas.host = host
        nas.username = "root"
        nas.authType = JumpServer.authKey
        nas.keyId = key.id
        nas.note = "内网跳板"
        nas.lastRttMs = 12

        var vps = JumpServer()
        vps.id = "srv-vps"
        vps.name = "东京 VPS"
        vps.host = pwdHost
        vps.username = "jiang"
        vps.authType = JumpServer.authPassword
        vps.note = "公网入口"
        vps.lastRttMs = 86
        if savedPassword {
            let ref = "pwd-srv-vps"
            let ok = KeyVault.save("demo-pass", account: ref)
            NSLog("[ABSEED] 写入演示密码=\(ok) 回读长度=\(KeyVault.load(account: ref).count)")
            vps.passwordRef = ref
        } else if !vps.passwordRef.isEmpty {
            vps.passwordRef = ""
        }

        if let data = try? JSONEncoder().encode([nas, vps]) {
            defaults.set(data, forKey: "jump_servers")
        }
        if let data = try? JSONEncoder().encode([key]) {
            defaults.set(data, forKey: "ssh_keys")
        }
    }

    /// 真连接验收用：把一把测试私钥（base64 PEM）写进钥匙串，并种一台密钥认证的服务器。
    ///
    /// 为什么要这档钩子：TOFU → 握手 → PTY → 界面挂载这条链路，只有真握手才验得出来。
    /// 私钥走参数传（模拟器 App 读不到宿主机任意路径），只在 Debug 包里生效。
    private static func seedKeyLogin(host: String, user: String, pemBase64: String, resetTofu: Bool) {
        guard let raw = Data(base64Encoded: pemBase64),
              let pem = String(data: raw, encoding: .utf8), !pem.isEmpty else {
            NSLog("[ABSEED] 测试私钥解码失败，跳过")
            return
        }

        var key = SshKeyEntry()
        key.id = "key-live"
        key.name = "本机 sshd 测试密钥"
        key.algorithm = "ssh-ed25519"
        key.privateRef = "priv-key-live"
        let ok = KeyVault.save(pem, account: key.privateRef)
        NSLog("[ABSEED] 写入测试私钥=\(ok) 回读长度=\(KeyVault.load(account: key.privateRef).count)")

        var live = JumpServer()
        live.id = "srv-live"
        live.name = "本机 sshd"
        live.host = host
        live.username = user
        live.authType = JumpServer.authKey
        live.keyId = key.id
        live.note = "真连接验收"

        let defaults = UserDefaults.standard
        var servers: [JumpServer] = []
        if let data = defaults.data(forKey: "jump_servers"),
           let decoded = try? JSONDecoder().decode([JumpServer].self, from: data) {
            servers = decoded.filter { $0.id != live.id }
        }
        servers.append(live)
        if let data = try? JSONEncoder().encode(servers) {
            defaults.set(data, forKey: "jump_servers")
        }

        var keys: [SshKeyEntry] = []
        if let data = defaults.data(forKey: "ssh_keys"),
           let decoded = try? JSONDecoder().decode([SshKeyEntry].self, from: data) {
            keys = decoded.filter { $0.id != key.id }
        }
        keys.append(key)
        if let data = try? JSONEncoder().encode(keys) {
            defaults.set(data, forKey: "ssh_keys")
        }

        if resetTofu {
            defaults.removeObject(forKey: "tofu_\(host)")
        }
        NSLog("[ABSEED] 测试服务器 \(host) user=\(user) 重置指纹=\(resetTofu)")
    }

    /// 一级页 / 服务器模块内页的直达（截图核对用）
    @MainActor
    private static func applyNavigation(page: String, serverPage: String, terminal: String, drawer: String) {
        switch page {
        case "computer": ABNavigator.shared.page = .computer
        case "computers": ABNavigator.shared.page = .computers
        case "servers": ABNavigator.shared.page = .servers
        case "about": ABNavigator.shared.page = .about
        default: break
        }
        if !terminal.isEmpty {
            ABNavigator.shared.page = .servers
            ServerModuleStore.shared.open(.servers)
            // 首帧就置位的话 SwiftUI 会漏掉这次 fullScreenCover 呈现，推迟一拍
            Task { @MainActor in
                try? await Task.sleep(nanoseconds: 700_000_000)
                ServerModuleStore.shared.openTerminal(jumpId: terminal)
            }
            return
        }
        // 抽屉直达（截图核对用）：启动后把侧边栏打开。首帧置位会被动画吞掉，推迟一拍
        if drawer == "1" {
            Task { @MainActor in
                try? await Task.sleep(nanoseconds: 500_000_000)
                ABNavigator.shared.openDrawer()
            }
        }
        guard !serverPage.isEmpty else { return }
        let target: ServerModuleStore.Page?
        switch serverPage {
        case "servers": target = .servers
        case "keys": target = .keys
        case "files": target = .files
        case "sessions": target = .sessions
        case "transfers": target = .transfers
        default: target = nil
        }
        if let target { ServerModuleStore.shared.open(target) }
    }

    private static func seedComputers(host: String, token: String, viaJump: Bool) {

        var profile = ServerProfile()
        profile.id = "p-seed"
        profile.group = "书房"
        profile.name = "黑苹果"
        profile.addresses = [host]
        profile.token = token
        profile.macs = [host: ["aa:bb:cc:dd:ee:ff"]]
        profile.routeOrder = ServerProfile.defaultRouteOrder()
        // 跳板机验收：关直连、清掉远程配对，逼探测/加载只能走 jump 通道
        if viaJump {
            profile.routeDisabled = [ServerProfile.routeLan]
            profile.jumpServerId = "srv-live"   // seedKeyLogin 种的那台
            profile.pairs = []
        } else {
            profile.routeDisabled = [ServerProfile.routeJump]
        }
        profile.routeTimeoutsMs = [ServerProfile.routeTunnel: 5000,
                                   ServerProfile.routeFunnel: 8000]
        profile.blockTimeoutsMs = [RemotePair.kindCloudflare: 5000]
        profile.blockProbeMode = [RemotePair.kindTS: ServerProfile.probeParallelAll]

        // 三条远程配对：Cloudflare / TS 直连 / TS Funnel（最后一条停用，验置灰）
        var cloudflare = RemotePair(kind: RemotePair.kindCloudflare,
                                    addr: "https://demo-x1.trycloudflare.com")
        cloudflare.id = "pair-cf"
        cloudflare.token = "ptok-cf"
        cloudflare.mid = "m-cf"
        cloudflare.os = "macos"
        cloudflare.host = "黑苹果"

        var direct = RemotePair(kind: RemotePair.kindTS, addr: "http://100.64.0.7:12300")
        direct.id = "pair-ts"
        direct.token = "ptok-ts"
        direct.mid = "m-ts"
        direct.os = "macos"
        direct.host = "黑苹果"

        var funnel = RemotePair(kind: RemotePair.kindTS,
                                addr: "https://demo.tailc5a8ef.ts.net")
        funnel.id = "pair-fn"
        funnel.token = "ptok-fn"
        funnel.mid = "m-fn"
        funnel.os = "macos"
        funnel.host = "黑苹果"
        funnel.enabled = false

        profile.pairs = [cloudflare, direct, funnel]

        var wake = PowerRoute(action: PowerRoute.actionWake, target: "", port: 9)
        wake.burst = 10
        wake.retry = 10
        wake.mode = "auto"
        profile.powerRoutes = [wake]

        let defaults = UserDefaults.standard
        if let data = try? JSONEncoder().encode([profile]) {
            defaults.set(data, forKey: "servers")
            defaults.set(profile.id, forKey: "active_id")
            defaults.removeObject(forKey: "migrated_multi_server")
            defaults.set(true, forKey: "migrated_multi_server")
        }
    }
}
#endif
