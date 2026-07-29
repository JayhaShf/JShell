# Ashell 终端字体校准预览实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**目标：** 将内置字体从官方 Bold 回退到官方 Regular，并制作同时包含三栏并排、单窗口切换和完整校准视图的本地 HTML 预览。

**架构：** Rust 应用继续通过 GPUI `add_fonts` 内置单个 Noto Sans Mono CJK SC 字体文件，配置字体族名称保持不变。预览是一个无构建步骤的独立 HTML 文件，通过 `@font-face` 加载同一字体，并用 JavaScript 将终端文本拆成固定宽度单元格，以模拟正式终端的一列 ASCII、两列 CJK 网格。

**技术栈：** Rust、GPUI、HTML、CSS、原生 JavaScript、Noto Sans Mono CJK SC、浏览器截图验证。

## 全局约束

- 官方不存在 `NotoSansMonoCJKsc-Black.otf`，必须使用官方 `NotoSansMonoCJKsc-Regular.otf`，不得用 Bold 或重命名文件冒充 Black。
- 三套预设分别为紧凑 `16px / 8px / 20px`、均衡 `17px / 9px / 23px`、宽松 `18px / 10px / 27px`，顺序为“字号 / 单列宽度 / 行高”。
- 预览严格使用当前 Ashell 深色主题色，并保持紧凑的 Windows 风格框架。
- 正式 Rust 终端的字号、单元格宽度和行高暂不修改。
- 不提交、不暂存任何文件。

---

### 任务 1：回退到官方 Regular 字体

**文件：**
- 创建：`assets/fonts/NotoSansMonoCJKsc-Regular.otf`
- 删除：`assets/fonts/NotoSansMonoCJKsc-Bold.otf`
- 修改：`src/app/theme.rs`
- 修改：`README.md`
- 修改：`README.en.md`

**接口：**
- 输入：Noto CJK 官方文件 `Sans/Mono/NotoSansMonoCJKsc-Regular.otf`
- 输出：GPUI 加载字体族 `Noto Sans Mono CJK SC`；预览页可通过相对路径加载同一字体文件。

- [ ] **步骤 1：下载并核对官方 Regular OTF**

运行：

```powershell
Invoke-WebRequest `
  -Uri 'https://raw.githubusercontent.com/notofonts/noto-cjk/main/Sans/Mono/NotoSansMonoCJKsc-Regular.otf' `
  -OutFile 'assets/fonts/NotoSansMonoCJKsc-Regular.otf'
fonttools ttx -q -t name -o - assets/fonts/NotoSansMonoCJKsc-Regular.otf
```

预期：名称表包含字体族 `Noto Sans Mono CJK SC`、子族 `Regular` 和 PostScript 名称 `NotoSansMonoCJKsc-Regular`。

- [ ] **步骤 2：让应用只加载 Regular 字体**

将 `src/app/theme.rs` 中的字体资源改为：

```rust
let font = std::borrow::Cow::Borrowed(
    include_bytes!("../../assets/fonts/NotoSansMonoCJKsc-Regular.otf").as_slice(),
);
```

字体族参数继续使用：

```rust
set_theme_font_names(cx.global_mut::<Theme>(), "Noto Sans Mono CJK SC");
```

- [ ] **步骤 3：同步文档并清理 Bold**

将中英文 README 的字体文件名统一改为 `NotoSansMonoCJKsc-Regular.otf`，确认代码和文档不再引用 Bold 后删除 `assets/fonts/NotoSansMonoCJKsc-Bold.otf`。

- [ ] **步骤 4：执行字体引用检查**

运行：

```powershell
rg -n "NotoSansMonoCJKsc-(Bold|Regular)|Noto Sans Mono CJK SC" src README.md README.en.md
Get-ChildItem assets/fonts
```

预期：运行时引用和 README 只包含 Regular，字体目录只保留 `NotoSansMonoCJKsc-Regular.otf`。

### 任务 2：创建三种终端字体预览

**文件：**
- 创建：`designs/ashell-terminal-font-calibration.html`

**接口：**
- 输入：`../assets/fonts/NotoSansMonoCJKsc-Regular.otf`
- 输出：`PRESETS` 常量、`renderTerminal(container, metrics)`、`applyPreset(name)`、`updateCalibration()`；页面提供 `[data-preset]`、`#font-size`、`#cell-width`、`#line-height`、`#grid-toggle` 和 `#reset` 控件。

- [ ] **步骤 1：建立页面布局与当前主题变量**

页面使用以下核心结构，不增加外部框架：

```html
<main class="app-shell">
  <header class="titlebar">
    <strong>Ashell 终端字体校准</strong>
    <div class="window-controls" aria-label="窗口控件">
      <button type="button" title="最小化">−</button>
      <button type="button" title="最大化">□</button>
      <button type="button" title="关闭">×</button>
    </div>
  </header>
  <nav class="view-tabs" aria-label="预览方式">
    <button type="button" data-view="comparison" aria-pressed="true">三栏并排</button>
    <button type="button" data-view="single" aria-pressed="false">单窗口切换</button>
    <button type="button" data-view="calibration" aria-pressed="false">完整校准</button>
  </nav>
  <section id="comparison-view" data-view-panel="comparison">
    <div class="comparison-grid" id="comparison-grid"></div>
  </section>
  <section id="single-view" data-view-panel="single" hidden>
    <div class="preset-switcher" id="single-preset-switcher"></div>
    <div class="terminal-frame" id="single-terminal"></div>
  </section>
  <section id="calibration-view" data-view-panel="calibration" hidden>
    <aside class="calibration-controls">
      <input id="font-size" type="range" min="12" max="22" step="1">
      <input id="cell-width" type="range" min="6" max="12" step="0.25">
      <input id="line-height" type="range" min="16" max="32" step="1">
      <label><input id="grid-toggle" type="checkbox">显示单元格网格</label>
      <button id="reset" type="button">恢复当前预设</button>
    </aside>
    <div class="terminal-frame" id="calibration-terminal"></div>
  </section>
</main>
```

