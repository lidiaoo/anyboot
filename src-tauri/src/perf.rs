//! 扫描耗时埋点（只写 trace，不参与任何逻辑判断）。
//!
//! 为什么要有它：`trace.log` 里原来只有一条「完成 耗时=Nms」的总账。知道慢，却不
//! 知道慢在哪一步 —— 只能靠猜。实际上就猜错过一次：直觉指向 APFS 整盘解析，实测
//! 大头却是**三次 PowerShell 冷启动**。与其继续猜，不如让每一步自己报数。
//!
//! 用法：函数体第一行 `let _span = perf::span("xxx");`，`refresh_and_update` 入口
//! `perf::begin()` 清表、出口 `perf::flush()` 汇总成一张按耗时降序的表。
//!
//! 口径：span 是**含子**的（外层把内层算进去了），所以各行相加会大于总耗时。这是
//! 刻意的 —— 要看的是「这段占多少」，不是「这段独占多少」。读表时自上而下，先看
//! 最外层那几段，再看它们的子段。
//!
//! 只在 debug_trace 打开时才真的有输出：`span()` 本身不做任何判断（Drop 里只是往
//! 内存 push 一条），`flush()` 发现 trace 关闭就丢掉 —— 埋点不能成为新的开销。

use super::*;
use std::cell::RefCell;
use std::time::Instant;

// `(名字, 耗时 ms)`，按调用结束顺序追加；`flush` 时再聚合成一张表。
//
// **按线程隔离**（不是全局 Mutex）：启动时 `thread::spawn(refresh_and_update)` 和
// 前端触发的刷新会**并发**跑两轮，全局表会被「后开始的那轮」`begin()` 清掉，
// 于是两份日志互相缺段（实测第一版就是这样：`channels.all` 只 1 次，可它的
// 子段 `esp.resolve` 却有 2 次）。thread_local 让每轮各记各的，天然不打架。
//
// 能这么做的前提：一轮扫描（`refresh_and_update` → `refresh_grub` / `refresh_channels`
// → `preview` → `apfs_reader`）全程同步、不跨线程。哪天把其中一段挪进线程池，
// 那段就得改成显式回传耗时，否则会静默丢数字。
thread_local! {
    static SPANS: RefCell<Vec<(&'static str, u128)>> = const { RefCell::new(Vec::new()) };
}

/// 扫一轮开始：清空上一轮的表。不清的话同一线程上的两轮会混在一起。
pub(crate) fn begin() {
    SPANS.with(|spans| spans.borrow_mut().clear());
}

/// 一段计时的 RAII 把手。**别写 `let _ = perf::span(..)`** —— 那样绑定立刻丢弃，
/// 记下来的是 0ms。要 `let _span = ...`（下划线开头的名字不会触发 unused 警告）。
pub(crate) struct Span {
    name: &'static str,
    started: Instant,
    /// `flush` 自己不再埋点（否则 flush 会记录自己，且永远少最后一条）
    enabled: bool,
}

impl Drop for Span {
    fn drop(&mut self) {
        if !self.enabled {
            return;
        }
        let ms = self.started.elapsed().as_millis();
        SPANS.with(|spans| spans.borrow_mut().push((self.name, ms)));
    }
}

pub(crate) fn span(name: &'static str) -> Span {
    Span {
        name,
        started: Instant::now(),
        enabled: true,
    }
}

/// 同名累加：`(名字, 合计 ms, 次数)`，按合计降序。
///
/// 同名多次是常态（`oc_menu` 对每个候选卷都要读一次卷标），拆成多行反而看不出
/// 这一类总共吃了多少时间。抽成纯函数是为了能单测排序与合并。
fn aggregate(spans: &[(&'static str, u128)]) -> Vec<(&'static str, u128, usize)> {
    let mut out: Vec<(&'static str, u128, usize)> = Vec::with_capacity(spans.len());
    for (name, ms) in spans {
        match out.iter_mut().find(|row| row.0 == *name) {
            Some(row) => {
                row.1 += *ms;
                row.2 += 1;
            }
            None => out.push((name, *ms, 1)),
        }
    }
    // 合计降序；相同时按名字排，保证两次扫描的日志顺序一致（好对比 diff）
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    out
}

/// 一轮扫描结束：把表打出来。返回本次记录到的段数（0 = 没有埋点数据）。
///
/// 参数 `total_ms` 是外层的总耗时（`refresh_and_update` 自己算的那个），只用来算
/// 百分比 —— 不在这里重新计时，否则会和总账那条对不上。
pub(crate) fn flush(app: &AppHandle, total_ms: u128) -> usize {
    let spans = SPANS.with(|spans| std::mem::take(&mut *spans.borrow_mut()));
    // trace 关着就整表丢掉：埋点不能变成「为了不输出还花时间拼字符串」
    if !trace_enabled(app) {
        return 0;
    }
    if spans.is_empty() {
        return 0;
    }
    let rows = aggregate(&spans);
    // 没有占比的分母就没意义：总账为 0（极快或异常）时只印绝对值
    let denom = if total_ms == 0 { 1 } else { total_ms };
    for (name, ms, count) in &rows {
        trace(
            app,
            "perf",
            &format!(
                "  {name:<26} {ms:>6}ms {:>3}% x{count}",
                ms * 100 / denom
            ),
        );
    }
    trace(
        app,
        "perf",
        &format!("合计 {} 段 / 总耗时 {total_ms}ms（各段含子，相加会超总账）", rows.len()),
    );
    rows.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregates_same_name_and_sorts_desc() {
        let spans = [
            ("a", 10),
            ("b", 300),
            ("a", 5),
            ("c", 20),
        ];
        let rows = aggregate(&spans);
        assert_eq!(rows[0], ("b", 300, 1), "最慢的排最前");
        assert_eq!(rows[1], ("c", 20, 1));
        assert_eq!(rows[2], ("a", 15, 2), "同名累加，次数记 2");
    }

    #[test]
    fn empty_input_yields_empty_table() {
        assert!(aggregate(&[]).is_empty());
    }

    #[test]
    fn span_records_elapsed_on_drop() {
        begin();
        {
            let _span = span("t");
        }
        let spans = SPANS.with(|spans| spans.borrow().clone());
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].0, "t");
    }
}
