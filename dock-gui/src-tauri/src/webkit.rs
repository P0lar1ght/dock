//! WKWebView 配置：关掉 WebKit 的「网页渲染贴近 60 帧」（`PreferPageRenderingUpdatesNear60FPSEnabled`，
//! 默认开），ProMotion 屏上网页才跑满 120 帧。
//!
//! 这个开关只在建 webview 之前设才生效，页面加载后再改不生效（2026-10-17 用独立 WKWebView 实测：
//! 开着 60 帧、建前关掉 120 帧）。所以主窗口不让 tauri.conf 自动建，在 setup 里带这份配置建；
//! 弹出的聊天 / 面板窗口同样带。
//!
//! 用的是私有 SPI（`+[WKPreferences _features]`、`-[WKPreferences _setEnabled:forFeature:]`）：
//! 先问 `respondsToSelector:`，系统没有就照默认配置走，不会崩。

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{msg_send, sel, ClassType, MainThreadMarker};
use objc2_foundation::{NSArray, NSString};
use objc2_web_kit::{WKPreferences, WKWebViewConfiguration};

const NEAR_60_FPS: &str = "PreferPageRenderingUpdatesNear60FPSEnabled";

/// 新的 webview 配置，帧率不封顶在 60。
pub fn configuration(mtm: MainThreadMarker) -> Retained<WKWebViewConfiguration> {
    let config = unsafe { WKWebViewConfiguration::new(mtm) };
    let prefs = unsafe { config.preferences() };
    unsafe { set_feature(&prefs, NEAR_60_FPS, false) };
    config
}

/// 按 key 找 WebKit 的 feature 开关并设上；找不到 / SPI 不在就什么都不做。
unsafe fn set_feature(prefs: &WKPreferences, key: &str, enabled: bool) {
    let cls = WKPreferences::class();
    let has_features: bool = msg_send![cls, respondsToSelector: sel!(_features)];
    let has_setter: bool = msg_send![prefs, respondsToSelector: sel!(_setEnabled:forFeature:)];
    if !has_features || !has_setter {
        return;
    }
    let features: Option<Retained<NSArray<NSObject>>> = msg_send![cls, _features];
    let Some(features) = features else { return };
    let want = NSString::from_str(key);
    for feature in features.iter() {
        let k: Option<Retained<NSString>> = msg_send![&*feature, key];
        if k.is_some_and(|k| k.isEqualToString(&want)) {
            let feature: &AnyObject = &feature;
            let _: () = msg_send![prefs, _setEnabled: enabled, forFeature: feature];
            return;
        }
    }
}
