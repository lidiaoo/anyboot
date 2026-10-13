/* AnyBoot AppImage 垫片：在一切 X 调用发生前调用 XInitThreads()。
 *
 * 背景：WebKitGTK 的加速合成线程与 GTK 主线程并发使用同一条 X 连接，
 * 若 XInitThreads 未在首个 Xlib 调用前执行，xcb 会以
 * "multi-threaded client and XInitThreads has not been called" 断言自杀。
 * 应用代码没调它（CachyOS 上靠 NVIDIA 检测关闭 DMA-BUF 渲染器绕开了该路径，
 * 其他 GPU 的机器就会踩雷），因此打包侧用 LD_PRELOAD 垫片兜底。
 * 经 zig 以 glibc 2.35 目标编译，垫片自身不抬高包门槛。
 */
#include <dlfcn.h>

__attribute__((constructor))
static void anyboot_xinitthreads(void) {
    void *h = dlopen("libX11.so.6", RTLD_LAZY | RTLD_GLOBAL);
    if (!h)
        return;
    int (*f)(void) = (int (*)(void))dlsym(h, "XInitThreads");
    if (f)
        (void)f();
}
