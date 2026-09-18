//! 后台任务（`Jobs`）的回归用例。
//!
//! 全部针对 2026-09 核实出的四个缺陷，先红后绿：
//!
//! 1. 运行期 `snapshot().output` 恒为空 —— `run_job` 只在进程退出后才写一次，
//!    所以 `job` 对运行中的任务毫无信息，TUI 的 tasks pane 同理。
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

    // 上限是 20KB（头 4KB + 尾 16KB），加上截断提示那一行的开销。断言要卡在
    // 上限本身，松到 64KB 就打不到「是否真的按 20KB 截」这件事。
    const CAP: usize = 20 * 1024;
    const NOTICE_SLACK: usize = 512;
    let out = jobs.snapshot(&id).unwrap().output;
    assert!(
        out.len() <= CAP + NOTICE_SLACK,
        "输出应截断到 {CAP} 字节上限（含提示开销），实际 {} 字节",
        out.len()
    );
    assert!(
        out.len() > CAP - 1024,
        "1MB 的输出应当把上限填满，实际只有 {} 字节 —— 说明截断切多了",
        out.len()
    );
    assert!(
        out.contains("截断"),
        "截断必须在正文里说明，否则模型会把残缺输出当完整结果：{}",
        &out[..out.len().min(200)]
    );
    // 头尾都要留住，不能只剩一头。
    assert!(
        out.starts_with("PADDING"),
        "头部应保留命令开头：{}",
        &out[..out.len().min(80)]
    );
    assert!(
        out.trim_end().ends_with("PADDINGPADDINGPADDING"),
        "尾部应保留收尾（失败原因通常在结尾）"
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

/// 主界面任务条要的是「最新一行进度」，不是整份输出。
#[tokio::test]
async fn list_brief_carries_only_the_last_line() {
    let jobs = Jobs::new();
    let id = jobs.start("echo 第一行; echo 第二行; echo 最后一行; sleep 5");

    let ready = until(Duration::from_secs(5), || {
        jobs.list_brief()
            .iter()
            .any(|j| j.id == id && j.output.contains("最后一行"))
    })
    .await;
    assert!(
        ready,
        "任务条摘要应能读到最新一行，实际：{:?}",
        jobs.list_brief()
            .iter()
            .map(|j| j.output.clone())
            .collect::<Vec<_>>()
    );

    let brief = jobs.list_brief().into_iter().find(|j| j.id == id).unwrap();
    assert_eq!(brief.output, "最后一行", "只要最后一行，不要整份");
    assert!(
        jobs.snapshot(&id).unwrap().output.contains("第一行"),
        "完整快照仍然带全部输出，两者不能混淆"
    );
    let _ = jobs.kill(&id).await;
}

/// 任务条每 80ms 重绘一次，摘要绝不能把整份缓冲拼出来。
#[tokio::test]
async fn list_brief_does_not_render_the_whole_buffer() {
    let jobs = Jobs::new();
    let id = jobs.start("yes PADDINGPADDINGPADDING | head -50000; echo 收尾行");

    let done = until(Duration::from_secs(20), || {
        jobs.snapshot(&id).is_some_and(|s| s.done)
    })
    .await;
    assert!(done, "任务应能结束");

    let full = jobs.snapshot(&id).unwrap().output;
    let brief = jobs.list_brief().into_iter().find(|j| j.id == id).unwrap();
    assert!(
        brief.output.len() <= 200,
        "摘要应限制在单行上限内，实际 {} 字节",
        brief.output.len()
    );
    assert!(
        full.len() > 10 * 1024,
        "对照组：完整快照确实是大块输出（{} 字节）",
        full.len()
    );
    assert_eq!(brief.output, "收尾行", "摘要取的是最后一行");
}

/// `live_count` 只数活着的后台任务：前台命令和已完成的都不算。
#[tokio::test]
async fn live_count_skips_foreground_and_finished() {
    let jobs = Jobs::new();
    let bg = jobs.start("sleep 5");
    let fg = jobs.start_foreground("sleep 5");

    let up = until(Duration::from_secs(2), || jobs.live_count() == 1).await;
    assert!(
        up,
        "只该数到那条后台任务，实际 {}（前台命令不进任务条）",
        jobs.live_count()
    );

    let _ = jobs.kill(&bg).await;
    let _ = jobs.kill(&fg).await;
    let drained = until(Duration::from_secs(5), || jobs.live_count() == 0).await;
    assert!(drained, "都结束后应当归零");
}
