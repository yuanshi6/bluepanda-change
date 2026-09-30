use bluepanda_core::{
    convert_batch, convert_file, read_gzip_nbt, ConversionOptions, Stage, Structure,
};
use fastnbt::Value;
use flate2::write::GzEncoder;
use flate2::Compression;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

fn compound(entries: impl IntoIterator<Item = (String, Value)>) -> Value {
    Value::Compound(entries.into_iter().collect::<HashMap<_, _>>())
}

fn vec3(x: i32, y: i32, z: i32) -> Value {
    compound([
        ("x".into(), Value::Int(x)),
        ("y".into(), Value::Int(y)),
        ("z".into(), Value::Int(z)),
    ])
}

/// Hand-authored one-cell Litematica 6.1 sample with a single air state.
fn write_sample(path: &Path) {
    let state = compound([("Name".into(), Value::String("minecraft:air".into()))]);
    let region = compound([
        ("Position".into(), vec3(-3, 4, 8)),
        ("Size".into(), vec3(1, 1, 1)),
        ("BlockStatePalette".into(), Value::List(vec![state])),
        (
            "BlockStates".into(),
            Value::LongArray(fastnbt::LongArray::new(vec![0i64])),
        ),
        ("TileEntities".into(), Value::List(vec![])),
        ("Entities".into(), Value::List(vec![])),
    ]);
    let root = compound([
        ("Version".into(), Value::Int(6)),
        ("SubVersion".into(), Value::Int(1)),
        ("MinecraftDataVersion".into(), Value::Int(3700)),
        ("Regions".into(), compound([("Region".into(), region)])),
    ]);
    let file = fs::File::create(path).unwrap();
    let mut gzip = GzEncoder::new(file, Compression::default());
    fastnbt::to_writer(&mut gzip, &root).unwrap();
    gzip.finish().unwrap();
}

fn write_asymmetric_sample(path: &Path) {
    let stone = compound([("Name".into(), Value::String("minecraft:stone".into()))]);
    let dirt = compound([("Name".into(), Value::String("minecraft:dirt".into()))]);
    // X-fastest index = y * X * Z + z * X + x. These four dirt entries exercise
    // both X coordinates, the Z stride, and a second Y layer in a 2x2x3 region.
    let dirt_indices = [1usize, 2, 4, 7];
    let mut words = vec![0i64];
    for index in dirt_indices {
        words[0] |= 1i64 << (index * 2);
    }
    let region = compound([
        ("Position".into(), vec3(-4, 7, 10)),
        ("Size".into(), vec3(2, 2, 3)),
        ("BlockStatePalette".into(), Value::List(vec![stone, dirt])),
        (
            "BlockStates".into(),
            Value::LongArray(fastnbt::LongArray::new(words)),
        ),
        ("TileEntities".into(), Value::List(vec![])),
        ("Entities".into(), Value::List(vec![])),
    ]);
    let root = compound([
        ("Version".into(), Value::Int(6)),
        ("SubVersion".into(), Value::Int(1)),
        ("Regions".into(), compound([("Asymmetric".into(), region)])),
    ]);
    let mut gzip = GzEncoder::new(fs::File::create(path).unwrap(), Compression::default());
    fastnbt::to_writer(&mut gzip, &root).unwrap();
    gzip.finish().unwrap();
}

#[test]
fn converts_unicode_paths_and_preserves_data_version() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("蓝图 one.litematic");
    let output_dir = temp.path().join("输出 目录");
    write_sample(&input);

    let result = convert_file(
        &input,
        &output_dir.join("蓝图 one.litematic.nbt"),
        &ConversionOptions::default(),
        |_| {},
    )
    .unwrap();
    assert_eq!(result.blocks, 0);
    assert_eq!(result.output.file_name().unwrap(), "蓝图 one.litematic.nbt");
    let structure: Structure = read_gzip_nbt(fs::File::open(result.output).unwrap()).unwrap();
    assert_eq!(structure.size, [1, 1, 1]);
    assert_eq!(structure.data_version, Some(3700));
    assert_eq!(structure.blocks.len(), 0);
}

#[test]
fn asymmetric_region_has_hand_calculated_x_fastest_coordinates() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("axis-order.litematic");
    write_asymmetric_sample(&input);
    let result = convert_file(
        &input,
        &temp.path().join("axis-order.nbt"),
        &ConversionOptions::default(),
        |_| {},
    )
    .unwrap();
    let structure: Structure = read_gzip_nbt(fs::File::open(result.output).unwrap()).unwrap();
    assert_eq!(structure.size, [2, 2, 3]);
    assert_eq!(structure.blocks.len(), 12);
    assert_eq!(
        structure
            .blocks
            .iter()
            .filter(|block| block.state == 1)
            .map(|block| block.pos)
            .collect::<Vec<_>>(),
        vec![[0, 0, 1], [0, 0, 2], [1, 0, 0], [1, 1, 0]]
    );
}

#[test]
fn preserves_existing_output_without_overwrite() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("source.litematic");
    let output = temp.path().join("source.litematic.nbt");
    write_sample(&input);
    fs::write(&output, b"keep this file").unwrap();

    let error = convert_file(&input, &output, &ConversionOptions::default(), |_| {}).unwrap_err();
    assert_eq!(error.stage, Stage::Commit);
    assert_eq!(fs::read(output).unwrap(), b"keep this file");
}

#[test]
fn all_same_stem_collisions_fail_even_with_overwrite_and_other_files_continue() {
    let temp = tempfile::tempdir().unwrap();
    let a_dir = temp.path().join("a");
    let b_dir = temp.path().join("b");
    fs::create_dir_all(&a_dir).unwrap();
    fs::create_dir_all(&b_dir).unwrap();
    let a = a_dir.join("house.litematic");
    let b = b_dir.join("house.litematic");
    let broken = temp.path().join("破损.litematic");
    let good = temp.path().join("单独.litematic");
    write_sample(&a);
    write_sample(&b);
    fs::write(&broken, b"not gzip").unwrap();
    write_sample(&good);
    let output = temp.path().join("result");

    let batch = convert_batch(
        &[a.clone(), b.clone(), broken.clone(), good.clone()],
        &output,
        &ConversionOptions { overwrite: true },
        |_| {},
    );
    assert_eq!(batch.results.len(), 4);
    assert!(batch.results[0].result.is_err());
    assert!(batch.results[1].result.is_err());
    assert!(batch.results[2].result.is_err());
    assert!(batch.results[3].result.is_ok());
    assert!(!output.join("house.litematic.nbt").exists());
    assert!(output.join("单独.litematic.nbt").exists());
}

#[test]
fn duplicate_inputs_are_deduplicated() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("single.litematic");
    write_sample(&input);
    let batch = convert_batch(
        &[input.clone(), input],
        &temp.path().join("out"),
        &ConversionOptions::default(),
        |_| {},
    );
    assert_eq!(batch.results.len(), 1);
    assert!(batch.results[0].result.is_ok());
}

#[test]
fn overwrite_replaces_only_after_verified_conversion() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("sample.litematic");
    let output = temp.path().join("target.nbt");
    write_sample(&input);
    fs::write(&output, b"old").unwrap();
    convert_file(
        &input,
        &output,
        &ConversionOptions { overwrite: true },
        |_| {},
    )
    .unwrap();
    let _: Structure = read_gzip_nbt(fs::File::open(output).unwrap()).unwrap();
}
