# 预览功能验证记录

本记录包含合成 NBT fixture、项目 core 测试，以及两份本机样例的预览和转换回读结果。真实样例仅从 Downloads 读取；转换文件写入 `/tmp/BluePandaChange-validation/`。

## 功能与限制

桌面预览支持 Litematica 6.1/7.1 `.litematic`、Sponge V1/V2/V3 `.schem` 和 Minecraft Java Structure `.nbt`。Structure NBT 可为 raw 或 gzip；`size`、块 `pos` 和实体 `blockPos` 使用三元素 `List<Int>`。`entities` 和 `DataVersion` 可缺省。存在 `palettes` 时首版使用第一个 palette 并提示；重复块坐标会拒绝预览。

预览 DTO 由 core 解析成共享方块状态表、相对文档原点的稀疏块、区域信息、实体数和警告。源区域按名称排序；重叠处显示首个区域的非空气块并在警告中报告。普通空气、洞穴空气和虚空空气不进入稀疏绘制列表。颜色和楼梯、台阶等简化几何由桌面端生成，实体不绘制。

预览专用预算：解压 NBT 256 MiB、源单元 16,777,216、Structure 块记录/可见方块 1,000,000、网格暴露面 500,000。取消可在读入和分批解码时响应；fastnbt 反序列化无法中途打断，会在反序列化前后检查。mesh 阶段也支持取消。以上预算只影响预览，转换使用既有转换流程。

## 自动化验证

`cargo test -p bluepanda-core` 覆盖原有解析与转换行为，以及新增预览行为：raw/gzip Structure、缺少 entities/DataVersion、List<Int>坐标、palette variants 首组提示、重复坐标、越界坐标、非有限实体位置、NBT尾随字节、取消、source-cell 限制、Litematica多区域冲突排序、三轴负尺寸和多区域外包尺寸归一化，以及三种蓝图来源统一输出。

真实输入与转换结果可用参数化示例复查：

```sh
cargo run -p bluepanda-core --example preview_compare -- \
  '/Users/yuanshi/Downloads/古风小院.litematic' \
  '/tmp/BluePandaChange-validation/古风小院.litematic.preview.nbt'

cargo run -p bluepanda-core --example preview_compare -- \
  '/Users/yuanshi/Downloads/Datacenter.schem' \
  '/tmp/BluePandaChange-validation/Datacenter.schem.preview.nbt'
```

示例会读取源文件、计时预览、转换到指定新路径、重新读取 Structure NBT，并比较尺寸、非空气方块状态与相对坐标映射、可见方块数和实体数。它不修改输入文件；再次运行前需移除或改名已有输出。

## 本机样例结果

| 输入 | 预览尺寸 | 源单元 | 可见非空气块 | 实体 | 预览耗时 | 转换耗时 | Structure 回读对比 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| `古风小院.litematic` | 125 × 59 × 115 | 848,125 | 85,318 | 18 | 345 ms | 1,789 ms | 尺寸、方块位置/状态、块数和实体数相同 |
| `Datacenter.schem` | 73 × 36 × 73 | 191,844 | 24,421 | 0 | 91 ms | 490 ms | 尺寸、方块位置/状态、块数和实体数相同 |

生成文件位于：

- `/tmp/BluePandaChange-validation/古风小院.litematic.preview.nbt`
- `/tmp/BluePandaChange-validation/Datacenter.schem.preview.nbt`

两份样例的源单元数和可见块数都低于预览预算；完整模型的预算边界仍需以更大文件实测。

GUI 对源文件及对应转换输出均完成网格预算实测，结果一致：

| 输入 | 网格视图 | 方块数 | 暴露面 | 构建耗时 |
| --- | --- | ---: | ---: | ---: |
| `古风小院.litematic` 源 / 输出 NBT | 全部 | 85,318 | 129,988 | 457 / 455 ms |
| 同上 | Y=0 | 14,375 | 1,048 | 72 / 71 ms |
| 同上 | 最高非空层 Y=31 | 1 | 6 | <1 ms |
| `Datacenter.schem` 源 / 输出 NBT | 全部 | 24,421 | 20,334 | 118 / 194 ms |
| 同上 | Y=0 | 190 | 488 | 约1 ms |
| 同上 | 最高非空层 Y=35 | 5 | 6 | <1 ms |

