# Validation record

Validated on macOS with Rust `1.96.0` on 2026-09-30. Synthetic, hand-authored, and real-sample checks provide fixed or independently decoded expectations.

## Automated checks

- `cargo test --workspace` — passed, 35 tests: parser 11, file conversion 6, transform and writer 14, GUI worker/font 4. The file tests include Unicode filenames/output paths, a manually calculated asymmetric `2×2×3` layout, duplicate source de-duplication, same-name output collision failure even with overwrite, existing target preservation, overwrite after verification, and a corrupt file mixed with successful files.
- GUI unit tests start a real background batch containing one valid and one damaged file, verify input-to-row result matching and queue lock/recovery, test no-input/no-output start guards, exercise reordered worker result delivery, and parse the embedded font while checking common Chinese UI glyphs.
- `cargo build --workspace --release` — passed. The macOS packaging script also invokes this workspace release build.
- `cargo fmt --all -- --check` — passed.
- `scripts/build-macos.sh` — passed; produced `target/release/bluepanda-change` and `dist/BluePandaChange.app`. `plutil -lint dist/BluePandaChange.app/Contents/Info.plist` — passed.
- `cargo check --workspace` — passed.
- CLI exit behavior — `--help` returned `0`; invocation with no required inputs returned `2`; a batch with one valid fixture and one missing file returned `1` after creating the valid file's output.

## Release CLI pressure samples

Generated synthetic GZIP NBT fixtures with `cargo run -p bluepanda-core --example make_bench_fixtures -- .validation/bench-inputs`, then converted them with the release CLI under `/usr/bin/time -l`. Measurements are from this macOS host; peak resident set size is reported exactly as bytes from `time -l`.

| Sample | Output blocks | Elapsed | Maximum resident set size |
|---|---:|---:|---:|
| 100,000 stone blocks | 100,000 | 0.34 s | 73,662,464 bytes |
| 1,000,000 stone blocks | 1,000,000 | 3.18 s | 709,640,192 bytes |
| Two one-block regions 1,000,000,000 blocks apart on X | 2 | 0.33 s | 2,883,584 bytes |

The distant-region result demonstrates that memory follows region contents and emitted records rather than allocating the full enclosing bounding box. Peak memory grows with the number of emitted block records: the one-million-block sample used about 677 MiB on this host. The generator and repeatable measurement command are in `crates/core/examples/make_bench_fixtures.rs` and `scripts/benchmark-macos.sh`.

## Real sample: 古风小院.litematic

An initial run, before the user authorized 7.1 support, rejected this file as unsupported. That run returned `1` after `0.47 s`, with maximum RSS `8,306,688` bytes, and wrote no output. This refusal preserved the original 6.1-only scope at that time.

After the explicit 7.1 support change, the release CLI converted the sample into a new project-local directory:

```text
/usr/bin/time -l target/release/bluepanda-change \
  /Users/yuanshi/Downloads/古风小院.litematic \
  --output-dir /Users/yuanshi/Documents/My project/BluePandaChange/real-sample-output-v7
```

It returned exit code `0` after `1.54 s`, with maximum RSS `90,865,664` bytes. The CLI's original-name output is `/Users/yuanshi/Documents/My project/BluePandaChange/real-sample-output-v7/古风小院.litematic.nbt`; the delivered friendly-name copy is `/Users/yuanshi/Documents/My project/BluePandaChange/real-sample-output-v7/古风小院.nbt`. Both files have SHA-256 `4fc1b2a17a5d31d3760e002720593bfafad406db11ade591d90a4fd248237794` and contain identical bytes. The output is 243 KiB compressed and 3,194,815 bytes decompressed. The CLI reports 85,318 blocks and 18 entities. The file remains GZIP-compressed and passes the independent reader's full NBT/trailer checks.

The exact shell command was `/usr/bin/time -l target/release/bluepanda-change '/Users/yuanshi/Downloads/古风小院.litematic' --output-dir '/Users/yuanshi/Documents/My project/BluePandaChange/real-sample-output-v7'`; `/usr/bin/time` reported 1.54 seconds elapsed and 90,865,664 bytes maximum resident set size. The output was written to a previously unused directory, with overwrite disabled.

The source SHA-256 before and after conversion was `9fee6162a557b3628dabca5424a92a46a0ed9b5c4a1ea8ed40a2585e0576c0e8`. The independent typed reader in `scripts/validate_real_sample.py` correctly decodes Java Modified UTF-8 and preserves NBT tag types; it does not use the core parser or convert through JSON. It confirmed source `Version=7`, `SubVersion=1`, `MinecraftDataVersion=3955`, one Region `Unnamed`, Position `(124, 58, 0)`, signed Size `(-125, -59, 115)`, inclusive minimum `(0, 0, 0)`, normalized output size `(125, 59, 115)`, palette length `545`, 10 bits per block, 132,520 packed longs, and 848,125 raw cells. The independently decoded non-air count is 85,318.

The verifier compared every non-air coordinate and `Name`/`Properties` state after X-fastest packed-stream decoding. Every output palette reference was in range, every output block coordinate was unique and inside `size`, and the output state at every coordinate matched the source. All 418 block entities matched by normalized position after removing only source top-level `x/y/z`; their remaining typed NBT payloads matched exactly. All 18 entities matched their translated `Pos`, `blockPos`, and typed NBT after translating known top-level fields by `(124, 58, 0)`. IDs were 17 bats and one chicken. The first entity position changed from `(-82.77139099663044, -51.99482395297761, 52.55215343203169)` to `(41.228609003369556, 6.005176047022388, 52.55215343203169)`.

This sample has no `block_pos`, `home_pos`, `leash`, `TileXYZ`, or `SleepingXYZ` entity fields, so those modern coordinate variants are covered by targeted synthetic tests rather than claimed as real-sample observations. The earlier refusal used an empty output directory that was removed; the successful conversion used a distinct new path and did not overwrite anything.

## Desktop application

- `cargo build -p bluepanda-gui` and launch of both the executable and packaged `.app` — passed at process level; both remained running.
- The native GUI window and file picker could not be visually exercised in this session. `screencapture -x` captured the active desktop as a black lock-screen view with only system widgets; the launched process did not appear in that captured desktop, and no UI automation/control tool was available to select files. The GUI worker, queue guards, result mapping, completion recovery, and embedded Chinese glyph coverage are verified by automated tests; native rendering and dialog interaction remain unverified.

## Platform status

Windows scripts and CI configuration are included but were not run on Windows. No remote CI run, release publication, or game integration was used.
