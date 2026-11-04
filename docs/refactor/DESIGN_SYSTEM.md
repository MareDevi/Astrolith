# Neutral 原生设计系统

状态：M0 提取与设计，M1 已实现中性 light/dark tokens、OKLCH 转换、共享控件映射与尺度快照。目标仍是尽量还原 mock 的视觉，而非延续旧 UI。基准来源为 `astrolith-web-mock/src/index.css` 和 `src/components/`。视觉参考必须经过原生交互和可访问性约束校正。

## 1. 颜色提取

下表保留原始 OKLCH 值。斜杠后为 alpha；未注明则为 1。中性色 chroma/hue 均为 0。

| CSS token | Light | Dark | 原生语义 |
| --- | --- | --- | --- |
| background | `oklch(1 0 0)` | `oklch(0.145 0 0)` | 正文/主工作面 |
| foreground | `oklch(0.145 0 0)` | `oklch(0.985 0 0)` | 主要文字 |
| card / popover | `oklch(1 0 0)` | `oklch(0.205 0 0)` | 分组面 / 悬浮面 |
| card-foreground / popover-foreground | `oklch(0.145 0 0)` | `oklch(0.985 0 0)` | 对应表面文字 |
| primary | `oklch(0.205 0 0)` | `oklch(0.922 0 0)` | 主要动作，不表示品牌色 |
| primary-foreground | `oklch(0.985 0 0)` | `oklch(0.205 0 0)` | primary 上文字 |
| secondary / muted / accent | `oklch(0.97 0 0)` | `oklch(0.269 0 0)` | 中性选中、hover、次级分组 |
| secondary-foreground / accent-foreground | `oklch(0.205 0 0)` | `oklch(0.985 0 0)` | 对应表面文字 |
| muted-foreground | `oklch(0.556 0 0)` | `oklch(0.708 0 0)` | 辅助说明，不用于必要正文 |
| border / input | `oklch(0.922 0 0)` | `oklch(1 0 0 / 10%)` / `oklch(1 0 0 / 15%)` | 结构细线 / 控件底色基值 |
| ring | `oklch(0.708 0 0)` → 原生 `0.65` | `oklch(0.556 0 0)` | 键盘焦点；light 下调以满足 3:1 |
| destructive | `oklch(0.577 0.245 27.325)` | `oklch(0.704 0.191 22.216)` | 真实错误/破坏性动作 |
| sidebar | `oklch(0.985 0 0)` | `oklch(0.205 0 0)` | 导航背景 |
| sidebar-foreground | `oklch(0.145 0 0)` | `oklch(0.985 0 0)` | 导航文字 |
| sidebar-primary | `oklch(0.205 0 0)` | **`oklch(0.488 0.243 264.376)`** | 深色原型遗留蓝色，目标替换为 dark primary |
| sidebar-primary-foreground | `oklch(0.985 0 0)` | `oklch(0.985 0 0)` | 目标随中性 primary 改为对应 primary-foreground |
| sidebar-accent / sidebar-accent-foreground | `oklch(0.97 0 0)` / `oklch(0.205 0 0)` | `oklch(0.269 0 0)` / `oklch(0.985 0 0)` | 导航 hover/选中 |
| sidebar-border / sidebar-ring | `oklch(0.922 0 0)` / `oklch(0.708 0 0)` | `oklch(1 0 0 / 10%)` / `oklch(0.556 0 0)` | 导航边界/焦点 |
| chart-1…5 | `.87 / .556 / .439 / .371 / .269`，均 `0 0` | 同 Light | 未用于本次工作区，无需新增图表模块 |

原型 button 的 secondary hover 使用 secondary 与 foreground 5% 混合；input 使用 input/50，focus ring/30；错误使用 destructive/20，disabled opacity/50。这些是提取事实，最终要测合成背景对比度，不能假设低 alpha 自动满足可访问性。

## 2. 原生 Token 所有权与映射

扩展现有 `crates/astrolith-gpui/src/theme/{tokens,palettes,typography,mod}.rs`，不新建并行主题系统。继续由 `theme::apply` 通过 `Theme::update` 同步 Component/Base，System appearance 沿用窗口监听。

| 产品角色 | 现有 ThemeTokens / Kit 消费位置 | 迁移要求 |
| --- | --- | --- |
| canvas / text | `background` / `foreground` | 正文与 Editor 背景一致 |
| navigation / chrome | `surface`，Kit sidebar/title_bar | 层级优先用表面明度，边界只画一次 |
| group / control | `elevated` / `input` | 控件和分组可区分，避免层层卡片 |
| overlay | `popover` / `popover_foreground` | Inspector、Select、DatePicker、menu 共用语义 |
| selected / hover / pressed | `selection` / `hover` / `active` | 中性填充，不能只靠 hover 表示选中 |
| primary / ring | 现有 `accent` 当前同时映射 primary/ring | M1 拆清 action、focus 角色，不能把 CSS accent 当 Kit primary |
| border / secondary text | `border` / `border_variant` / `muted` | CSS muted 是背景，现有 muted 是文字；禁止同名直接复制 |
| status | `danger` / `warning` / `success` / `info` | 仅表示真实状态，不为导航上色 |