小院尺寸顶层 Y=58 是空层。两份样例所有网格均未超过 500,000 暴露面预算；源蓝图和输出 NBT 的尺寸、可见方块映射、实体计数一致。

Release arm64 应用分别用 `open -n '.../BluePandaChange.app' --args --preview-file '/绝对路径/文件'` 启动源文件与转换输出 NBT。两组文件均通过正常预览 worker、CPU 网格构建和 Glow GPU 绘制路径，真实窗口中可见方块画面；GUI 截图记录为：

- `/tmp/BluePandaChange-validation/BluePandaChange-ancient-source.png`
- `/tmp/BluePandaChange-validation/BluePandaChange-ancient-output.png`
- `/tmp/BluePandaChange-validation/BluePandaChange-datacenter-source.png`
- `/tmp/BluePandaChange-validation/BluePandaChange-datacenter-output.png`
- `/tmp/BluePandaChange-validation/BluePandaChange-datacenter-output-layer0-attempt.png`（Y=0）
- `/tmp/BluePandaChange-validation/BluePandaChange-datacenter-output-layer9.png`（Y=9）

## 最终检查

已完成以下检查：

- `cargo test --workspace`：通过，共 74 项：core unit 30、conversion 6、Sponge 12、transform 14、GUI 12；doc tests 通过。
- `cargo fmt --all -- --check`：通过。
- `git diff --check`：通过。
- `./scripts/build-macos.sh`：成功生成 arm64 Release 应用 `dist/BluePandaChange.app`；`plutil -lint` 通过，Mach-O 架构为 arm64，应用已启动并确认显示主窗口。

新增 `--preview-file PATH` 启动参数单测通过；可重复传入多个文件。真实窗口中已确认四个样例（两份源文件及其转换 NBT）都能完成 GPU 绘制。另用 CGEvent 实际点击 Datacenter 的 Y=0 层（190 块、488 面），拖动预览画布改变视角，并用滑条切换至 Y=9。文件选择器、拖放和顶层控件尚未实际操作验收。Windows 构建和运行尚未实测，CI 也未触发。

## 批量转换工作台人工验收（2026-10-01）

以下结果来自本机桌面应用的真实运行，不是代码审查结论：

- `cargo build -p bluepanda-gui`：成功。
- `cargo check --workspace`：成功；输出包含 dead-code warnings。
- GUI 批量加入真实样例 `/Users/yuanshi/Downloads/古风小院.litematic` 与 `/Users/yuanshi/Downloads/Datacenter.schem` 后转换：2/2 成功，生成文件通过回读校验，界面显示批次完成和 100%。
- 在两份真实文件之间加入系统临时目录中的 `malformed_fixture.schem`（坏 gzip）：该项显示失败，批次继续处理后续文件，另外两项仍成功；结果为 2 成功、1 失败，3/3 已处理。
- 真实转换期间截取了阶段进度视图，包含当前文件的 1% 阶段进度；预览视图实际加载 `Datacenter.schem`（24,421 个可见方块）。

上述运行对应的界面截图保存在 `design/implementation-captures/`：`waiting.png`、`preview.png`、`running.png`、`complete.png`、`failure.png`、`workspace.png`。这些截图记录本次真实样例运行；之后工作区又有视觉调整，需以最终构建重新截图核对最新布局。

本次尚未手动验收输出已存在时的覆盖保护、单文件转换与继续处理按钮的逐项交互、系统文件管理器打开位置、复制输出路径、拖放文件和主题切换。窗口尺寸、表格点击、文件选择器及系统级拖放也未纳入上述真实运行结果。以上未操作项不代表功能通过；本次未新增或运行自动测试。

## 最终 GUI 构建与截图索引（2026-10-01）

