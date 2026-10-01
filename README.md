# BluePandaChange

BluePandaChange converts Litematica `.litematic` and Sponge `.schem` blueprints into compressed Minecraft Structure `.nbt` files. It provides a Chinese desktop window and a command-line interface backed by the same Rust conversion library.

The project includes a batch converter, a desktop workspace with local 3D previews, and support for Litematica 6.1/7.1 and Sponge schematic V1/V2/V3. It is written in Rust and distributed under the MIT License. The bundled Noto Sans SC fonts are separately licensed under the SIL Open Font License 1.1; see `crates/gui/assets/OFL.txt`.

The converter reads Litematica `Version=6, SubVersion=1` and `Version=7, SubVersion=1`, plus Sponge schematic versions 1, 2, and 3. Sponge V1/V2 use a local `Palette` and `BlockData`; Sponge V3 uses the nested `Schematic` compound, with block palette/data under `Blocks`. It does not inspect game installations, consult a block registry, or perform game-version migrations. A source `MinecraftDataVersion` or `DataVersion` is copied to the output `DataVersion` when present; it is not rewritten for a target game version.

## Build

Install the Rust stable toolchain, then run `cargo build --workspace --release`. The CLI is written to `target/release/bluepanda-change`; the desktop app binary is `target/release/bluepanda-gui` (on Windows, `bluepanda-gui.exe`). `scripts/build-macos.sh` also packages `dist/BluePandaChange.app`; `scripts/build-windows.ps1` provides the Windows build command. GitHub Actions builds and tests on macOS and Windows.

## Command line

```text
bluepanda-change <INPUT>... --output-dir <DIR> [--overwrite]
```

For example:

```sh
bluepanda-change house.litematic tower.litematic build-v1.schem build-v2.schem build-v3.schem --output-dir ./converted
```

This converts Litematica and Sponge files in one batch. Outputs use the input filename with `.nbt` appended, for example `build-v2.schem.nbt`. Existing outputs are kept unless `--overwrite` is supplied. The process returns `0` when all files convert, `1` when one or more files fail, and `2` for invalid command-line arguments. A failed file does not prevent later files from being processed.

## Desktop window

Run `cargo run -p bluepanda-gui` or launch the release binary. Add `.litematic` and `.schem` files, choose an output directory, set whether existing outputs may be replaced, and start conversion. The queue cannot be changed while conversion is running. Each file shows its own progress and result.

The conversion workspace keeps the file table and selected-file details side by side. Select rows independently from the checkboxes used for removal, and use a row's context menu to preview a blueprint or open its location. During a batch, the footer tracks terminal files against the batch total and shows an estimated remaining time when it can be estimated; the selected file's stage progress and elapsed time appear in its details. A failed file does not stop later files; “继续处理其余文件” continues only files still waiting and does not retry failed files. Result details include output size, warnings, processing logs, and actions to copy the output path or show it in the file manager.

### 本地三维预览

桌面窗口可预览 Litematica 6.1/7.1 `.litematic`、Sponge V1/V2/V3 `.schem` 和 Minecraft Java Structure `.nbt`。预览完全在本机进行，不读取 Minecraft 安装目录。`.nbt` 文件仅用于预览；只有 `.litematic` 与 `.schem` 可转换。

添加文件或拖入文件后，点击队列中的文件即可加载预览。预览包含方块颜色和楼梯、台阶等简化几何；拖动鼠标旋转视角，平移并滚轮缩放，点击“重置视角”恢复取景。可选择全部方块或逐层查看，Y=0 是可单独选择的第 0 层。预览面板显示尺寸、区域、非空气方块数、实体数量和解析警告；实体本身不绘制。转换完成后可点击“预览转换输出”查看生成的 Structure 文件。

也可启动时直接载入一个或多个文件：`cargo run -p bluepanda-gui -- --preview-file '/path/to/file.litematic'`。macOS Release 应用可用 `open -n '/path/to/BluePandaChange.app' --args --preview-file '/path/to/file.litematic'` 启动；多文件时重复 `--preview-file PATH`。

预览有独立资源上限：解压后 NBT 最多 256 MiB、源蓝图最多 16,777,216 个单元、最多 1,000,000 个可见方块、最多 500,000 个暴露网格面。超过限制时只会停止预览，不会改变转换处理。多区域中重叠方块按区域名称顺序确定显示记录，并在警告中报告重叠。

The bundled Noto Sans SC font is licensed under SIL Open Font License 1.1. Its license is in `crates/gui/assets/OFL.txt`.

## Conversion behavior

The converter preserves block states, block entities, and entities where the standard Structure format represents them. Unknown block namespaces and typed NBT payload fields are retained; block state syntax is validated, but names are not checked against a block registry. Sponge V1 permits a missing `DataVersion`; V2 and V3 require it. V1/V2 `PaletteMax` is optional; when present, it must be a positive integer and every local palette ID must be below it. V1/V2 block and entity payload fields are read at their entry level, while V3 payloads nested in `Data` are merged into the output entry.

Regions are normalized into one enclosing bounding box. Conflicting overlapping regions fail with a location; ordinary air records are omitted. Litematica metadata and pending ticks are reported as omitted data. Sponge metadata, recognized biome fields in V2/V3, non-zero `Offset`, and V1 `TileEntities.ContentVersion` are reported as omitted data when present. V2/V3 payload fields with the same name are retained. A non-zero Sponge `Offset` is not applied to block or entity coordinates. Sponge V3 schematics without a `Blocks` compound are accepted as entity-only inputs and produce an air-only block volume; this no-`Blocks` path has a safety limit of 16,777,216 cells. Outputs are staged in the destination directory, checked by reading them back, and committed only after verification.

Run workspace checks with `cargo test --workspace` and `cargo build --workspace --release`.

For a local preview-versus-conversion round-trip check, run `cargo run -p bluepanda-core --example preview_compare -- INPUT OUTPUT.nbt`. The example loads the preview, converts to the requested output, reopens the Structure NBT, and compares dimensions, visible non-air block states and positions, and entity counts. Choose an output path that does not already exist.
