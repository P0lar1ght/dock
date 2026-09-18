//! 内置技能的嵌入清单：编译期把 `builtin/<skill>/…` 打进二进制，
//! 启动时由 `discover::materialize_bundled` 写到 `$DOCK_HOME/bundled/skills/`
//! （对齐 Grok 的 bundled 缓存：只写缓存目录，永不写 `~/.dock/skills/`；
//! 同名技能被仓库/用户/项目层覆盖）。
//!
//! 内置只收**产品自带**的技能：dock 自述类（使用指南、配置手册），以及产品能力
//! 自己的创作手册（`create-workflow` —— `workflow` 工具的描述直接指着它，装了
//! dock 就得有）。内容只依赖对外的产品面，不跟内部源码走。通用技能放仓库
//! `skills/` 目录，不进二进制。

/// `(builtin 内相对路径, 文件内容)`。路径即物化路径。
pub static BUILTIN_FILES: &[(&str, &str)] = &[
    (
        "create-workflow/SKILL.md",
        include_str!("builtin/create-workflow/SKILL.md"),
    ),
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
