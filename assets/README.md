# 应用图标

“守护计时”：靛蓝紫色盾牌、暂停符号和时钟弧线。窗口、托盘、程序 EXE 与安装器使用同一套设计，原生图标嵌入程序，不依赖用户目录中的图片。

| 文件 | 用途 |
| --- | --- |
| `app-icon.png` | 256 × 256 RGBA PNG，README 展示与设计参考 |
| `app-icon.ico` | 16、20、24、32、40、48、64、128、256 像素图标，用于 Windows EXE 和安装器 |
| `app-icon-32.rgba` | 32 × 32 × 4 字节的原始 RGBA 数据，用于托盘 |
| `app-icon-256.rgba` | 256 × 256 × 4 字节的原始 RGBA 数据，用于窗口 |

RGBA 文件没有头部，按行排列，每像素 R、G、B、A 各一个字节，透明度未经预乘。Rust 在编译时校验字节长度；发布流水线比对实际 EXE、安装器与绿色包中的 ICO 图像内容。

本图标于 2026-09-23 使用内置 imagegen 工具生成，随后用 Pillow 缩放并转换为 ICO 与 RGBA；没有使用雷神或外星仔官方标识。生成源图为 1254 × 1254 透明 PNG，源图保存在 `docs/brand/accelerator-guard-source.png`，仓库同时保存发行所需资源。更换设计时，应从同一张源图以 Lanczos 分别缩放到所需尺寸，使用 `RGBA` 模式保存 PNG 和原始字节，并生成上述所有 ICO 尺寸，一起提交四个资源文件。单独替换 PNG 不会改变已经内嵌的程序图标。

## 生成提示词

```text
Use case: logo-brand. Create one production-ready Windows app icon for 加速器守护 / Accelerator Guard, an independent utility that automatically pauses unused gaming accelerator time and supports multiple accelerator providers. Standalone square icon, one mark only, no mockup or presentation sheet. A clean confident rounded shield with two unmistakable pause bars cut into its center and a restrained single timer arc integrated along one shoulder. Use the existing application's quiet indigo and soft violet identity: a rich indigo shield with subtle violet shading and crisp ivory negative-space pause bars. Large simple shapes readable at 16 and 32 pixels; balanced front-facing geometric silhouette, substantial strokes, no tiny details. Transparent background outside the shield, centered generous consistent margin approximately 12%. No text, letters, digits, watermark, lightning bolt, alien mascot, official accelerator branding, game controller, scenery, dramatic glow, or extra badges. Calm polished software utility identity that looks clear on light and dark Windows taskbars. Output a transparent PNG at high resolution.
```
