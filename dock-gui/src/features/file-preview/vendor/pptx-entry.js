// @jvmr/pptx-to-html 打成一个自带依赖的 IIFE（scripts/bundle-frame-libs.mjs），放进沙箱 iframe 用。
import { pptxToHtml } from "@jvmr/pptx-to-html";
window.pptxToHtml = pptxToHtml;
