# BluePandaChange

BluePandaChange converts Litematica `.litematic` and Sponge `.schem` blueprints into compressed Minecraft Structure `.nbt` files. It provides a Chinese desktop window and a command-line interface backed by the same Rust conversion library.

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

The bundled Noto Sans SC font is licensed under SIL Open Font License 1.1. Its license is in `crates/gui/assets/OFL.txt`.

## Conversion behavior

The converter preserves block states, block entities, and entities where the standard Structure format represents them. Unknown block namespaces and typed NBT payload fields are retained; block state syntax is validated, but names are not checked against a block registry. Sponge V1 permits a missing `DataVersion`; V2 and V3 require it. V1/V2 `PaletteMax` is optional; when present, it must be a positive integer and every local palette ID must be below it. V1/V2 block and entity payload fields are read at their entry level, while V3 payloads nested in `Data` are merged into the output entry.

Regions are normalized into one enclosing bounding box. Conflicting overlapping regions fail with a location; ordinary air records are omitted. Litematica metadata and pending ticks are reported as omitted data. Sponge metadata, recognized biome fields in V2/V3, non-zero `Offset`, and V1 `TileEntities.ContentVersion` are reported as omitted data when present. V2/V3 payload fields with the same name are retained. A non-zero Sponge `Offset` is not applied to block or entity coordinates. Sponge V3 schematics without a `Blocks` compound are accepted as entity-only inputs and produce an air-only block volume; this no-`Blocks` path has a safety limit of 16,777,216 cells. Outputs are staged in the destination directory, checked by reading them back, and committed only after verification.

Run workspace checks with `cargo test --workspace` and `cargo build --workspace --release`.
