// AnyBoot 远程客户端（Android）—— 只是一个 WebView 外壳：
// 它加载电脑端 AnyBoot 内置 HTTP 服务（默认 12300）托管的同一份前端页面，
// 页面上所有 /api/* 调用都是同源请求，等价于用手机浏览器打开那个地址。
// 所以这里不存在「把 Rust 核心编译到安卓」这回事，也不需要 NDK。
plugins {
    id("com.android.application") version "8.7.3" apply false
}
