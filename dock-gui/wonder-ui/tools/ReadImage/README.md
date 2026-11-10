# ReadImage

Wonder 文件 Hofmann · 页面「1 · 界面」· 工具卡组（起点 x=19268）。2026-10-11 按 Dock 真实工具美化（Liquid Glass、紧凑 IDE、中文 UI、强调色 #0a84ff）。

| 文件 | Label | Node id | 真实工具 | GUI 显示 | 画布坐标 | 依据 |
|---|---|---|---|---|---|---|
| `collapsed.jsx` / `collapsed.png` | Tool Card Read Image Collapsed | `men` | `read_file（图片）` | Read · 路径 + 格式/尺寸 + 320×200 缩略图 | (19268, 921) | cordis-spine/src/tools/read_file/image.rs, tool_images.rs, dock-gui/src/components/tools/parts.tsx ToolImage |
| `expanded.jsx` / `expanded.png` | Tool Card Read Image Expanded | `fam` | `read_file（图片）` | Read · 大图 + 元信息 | (20048, 921) | read_file/image.rs (MAX_IMAGE_PAYLOAD_BYTES=768KB, MAX_IMAGE_PIXELS=1MP, MAX_IMAGE_DIMENSION=2000) |
| `failed.jsx` / `failed.png` | Tool Card Read Image Failed | `fri` | `read_file（图片）` | Read · 失败原文 | (20828, 921) | read_file/image.rs image_read_output → "Could not embed image in conversation: …"; ToolCard.tsx 失败分支 |
| `viewer-preview.jsx` / `viewer-preview.png` | Tool Card Read Image Viewer Preview | `gmt` | `read_file（图片）` | 点击缩略图 → ImageViewer | (19268, 1653) | dock-gui/src/components/ImageViewer.tsx (bg-black/80, caption W×H, Esc 关闭) |

## Read 读图片：源码结论

- `read_file` 先按魔数 / 扩展名识别图片（png/jpeg/gif/webp，bmp/tiff/ico 先转 PNG），`sniff_mime` 定 MIME。
- `image_read_output` 返回正文 `"{path}\nImage content included inline"` + `ToolResult.images`（1 张，含 mime、width、height；落盘 `$DOCK_HOME/tool-images/`，每次最多 20 张）。
- 压缩预算：base64 ≤ 768 KB、总像素 ≤ 1,048,576、长边 ≤ 2000、地板 128；JPEG 质量阶梯 85/70/50/40；解码上限 178,956,970 像素。
- 失败时直接给出 `Could not embed image in conversation: …`，不会有图片；纯文本模型（`supports_images=false`）不发图片字节，占位文案改为一句说明。
- GUI：`ToolCard.tsx` 现在只在**展开**后渲染 `ToolImages`；缩略图按原图比例缩放，最大 320×200（`parts.tsx`），标题 `W×H`，点击打开 `ImageViewer`（bg-black/80，Esc 关闭）。
- 本组设计稿把缩略图放到**折叠态**工具行下方（用户新需求），属于 GUI 行为改动；展开态显示 576×360 大图 + 元信息；`viewer-preview` 对应 ImageViewer。
- 示例图：Wonder 上传的 Dock 分屏界面截图（1024×640，见 `wonder-ui/reference/20-dock-shell-split-panels.png`）。