新旧用户统一迁移到 mock Neutral light/dark；淘汰 Manuscript、Ink 和旧 palette 入口，不提供旧 UI 兼容外观。代码高亮、Copyedit 自有语义颜色可保留，通用选中和主动作不使用蓝色。

OKLCH 不是 GPUI 组件直接接受的 CSS。M1 在主题边界完成受验证的色彩转换，透明色保留 alpha；不在 render 中做转换。所有控件从 token/`cx.theme()` 取色，不散落 raw hex。暂不为颜色转换引入大依赖。

补齐应用持有的 typography、spacing、radius、surface、interaction、elevation scales。Kit 对自定义 spacing/elevation 的存储能力必须以 0.8.1 源码为准，不假设 Theme round-trip 能保存任意自定义字段。

## 3. 排版与节奏

以下 px 为原型在 16 px CSS root 下的参考换算，不是原生组件内写死的值。

| 层级/角色 | 原型来源 | 目标约束 |
| --- | --- | --- |
| UI 字体 | `index.css` Inter Variable | 以 Inter Variable 及 mock 字体度量为默认目标；评估授权与字体打包，CJK/缺字体采用度量接近的 fallback |
| 正文 | `writing-view.tsx` text-base + leading-8 | 以 mock 的 sans 正文和 16/32 比例为目标，移除旧 serif 默认；字号/行高可配，不靠缩放整个 UI 改正文 |
| 文档标题 | text-4xl，sm:text-5xl，semibold/tight | 参考 36/40 与 48/48；真实内容标题，不插入演示副标题 |
| 二级标题 | text-2xl，semibold | 参考 24/32；不能破坏原有 Markdown 装饰和源码映射 |
| 元数据/表单 label | text-xs，部分 11 px | 参考 12/16；缩放后仍可读，必要信息不靠极小灰字 |
| 正文宽度 | max-w-3xl | 以 mock 外框 768 与内边距关系为目标；当前 720 是待迁移基线，不作为保留设计 |
| 正文留白 | px-6/8，py-16/24 | 24/32 水平、64/96 垂直；小窗缩减留白，不挤压编辑内容 |

提取 spacing 集合为 2、4、6、8、12、16、20、24、32、48、64、96。原生核心采用 2/4/8/12/16/24/32 的命名级别；6/20 为控件局部补充，48/64/96 为写作空间级别。保存在 token 定义，布局通过 rem helpers/semantic size 消费。

窗口 UI base size 当前 14，editor base 18，editor_width 720。必须明确 UI zoom、正文大小、系统显示缩放三个轴，不把 web 的 16 px root 当原生 rem。新默认比例按 mock 校准，旧外观字段不得压过新默认；用户后续显式调整仍需持久化。缓存排版测量需随字号、宽度、rem 失效。

## 4. 圆角、边界与表面

CSS `--radius = 0.625rem`；原型导出：sm .375rem、md .5rem、lg .625rem、xl .875rem、2xl 1.125rem、3xl 1.375rem、4xl 1.625rem。

原型按钮 `rounded-4xl`、输入 `rounded-3xl`、模式 tabs `rounded-full`，Inspector `rounded-xl`。原生控件应尽量复原这些轮廓，不能因沿用旧 UI 半径而擅自改成方形或另一套组件风格。M1 以 mock 比例建立 control-small/control/surface/overlay tiers，按嵌套 inset 选择内外半径；按钮、输入与模式切换器分别对照原型验证。

表面层级：window canvas（平）→ navigation/group（靠背景区分）→ controls（中性底色）→ floating popover/menu（共享轻阴影与细线）→ decision dialog（更高遮挡层）。只在真实浮层使用 elevation。原型 shadow-sm/xl 与 z-40/50 仅表达关系，不直接转换为 GPUI 数字层级。

原型 preferences backdrop 有 blur；本次明确不迁移玻璃效果。原型仿 macOS 三点与外框留白也不是原生窗口装饰规范；保留实际平台 title bar drag region。

## 5. 布局与响应式提取

