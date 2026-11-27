//! 手动试：经出口代理跑一条命令，跑完列出它的出网，并导出一份审计报告（链路图 + 时间线）。
//!
//! ```sh
//! cargo run -p dock-audit --example egress -- curl -s https://example.com -o /dev/null
//! ```

use std::sync::{Arc, Mutex};

use dock_audit::ca::Authority;
use dock_audit::proxy::{Options, Proxy};
use dock_audit::record::Flow;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((program, rest)) = args.split_first() else {
        eprintln!("usage: egress <command> [args…]");
        std::process::exit(2);
    };
    let dir = std::env::temp_dir().join("dock-audit-example");
    let ca = Arc::new(Authority::load_or_create(&dir).expect("CA"));
    let bundle = ca.bundle().expect("bundle");
    let flows: Arc<Mutex<Vec<Flow>>> = Arc::default();
    let audit_dir = dir.join(format!("run-{}", dock_audit::record::now_ms()));
    let log =
        Arc::new(dock_audit::store::AuditLog::open(&audit_dir, "events.jsonl").expect("audit log"));
    let command = args.join(" ");
    log.record(
        "tool_call",
        serde_json::json!({ "tool": "bash", "call_id": "c1", "args": command }),
    );
    log.record("command", serde_json::json!({ "tool": "bash", "job": "example", "program": program, "command": command }));
    let sink_log = log.clone();
    let sink = flows.clone();
    let proxy = Proxy::new(
        Some(ca.clone()),
        Options::default(),
        Arc::new(move |f: Flow| {
            sink_log.flow(f.clone());
            sink.lock().unwrap().push(f);
        }),
        Vec::new(),
        None,
    );
    let listener = proxy.listen("example").await.expect("listen");
    let env = dock_audit::env::command_env(listener.port, Some((&ca.cert_path(), &bundle)), "");
    let status = tokio::process::Command::new(program)
        .args(rest)
        .envs(env)
        .status()
        .await
        .expect("spawn");
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    log.record(
        "command_end",
        serde_json::json!({ "job": "example", "exit": status.code() }),
    );
    log.record(
        "tool_result",
        serde_json::json!({ "tool": "bash", "call_id": "c1", "is_error": !status.success() }),
    );
    drop(log);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let events = dock_audit::store::read(&audit_dir);
    eprintln!("\n{}", dock_audit::graph::Summary::build(&events).tree());
    match dock_audit::graph::export(&audit_dir, "egress 示例") {
        Ok(out) => eprintln!("report: {}", out.join("report.html").display()),
        Err(e) => eprintln!("export failed: {e}"),
    }
    eprintln!("\n== exit {status}");
    for f in flows.lock().unwrap().iter() {
        eprintln!(
            "{:6} {:5} {}:{} {} {} status={} up={} down={} decrypted={}{}{}",
            f.kind,
            f.scheme,
            f.host,
            f.port,
            f.method.as_deref().unwrap_or("-"),
            f.path.as_deref().unwrap_or(""),
            f.status
                .map(|s| s.to_string())
                .unwrap_or_else(|| "-".into()),
            f.bytes_up,
            f.bytes_down,
            f.decrypted,
            f.note
                .as_deref()
                .map(|n| format!(" note={n}"))
                .unwrap_or_default(),
            f.error
                .as_deref()
                .map(|e| format!(" error={e}"))
                .unwrap_or_default(),
        );
    }
}
