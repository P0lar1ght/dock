//! 后台任务（`Jobs`）的回归用例。
//!
//! 全部针对 2026-09 核实出的四个缺陷，先红后绿：
//!
//! 1. 运行期 `snapshot().output` 恒为空 —— `run_job` 只在进程退出后才写一次，
//!    所以 `get_task_output` 对运行中的任务毫无信息，TUI 的 tasks pane 同理。
//! 2. stderr 超过管道缓冲（64KB）即永久挂死 —— 旧实现先 `read_to_end(stdout)`
//!    读到 EOF 才轮到 stderr，而 stdout 的 EOF 要等进程退出，进程又卡在写
//!    stderr 上。
//! 3. 输出没有任何上限 —— `read_file` 会截断，job 不会。
//! 4. `kill` 在 `job.child` 锁上与 `wait` 抢锁。

use std::time::{Duration, Instant};

use cordis_spine::Jobs;

/// 轮询到 `pred` 为真或超时。返回是否命中，避免各用例各写一遍 sleep 循环。
async fn until(timeout: Duration, mut pred: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if pred() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// 缺陷 1：任务还在跑的时候就该能看到已经产出的那部分。
#[tokio::test]
async fn snapshot_shows_output_while_still_running() {
    let jobs = Jobs::new();
    // 每 200ms 一行，总共约 2s。第一行在 ~0ms 就该可见。
    let id = jobs.start("for i in 1 2 3 4 5 6 7 8 9 10; do echo tick-$i; sleep 0.2; done");

    let saw_partial = until(Duration::from_millis(1500), || {
        jobs.snapshot(&id)
            .is_some_and(|s| !s.done && s.output.contains("tick-1"))
    })
    .await;

    assert!(
        saw_partial,
        "任务运行期间应能读到已产出的输出，实际快照：{:?}",
        jobs.snapshot(&id).map(|s| (s.done, s.output))
    );
}

/// 缺陷 2：往 stderr 写满管道缓冲不能把任务卡死。
/// cargo / npm 的进度都走 stderr，这是日常路径不是边角。
#[tokio::test]
async fn large_stderr_does_not_deadlock() {
    let jobs = Jobs::new();
    let id = jobs.start("yes ERRLINE0123456789 | head -20000 >&2; echo finished-on-stdout");

    let done = until(Duration::from_secs(20), || {
        jobs.snapshot(&id).is_some_and(|s| s.done)
    })
    .await;

    assert!(
        done,
        "往 stderr 写 200KB 的任务应能正常结束，实际一直 running"
    );
    let out = jobs.snapshot(&id).unwrap().output;
    assert!(
        out.contains("finished-on-stdout"),
        "stdout 的收尾行应当保留：{out}"
    );
}

/// 缺陷 2 的对称情形：stdout 侧写满也不能卡死。
#[tokio::test]
async fn large_stdout_does_not_deadlock() {
    let jobs = Jobs::new();
    let id = jobs.start("yes OUTLINE0123456789 | head -20000; echo finished-on-stdout");

    let done = until(Duration::from_secs(20), || {
        jobs.snapshot(&id).is_some_and(|s| s.done)
    })
    .await;

    assert!(done, "往 stdout 写 200KB 的任务应能正常结束");
    assert!(jobs
        .snapshot(&id)
        .unwrap()
        .output
        .contains("finished-on-stdout"));
}

/// 缺陷 3：输出要有上限，并且截断这件事要说出来。
#[tokio::test]
async fn output_is_capped_and_says_so() {
    let jobs = Jobs::new();
    // 约 1MB，远超 20KB 上限。
    let id = jobs.start("yes PADDINGPADDINGPADDING | head -50000");

    let done = until(Duration::from_secs(20), || {
        jobs.snapshot(&id).is_some_and(|s| s.done)
    })
    .await;
    assert!(done, "任务应能结束");

    let out = jobs.snapshot(&id).unwrap().output;
    assert!(
        out.len() < 64 * 1024,
        "输出应被截断到上限附近，实际 {} 字节",
        out.len()
    );
    assert!(
        out.contains("截断"),
        "截断必须在正文里说明，否则模型会把残缺输出当完整结果：{}",
        &out[..out.len().min(200)]
    );
}

/// 缺陷 4：`kill` 不能被运行中的 `wait` 挡住。
#[tokio::test]
async fn kill_returns_promptly_for_a_running_job() {
    let jobs = Jobs::new();
    let id = jobs.start("sleep 30");
    // 让任务真的起来再杀。
    let _ = until(Duration::from_secs(2), || jobs.snapshot(&id).is_some()).await;

    let start = Instant::now();
    let msg = jobs.kill(&id).await;
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_secs(3),
        "kill 应立刻返回，实际耗时 {elapsed:?}（说明被 wait 的锁挡住了）"
    );
    assert!(msg.contains(&id), "返回文案应指明任务：{msg}");

    let done = until(Duration::from_secs(5), || {
        jobs.snapshot(&id).is_some_and(|s| s.done)
    })
    .await;
    assert!(done, "被杀掉的任务应当标记为 done");
}