- App：参考上限 1580，示意高度 min(930, viewport−48)，toolbar 56、document rail 48、status 32；原生窗口自身占满可用区域，不嵌套一层网页外框。
- Sidebar：参考 224，纵向独立滚动；当前原生 preferred width 240、clamp 160…400 属于旧基线；新默认按 mock 校准，后续显式 splitter 偏好才恢复。
- Writing：正文居中，不显示底部演示提示 pill；保存/错误可在必要时显现，不能被 auto-hide 永久藏住。
- Build：参考 tabs 40、工具区 160、工具 header 36；真实输出才显示。Build 默认布局不强制打开 Terminal/Problems。
- Embedded Preview：参考源码占 42%，md 以下隐藏源码；原生先定义两边最小工作宽度，不直接按浏览器 breakpoint 隐藏当前文档。
- Inspector：右侧内缩 16，参考宽 min(416, viewport−32)，header/body/footer；body 是唯一表单滚动区域。原生最大高度为可用内容区高度减上下安全距，窗口缩小后重新约束位置。
- 当前窗口 minimum 1000×700，`shell.rs::restored_bounds` 同样执行此下限。本轮不承诺手机宽度；验收覆盖最小窗口、高 UI zoom、长字段与长路径。

## 6. 真实交互与组件契约

| 场景 | 原生组件/所有者 | 必需行为 |
| --- | --- | --- |
| 模式切换 | Shell 的 Actions + Kit tabs/segmented composition | persistent selected，键盘可达；同一文档不换示例文件 |
| Toolbar | 现有 Button/DropdownMenu/Tooltip | ghost 常规动作；图标具名；危险/加载/禁用有可见原因 |
| 文件导航 | 现有 ProjectWorkspace/List | domain ID、键盘选择、filter/sort 保留；不是演示树 |
| Inspector | retained FrontmatterForm + controlled Popover | 默认关闭、outside/Esc 关闭、返回触发焦点、编辑区几何不变 |
| 表单 | 现有 Input/NumberInput/Switch/DatePicker/Select 与 row entities | schema path 身份、required/default/description/error，长 label 可换行 |
| 设置 | 现有 PreferencesSheet / draft model | Apply/Cancel 语义保留，新增页不重复 resolver |
| Preview 状态 | retained PreviewPanel | stopped/starting/ready/stopping/failed，真实 URL/错误，不画假站点 |

Inspector 打开时转移到首个合理控件或面板入口。Esc 优先关闭内层 Select/DatePicker，再关闭 Inspector；不能抢 IME composition 取消。outside 点击不得穿透触发正文格式或文件删除。关闭恢复 trigger；trigger 已失效时回到活动 editor。切换文档需明确刷新/关闭当前表单，拒绝过期字段事件。

Popover 不替代为 absolute div；必须验证 z-order、hit testing、窗口 clamp、焦点与嵌套弹出。当前 Form 的滚动所有权和浮层不能叠加成两个竞争滚动区。表单超长时滚动，header/close 始终可达。

## 7. 状态与可访问性

Rest、Hover、Pressed、Open、Selected、Focus visible、Disabled、Loading、Error 使用共同 token 策略。Selected fill/foreground 与 hover 区分；焦点 ring 不被祖先 clip。错误同时显示文字与恢复动作，状态不只靠颜色。

普通文本目标对比度 ≥4.5:1，较大文字和必要控件边界/焦点目标 ≥3:1；需对最终 alpha 合成与真实字体测量。Focus 弱化区域仍可选择/编辑；不能用整层低 opacity 让光标、选区和错误一起消失。

原型 Sidebar 200 ms、Dialog 100 ms 与 opacity transition 只作动效上界参考。优先即时模式切换，尊重 reduced motion；不添加持续装饰动画。

## 8. M1 实施记录

- `theme/tokens.rs` 用产品语义命名角色；CSS `muted`（背景）映射为 `control`/`hover`，`muted-foreground` 映射为 `muted`，避免同名误解。`accent` 不再承担 primary 与 ring 两个角色，拆为 `primary`、`primary_foreground`、`focus`。
- `theme/palettes.rs` 只保留中性 light/dark 两组值，并在主题边界把 CSS OKLCH 转为 HSLA；alpha 保留，透明细线由渲染器合成。测试覆盖白/黑/红参考值、alpha、边界合成与文本对比度。
- Light focus ring 从 mock 的 `0.708` 调整为 `0.65`：mock 原值在白色画布上约 2.6:1，低于必要控件/焦点 3:1 目标；颜色仍为中性灰。Dark ring 保持 `0.556`（约 4.2:1）。
- `theme/metrics.rs` 固化 spacing 2…96、type 12/14/16/32/24/36/48/13、radius small/control/surface/overlay/pill 与 overlay elevation。Kit 0.8.1 只持久化 `radius` 与 `radius_lg`，分别投影为 10 px 与 14 px。
- `theme/typography.rs` 默认使用 Inter（`Inter Variable`/`Inter`），缺失时回退平台 UI 字体；正文不再默认 serif。CJK 由 GPUI 的脚本回退覆盖。
- Kit `Theme::change` + `Theme::update` 同步 Component/Base；sidebar、title bar、status bar、tabs、list、table、primary/secondary、popover、switch、slider、overlay 均映射到中性角色。
- `light_theme` / `dark_theme` 配置字段、palette 枚举和设置页入口已删除。升级加载时把原 `global.json` 完整备份为 `global.json.pre-neutral.json`，再移除两个旧键；未知顶层字段、主题偏好与其它设置保留。
- 导航 `WorkspacePanel` 使用 `surface`，正文面板使用 `background`，建立与 mock 一致的 sidebar/canvas 层级。
- 已用真实原生窗口（Linux Wayland）截图检查 light/dark：画布、sidebar、title bar、按钮与空状态均为中性，无蓝色通用强调，focus ring 为中性灰。

