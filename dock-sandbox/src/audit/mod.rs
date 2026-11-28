//! 审计：沙箱的「观测」与「审计」——
//!
//! - [`ca`]：Dock 自己的根证书，出口代理按主机现签叶子证书。
//! - [`proxy`]：出口代理。命令经环境变量走它；HTTPS 在这里解密（客户端不信 Dock 根证书、
//!   只说 h2 的连接原样转发、只记元数据），每次请求 / 每条隧道发一条 [`record::Flow`]。
//! - [`store`]：只追加的审计日志，每行带上一行的哈希，篡改看得出来。
//! - [`graph`]：把一段审计记录汇总成链路图（Mermaid / 结构化）、时间线和导出报告。
//! - [`packet`]：按条读回一次出网的请求包 / 响应包（正文解压），给界面原样显示。
//! - [`ws`]：WebSocket 握手之后的消息：代理在转发的字节里认帧（含 permessage-deflate），按段记。
//!
//! 接进 Dock 的是 spine 的 `sandbox` 插件（开着沙箱的页才记）；浏览器的网络记录由 `dock mcp browser` 自己写。

pub mod ca;
pub mod env;
pub mod graph;
pub mod packet;
pub mod proxy;
pub mod record;
pub mod store;
pub mod ws;
