import Foundation

/// 探测电脑端的 `GET /health`（免鉴权探活接口）。
///
/// 用途只有一个：拿到 `machineId` —— 一台电脑可能有多块网卡、多个 IP，
/// 客户端靠它认出「这几条地址其实是同一台机器」，把地址并进同一条记录、
/// 只保留一份页面（而不是一个网卡一个标签）。
///
/// 老版本服务端不返回这个字段，这时标识是空串，客户端会退回按「分组名相同」
/// 判断同一台电脑 —— 所以这个探测失败不影响任何功能，只是少一点自动识别。
enum HealthProbe {

    /// 「这台电脑是谁」（`/health` 里的机器标识与机器名）
    struct Machine {
        let id: String
        let name: String
    }

    /// 单次探测超时：这是保存地址后顺带做的事，不能拖住用户
    private static let timeout: TimeInterval = 1.5

    /// 解析响应体（纯逻辑，便于校验）；不是 JSON 时给 nil
    static func parse(_ body: String) -> Machine? {
        guard let data = body.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            return nil
        }
        let id = (object["machineId"] as? String ?? "").trimmedAnyBoot
        let name = (object["machineName"] as? String ?? "").trimmedAnyBoot
        return Machine(id: id, name: name)
    }

    /// 探测一个地址；不通 / 不是 AnyBoot 时给 nil。回调都在主线程。
    static func fetch(host: String, completion: @escaping (Machine?) -> Void) {
        let value = host.trimmedAnyBoot
        guard !value.isEmpty, let url = URL(string: "http://\(value)/health") else {
            completion(nil)
            return
        }
        var request = URLRequest(url: url)
        request.timeoutInterval = timeout
        request.cachePolicy = .reloadIgnoringLocalCacheData
        URLSession.shared.dataTask(with: request) { data, response, _ in
            let machine: Machine? = {
                guard let data,
                      let http = response as? HTTPURLResponse, http.statusCode == 200,
                      let body = String(data: data, encoding: .utf8) else {
                    return nil
                }
                return parse(body)
            }()
            DispatchQueue.main.async { completion(machine) }
        }.resume()
    }

    /// 心跳探测：只看通不通，通了附带往返毫秒（不通给 nil）。回调在主线程。
    ///
    /// 跟 `fetch` 的分工：`fetch` 是「保存地址后顺手认一次机器」，10 秒心跳走这里
    /// —— 不解析内容、超时自己带（心跳要比 `fetch` 宽容一点，弱网别误报）。
    static func ping(host: String, timeout: TimeInterval, completion: @escaping (Double?) -> Void) {
        let value = host.trimmedAnyBoot
        guard !value.isEmpty, let url = URL(string: "http://\(value)/health") else {
            completion(nil)
            return
        }
        var request = URLRequest(url: url)
        request.timeoutInterval = timeout
        request.cachePolicy = .reloadIgnoringLocalCacheData
        let started = Date()
        URLSession.shared.dataTask(with: request) { _, response, _ in
            let ok = (response as? HTTPURLResponse)?.statusCode == 200
            DispatchQueue.main.async {
                completion(ok ? Date().timeIntervalSince(started) * 1000 : nil)
            }
        }.resume()
    }
}