仍未迁移（不阻塞 M1，但 A26 未完成）：

- mock 的模式 tabs、Build 标签条与预览浮层属于 M2；本轮未添加。
- Kit 的 light input 背景固定取 `background`，因此无法在不改上游的情况下复现 mock 的 `input/50` 浅灰填充；dark 输入已使用半透明 input 基值。
- Kit 按钮/输入统一使用 `radius`，尚未逐控件复现 mock 的 pill（`rounded-4xl`/`rounded-3xl`）轮廓；模式切换器为 M2 范围。
- 正文 measure 仍沿用用户配置默认 720，M3 再按 mock 768 与可配置排版收口；spacing 尺度尚未替换全部旧布局硬编码。
- 代码高亮与 Copyedit 自有语义色未改。

## 9. 实施审查

M1 首先校对 Theme adapter；随后逐区域迁移 chrome、表单、菜单与选中态。M7 在真实内容与真实窗口下记录 dark/light、最小/宽窗、UI zoom 和缩放截图及边界测量。M0 只有源码对照，不宣称完成 pixel parity。

## 10. M3 实施记录

- 正文排版不再随 pane 宽度缩放。`ProseStyle` 直接采用用户持久化的字体、字号、行高倍数与 measure，非法持久值在渲染边界 clamp；`set_prose_style` 只改绘制属性，不写编辑器文本、选区或历史。
- 新默认按 mock：16 px 字号、2.0 行高（32 px）、768 px measure、`PROSE_INSET` 32、`PROSE_TOP` 96；正文默认 sans（Inter，缺失回退平台 UI 字体），旧 serif 默认不再存在。
- 文档字体与界面字体分开配置：`prose_font_family` 为空时使用 mock sans；缺字体时回退同一 sans，校验层拒绝未安装家族。CJK 仍依赖 GPUI 的脚本回退，未捆绑 CJK 字体。
- Focus/Typewriter 作为全局偏好持久化；启动、切换命令和 Apply 都走同一 `WritingModes` 状态。sentence focus 保持既有语义，未新增 paragraph focus。
- 旧配置若没有 `editor_line_height`，一次性备份为 `global.json.pre-writing.json`，把未改动的旧默认 18/720 升为 16/768 并写入新字段；用户显式改过的字号或宽度保留。
- 原生 Wayland dark/System 截图核对了默认排版与持久化后的 22 px/2.4/900 + Focus；150%/200% 缩放与 IME 组合仍未验收。

## 11. M4b 实施记录

- 永久 right Dock 及其 `inspector_visible`/`inspector_width` 布局字段已删除。旧配置中的未知字段由 serde 忽略，不会恢复旧右栏。
- `FrontmatterForm` 保留 entity、字段 rows 与 `FrontmatterEdit` subscription，但只渲染表单 body，是浮层内唯一滚动区域；浮层本身提供 “Frontmatter / Entry details · collection”、关闭按钮与边界。
- 浮层由 pinned controlled `Popover` 承载，锚定文档 rail 的 Details 按钮（TopRight，6 px 偏移），宽度 `min(416, viewport−32)`，高度 `viewport−144` 并设下限；Positioner 负责 window margin、翻转与 clamp。
- 默认关闭；按钮和 `Ctrl+2` 打开，outside/Esc/关闭按钮关闭。Escape 与焦点恢复由 pinned popover 拥有；关闭后回到正文编辑器。模式切换关闭它但不写入布局。
- Form 在 `active_frontmatter_key` 变化时从完整源重建；schema 刷新使用最新 buffer 文本，因此已提交的字段编辑不会回退。无 frontmatter 的文档禁用按钮与命令。
- GPUI 测试覆盖：默认关闭、按钮打开、outside/close/Esc 关闭、正文 bounds 不变、Esc 后编辑器焦点恢复、模式切换关闭。原生 Wayland 以 `Ctrl+2` 打开真实字段并 Esc 关闭。
