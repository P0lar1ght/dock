//! 内置技能的嵌入清单：编译期把 `builtin/<skill>/…` 打进二进制，
//! 启动时由 `discover::materialize_bundled` 写到 `$DOCK_HOME/bundled/skills/`
//! （对齐 Grok 的 bundled 缓存：只写缓存目录，永不写 `~/.dock/skills/`；
//! 同名技能被仓库/用户/项目层覆盖）。
//!
//! 内置只收面向用户的 dock 自述类技能（使用指南、配置手册），内容不依赖
//! dock 源码；通用技能放仓库 `skills/` 目录，不进二进制。

/// `(builtin 内相对路径, 文件内容)`。路径即物化路径。
pub static BUILTIN_FILES: &[(&str, &str)] = &[
    (
        "dock-config/SKILL.md",
        include_str!("builtin/dock-config/SKILL.md"),
    ),
    (
        "dock-guide/SKILL.md",
        include_str!("builtin/dock-guide/SKILL.md"),
    ),
];

/// 内置技能目录名（去重自 BUILTIN_FILES 的首段）。测试用；物化直接遍历表。
#[cfg(test)]
pub fn builtin_skill_dirs() -> Vec<&'static str> {
    let mut dirs: Vec<&'static str> = BUILTIN_FILES
        .iter()
        .filter_map(|(p, _)| p.split('/').next())
        .collect();
    dirs.dedup(); // 表已按路径排序，同目录相邻
    dirs
}
