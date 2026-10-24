//! 探测专用线程池。
//!
//! 为什么单开一个池：探测是「一堆短命 TCP / TLS 请求 + 等超时」，特征是
//! **数量随候选地址膨胀、单个可能一直等到超时**。以前直接扔给
//! `tauri::async_runtime::spawn_blocking`，用户把超时调大、又开「并发全部」时，
//! 会把 Tauri 全局阻塞池占满 —— 界面上别的命令（读设置、发电源包、列档案）
//! 跟着一起卡住，看起来像「整个应用卡死」。
//!
//! 所以探测走自己的固定线程池：
//! - 线程数 = min(max(CPU, 2), 8)：并发全部时也不会开成几百个线程；
//! - 队列无界：提交永不阻塞调用方（否则并发探测会反向卡住主流程）；
//! - 进程级单例，懒创建 —— 不探测就一个线程都不开。
//!
//! 仅用于**探测**（`/health` 之类的只读短请求）。带副作用的操作（电源、SSH 转发）
//! 不要往这里塞。

use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

/// 一个待执行任务：自带返回值通道的闭包
type Job = Box<dyn FnOnce() + Send + 'static>;

/// 固定大小线程池（提交无界、执行 FIFO）
pub(crate) struct ProbePool {
    sender: Mutex<Sender<Job>>,
    workers: usize,
}

impl ProbePool {
    fn new(workers: usize) -> Self {
        let (sender, receiver) = mpsc::channel::<Job>();
        // 多个 worker 共享同一个接收端：谁空谁拿（Mutex 保证只有一个在 recv）
        let receiver = Arc::new(Mutex::new(receiver));
        for index in 0..workers {
            let rx = Arc::clone(&receiver);
            // 线程起不来（极端资源耗尽）不影响主流程：任务留在队列里，
            // 调用方的 recv_timeout 会按超时收束，界面给「探测超时」而不是崩。
            let _ = thread::Builder::new()
                .name(format!("anyboot-probe-{index}"))
                .spawn(move || loop {
                    let job = {
                        let guard = match rx.lock() {
                            Ok(guard) => guard,
                            Err(poisoned) => poisoned.into_inner(),
                        };
                        guard.recv()
                    };
                    match job {
                        Ok(job) => job(),
                        Err(_) => break, // 发送端销毁 = 进程收尾
                    }
                });
        }
        Self {
            sender: Mutex::new(sender),
            workers,
        }
    }

    /// 提交任务（不阻塞；池满就排队）
    pub(crate) fn execute(&self, job: Job) {
        let sender = match self.sender.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        // 接收端全没了（理论上只有进程退出）→ 任务直接丢弃，调用方按超时收束
        let _ = sender.send(job);
    }

    /// 线程数（诊断 / 单测用）
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn workers(&self) -> usize {
        self.workers
    }
}

/// 进程级单例：第一次探测时才建线程
pub(crate) fn pool() -> &'static ProbePool {
    static POOL: OnceLock<ProbePool> = OnceLock::new();
    POOL.get_or_init(|| {
        let cpus = thread::available_parallelism()
            .map(|value| value.get())
            .unwrap_or(4);
        ProbePool::new(cpus.clamp(2, 8))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn runs_jobs_and_returns_values() {
        let (tx, rx) = mpsc::channel::<u32>();
        for value in 0..4u32 {
            let tx = tx.clone();
            pool().execute(Box::new(move || {
                let _ = tx.send(value * 2);
            }));
        }
        drop(tx);
        let mut got: Vec<u32> = Vec::new();
        while let Ok(value) = rx.recv_timeout(std::time::Duration::from_secs(5)) {
            got.push(value);
        }
        got.sort_unstable();
        assert_eq!(got, vec![0, 2, 4, 6]);
    }

    #[test]
    fn worker_count_is_bounded() {
        let workers = pool().workers();
        assert!((2..=8).contains(&workers), "线程数应夹在 2..=8，实际 {workers}");
    }
}
