# BluePandaChange

BluePandaChange converts one or more Litematica `.litematic` blueprints into compressed Minecraft Structure `.nbt` files. It provides a Chinese desktop window and a command-line interface backed by the same Rust conversion library.

The converter reads Litematica `Version=6, SubVersion=1` and `Version=7, SubVersion=1` files. It does not inspect game installations or select a game version. A source `MinecraftDataVersion` is copied to the output `DataVersion` when present.

## Build

Install the Rust stable toolchain, then run `cargo build --workspace --release`. The CLI is written to `target/release/bluepanda-change`; the desktop app binary is `target/release/bluepanda-gui` (on Windows, `bluepanda-gui.exe`). `scripts/build-macos.sh` also packages `dist/BluePandaChange.app`; `scripts/build-windows.ps1` provides the Windows build command. GitHub Actions builds and tests on macOS and Windows.

## Command line

```text
bluepanda-change <INPUT>... --output-dir <DIR> [--overwrite]
```

For example:

```sh
bluepanda-change house.litematic tower.litematic --output-dir ./converted
```

Outputs use the input filename with `.nbt` appended. Existing outputs are kept unless `--overwrite` is supplied. The process returns `0` when all files convert, `1` when one or more files fail, and `2` for invalid command-line arguments. A failed file does not prevent later files from being processed.

## Desktop window

Run `cargo run -p bluepanda-gui` or launch the release binary. Add files, choose an output directory, set whether existing outputs may be replaced, and start conversion. The queue cannot be changed while conversion is running. Each file shows its own progress and result.

The bundled Noto Sans SC font is licensed under SIL Open Font License 1.1. Its license is in `crates/gui/assets/OFL.txt`.

## Conversion behavior

The converter preserves block states, block entities, and entities where the standard Structure format represents them. Regions are normalized into one enclosing bounding box. Conflicting overlapping regions fail with a location; ordinary air records are omitted. Litematica metadata and pending ticks are reported as omitted data. Outputs are staged in the destination directory, checked by reading them back, and committed only after verification.

Run workspace checks with `cargo test --workspace` and `cargo build --workspace --release`.
