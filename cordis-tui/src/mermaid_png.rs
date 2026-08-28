//! Lazy Mermaid PNG via copied `xai-grok-mermaid` (Open Image / Copy Image Path).

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

use xai_grok_mermaid::{
    default_engine, render_checked, MmdcEngine, MermaidEngine, MermaidTheme, RenderLimits,
    RenderParams,
};

use crate::theme::ThemeKind;

pub fn render_open_png(source: &str, prefer_mmdc: bool) -> Result<PathBuf, String> {
    let engine: std::sync::Arc<dyn MermaidEngine> = if prefer_mmdc {
        MmdcEngine::detect()
            .map(|e| std::sync::Arc::new(e) as std::sync::Arc<dyn MermaidEngine>)
            .unwrap_or_else(default_engine)
    } else {
        default_engine()
    };
    let dark = !matches!(crate::theme::Theme::current_kind(), ThemeKind::GrokDay);
    let theme = if dark {
        MermaidTheme::Dark
    } else {
        MermaidTheme::Light
    };
    let params = RenderParams::for_os_viewer(theme, 800, 8192);
    let diagram = render_checked(engine.as_ref(), source, &params, &RenderLimits::default())
        .map_err(|e| e.to_string())?;
    let dir = std::env::temp_dir().join("dock-mermaid");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut hasher);
    let path = dir.join(format!("{:x}.png", hasher.finish()));
    std::fs::write(&path, &diagram.png).map_err(|e| e.to_string())?;
    Ok(path)
}

pub fn open_path(path: &Path) -> Result<(), String> {
    let mut cmd = if cfg!(target_os = "macos") {
        Command::new("open")
    } else if cfg!(target_os = "windows") {
        Command::new("cmd")
    } else {
        Command::new("xdg-open")
    };
    if cfg!(target_os = "windows") {
        cmd.args(["/C", "start", "", &path.display().to_string()]);
    } else {
        cmd.arg(path);
    }
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}