使用当前深色主题的主要颜色：`#0b0b0b` 背景、`#171717` 侧栏、`#202020` 面板、`#303030` 边线、`#f5f5f5` 正文、`#a5a5a5` 次级文字、`#a7d797` 绿色、`#d8ca77` 黄色和 `#d75050` 红色。

- [ ] **步骤 2：定义字体与三套精确参数**

```css
@font-face {
  font-family: "Ashell Noto Mono";
  src: url("../assets/fonts/NotoSansMonoCJKsc-Regular.otf") format("opentype");
  font-style: normal;
  font-weight: 400;
}
```

```javascript
const PRESETS = {
  compact: { label: "紧凑", fontSize: 16, cellWidth: 8, lineHeight: 20 },
  balanced: { label: "均衡", fontSize: 17, cellWidth: 9, lineHeight: 23 },
  relaxed: { label: "宽松", fontSize: 18, cellWidth: 10, lineHeight: 27 },
};
```

- [ ] **步骤 3：按真实终端网格渲染固定宽度字符**

实现 `isWideCharacter(character)`，覆盖 CJK Unified Ideographs、Hiragana、Katakana、Hangul 和全角标点范围。`renderTerminal` 为每个 Unicode 字符生成 `.cell`，空格同样占格：

```javascript
cell.style.width = `${metrics.cellWidth * (isWideCharacter(character) ? 2 : 1)}px`;
cell.style.height = `${metrics.lineHeight}px`;
cell.style.fontSize = `${metrics.fontSize}px`;
```

每一行使用独立 `.terminal-line`，样本文本包含：

```text
prod@shanghai:~$ printf "ABC abc 0123456789 !@#$%^&*()"
英文宽度 ABCDEFG / 数字 012345 / 中文网格 服务器状态正常
drwxr-xr-x  5 deploy deploy 4096 Jul 27 18:42 releases
/var/www/ashell/releases/2026-07-27/bin/ashell-worker
```

ANSI 风格颜色通过数据片段标记应用，不把整行字符串拼接为不受控的 HTML。

- [ ] **步骤 4：实现三栏、单窗口和校准视图**

三栏视图固定渲染所有 `PRESETS`。单窗口视图通过 `[data-preset]` 分段按钮调用 `applyPreset(name)`。校准视图用三个范围输入更新 CSS 自定义属性和数值标签：

```javascript
function updateCalibration() {
  const metrics = {
    fontSize: Number(fontSizeInput.value),
    cellWidth: Number(cellWidthInput.value),
    lineHeight: Number(lineHeightInput.value),
  };
  renderTerminal(calibrationTerminal, metrics);
}
```

网格开关只切换 `.show-grid` 类；重置按钮恢复当前预设，不改变当前视图。

- [ ] **步骤 5：实现稳定的响应式布局**

桌面端三栏使用：

```css
.comparison-grid {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
}
```

在 `max-width: 900px` 下改为单列；所有终端区域使用固定最小高度与 `overflow: auto`，控制栏允许换行，按钮和数值标签使用固定尺寸，避免内容变化导致布局抖动。

### 任务 3：验证 HTML 和 Rust 构建

**文件：**
- 验证：`designs/ashell-terminal-font-calibration.html`
- 验证：`assets/fonts/NotoSansMonoCJKsc-Regular.otf`
- 验证：`src/app/theme.rs`

**接口：**
- 输入：本地 HTML 文件和 release 可执行文件。
- 输出：桌面、移动截图与成功的格式检查、测试、release 构建。

- [ ] **步骤 1：检查 HTML 结构与字体加载**

在浏览器打开：

```text
file:///D:/文档/Ashell/ashell/designs/ashell-terminal-font-calibration.html
```

在页面中验证：`document.fonts.check('16px "Ashell Noto Mono"')` 返回 `true`；紧凑、均衡、宽松卡片的计算字号分别为 `16px`、`17px`、`18px`。

- [ ] **步骤 2：验证交互行为**

依次切换三种查看方式和三套预设，移动三个滑块，开关网格并点击重置。预期：完整预览实时变化，中文单元格宽度始终为 ASCII 的两倍，单窗口与当前预设同步，重置恢复精确参数。

- [ ] **步骤 3：检查桌面与移动布局**

在 `1440x1000` 和 `390x844` 视口截图。预期：桌面显示三栏，移动端纵向排列；标题、控件和终端文本不重叠、不被按钮遮挡，最长单词可滚动查看。

- [ ] **步骤 4：执行 Rust 验证**

运行：

```powershell
cargo fmt
cargo fmt --check
cargo test
Get-Process ashell -ErrorAction SilentlyContinue | Stop-Process -Force
cargo build --release
```

预期：格式检查退出码为 0，全部测试通过，release 构建退出码为 0。

- [ ] **步骤 5：启动最新 release 并检查工作区状态**

运行：

```powershell
Start-Process target/release/ashell.exe
git diff --check
git status --short
```

预期：Ashell 使用 Regular 字体正常启动；差异检查没有错误；所有改动保持未提交、未暂存。