本节记录最终文件名列左对齐与暗色工具栏文字调整后的构建及截图；上方“本机样例结果”和“批量转换工作台人工验收”保留为此前记录，不作为本轮新增测试结果。

- `cargo build -p bluepanda-gui`：通过。
- `cargo check --workspace`：通过。两条命令有未使用私有方法/字段的 `dead_code` 警告，无编译错误；本轮没有运行自动测试。
- `bash scripts/build-macos.sh`：成功生成 `dist/BluePandaChange.app` 和 `target/release/bluepanda-change`。`plutil -lint dist/BluePandaChange.app/Contents/Info.plist` 通过；GUI Mach-O 为 arm64。
- 最终 release CLI 对单个真实 `Datacenter.schem` 转换成功，只生成 `/tmp/bluepanda-final-qa/single/Datacenter.schem.nbt`（64,465 bytes）。对同一目标目录再次运行且不传 `--overwrite` 返回退出码 1 和 `output already exists`；前后 SHA-256 均为 `d9bfae702f678cebcad0c56de8c1c5e40f0199f2cde6d43efb40bb057abdf362`，证明原输出未被覆盖。这是 CLI/core 行为验收，不代表 GUI 按钮点击验收。

最终截图由 GUI 的 framebuffer 截图事件采集，队列使用本机真实文件 `/Users/yuanshi/Downloads/古风小院.litematic` 与 `/Users/yuanshi/Downloads/Datacenter.schem`；截图没有伪造队列状态：

| 目标状态 | 截图 | 真实内容 |
| --- | --- | --- |
| 参考图队列工作台 | `design/implementation-captures/workspace.png` | 浅色工作台，两个真实文件，选中 Datacenter 行 |
| 参考图三维预览 | `design/implementation-captures/preview.png` | 最终构建加载真实古风小院，侧栏显示实际尺寸、方块数、实体与 Region 元数据 |
| 待处理 | `design/implementation-captures/waiting.png` | 两个真实文件处于 Waiting，未选中行 |
| 正在处理 | `design/implementation-captures/running.png` | 实际转换事件达到 1% 阶段进度并显示当前文件与阶段 |
| 全部成功 | `design/implementation-captures/complete.png` | 两个真实输入成功，批次 2/2 |
| 含失败并自动继续 | `design/implementation-captures/failure.png` | 临时目录坏 gzip schematic 失败；其余两份真实文件继续成功，批次 3/3 |

额外截图：`design/implementation-captures/dark-small.png` 是深色 900×600 工作台；`preview-top.png`、`preview-orthographic.png`、`preview-through.png`、`preview-layer-zero.png`、`preview-wireframe.png`、`preview-translucent.png` 和 `preview-orthographic-through-hidden.png` 记录预览视角/模式/切片场景。除最终默认 `preview.png` 外，这些额外预览图来自此前场景采集；最终源码的截图生成入口保留。两 Region 重叠 fixture `/tmp/bluepanda-overlap-two-regions.litematic` 已在此前预览场景验证：隐藏 A_Stone 后同坐标 B_Glass 恢复显示。Running 场景为等待真实进度事件而对事件送达稍作节流，截图中的阶段与计数来自实际转换，但该场景用时不是性能基准。

本轮没有完成 GUI 鼠标端到端验收。曾在已运行的旧 GUI 实例中打开行菜单，菜单展示“预览建筑/单个转换/打开文件位置”；该旧窗口截图另存为 `design/implementation-captures/workspace-menu-legacy.png`，不属于最终截图索引或最终布局证据。GUI 应用包通过 plist 与架构检查，但本轮没有确认新打包应用窗口的成功启动。覆盖保护和单文件转换已通过上面的 CLI 实测；勾选与高亮独立、移除优先、行菜单单文件转换、转换中重复启动禁用、继续按钮不重启失败项、Finder 定位、复制路径、拖放和手动主题切换均未在本轮实际点击验证。预览按钮、相机操作、切片快速切换与取消也未在本轮逐项手动验证。
