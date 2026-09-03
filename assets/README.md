# 应用图标

“守护计时”：深蓝圆角底、青色盾牌、暂停符号和时钟弧线。窗口、托盘、程序 EXE 与安装器使用同一套设计，原生图标嵌入程序，不依赖用户目录中的图片。

| 文件 | 用途 |
| --- | --- |
| `app-icon.png` | 256 × 256 RGBA PNG，README 展示与设计参考 |
| `app-icon.ico` | 16、20、24、32、40、48、64、128、256 像素图标，用于 Windows EXE 和安装器 |
| `app-icon-32.rgba` | 32 × 32 × 4 字节的原始 RGBA 数据，用于托盘 |
| `app-icon-256.rgba` | 256 × 256 × 4 字节的原始 RGBA 数据，用于窗口 |

RGBA 文件没有头部，按行排列，每像素 R、G、B、A 各一个字节，透明度未经预乘。Rust 在编译时校验字节长度；发布流水线比对实际 EXE、安装器与绿色包中的 ICO 图像内容。

本图标于 2026-09-03 使用内置 imagegen 工具生成，随后用 Pillow 缩放并转换为 ICO 与 RGBA；没有使用雷神官方标识。生成源图为 1254 × 1254 透明 PNG，源图保留于设计交付文件，仓库保存发行所需资源。更换设计时，应从同一张源图以 Lanczos 分别缩放到所需尺寸，使用 `RGBA` 模式保存 PNG 和原始字节，并生成上述所有 ICO 尺寸，一起提交四个资源文件。单独替换 PNG 不会改变已经内嵌的程序图标。

## 生成提示词

```text
Use case: logo-brand. Create one production-ready Windows application icon for Leigod Guard, an independent utility that protects paid gaming time by pausing unused accelerator time. Asset: square 1024x1024 PNG app icon, centered standalone mark, not an icon sheet or mockup. Design a bold, simple shield whose inner negative space forms two clear pause bars, with a restrained short clock arc integrated into the shield silhouette. Premium and calm gaming utility identity. Deep midnight navy rounded-square tile, clean electric cyan/turquoise shield, luminous ivory pause bars, subtle depth but mostly crisp flat geometric shapes. Large simple silhouette readable at 16px and 32px; generous separation of shapes, thick strokes, no fine detail. The tile fills about 88 percent of the canvas with consistent margins and clean rounded corners; truly transparent outside the tile. No words, letters, numbers, watermark, scenery, game controller, official Leigod logo, or additional badges. Front-facing orthographic icon with balanced proportions. Deliver exactly one polished icon.
```
