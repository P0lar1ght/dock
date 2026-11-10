// 输入法还在组字、或刚用回车确认候选词时的按键，不能当成「发送」「确认」。
//
// 只看 `isComposing` 不够：WebKit（Tauri 在 macOS 上的 WKWebView、Safari）里用回车确认
// 候选词时，`compositionend` 先于 `keydown` 触发，那次 keydown 的 `isComposing` 已经是
// false，只剩 `keyCode` 还是 229。Chromium 两样都对，这条判断对它无害。
export function imeComposing(e: { nativeEvent: KeyboardEvent }): boolean {
  return e.nativeEvent.isComposing || e.nativeEvent.keyCode === 229;
}
