use bluepanda_core::{
    build_structure, convert_batch, parse_schem, parse_schematic, read_gzip_nbt, ConversionOptions,
    Stage, Structure,
};
use fastnbt::{ByteArray, IntArray, LongArray, Value};
use flate2::{write::GzEncoder, Compression};
use std::fs;
use std::io::{Cursor, Write};
use std::path::Path;

fn compound<K: Into<String>>(entries: impl IntoIterator<Item = (K, Value)>) -> Value {
    Value::Compound(entries.into_iter().map(|(k, v)| (k.into(), v)).collect())
}

fn gzip_nbt(root: &Value) -> Vec<u8> {
    let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
    fastnbt::to_writer(&mut gzip, root).unwrap();
    gzip.finish().unwrap()
}

fn write_nbt(path: &Path, root: &Value) {
    fs::write(path, gzip_nbt(root)).unwrap();
}

fn encode_varints(values: &[u32]) -> Vec<i8> {
    let mut bytes = Vec::new();
    for &value in values {
        let mut n = value;
        loop {
            let mut byte = (n & 0x7f) as u8;
            n >>= 7;
            if n != 0 {
                byte |= 0x80;
            }
            bytes.push(byte as i8);
            if n == 0 {
                break;
            }
        }
    }
    bytes
}

fn palette(entries: &[(&str, i32)]) -> Value {
    compound(
        entries
            .iter()
            .map(|(name, id)| (String::from(*name), Value::Int(*id))),
    )
}

fn v1_v2(version: i32, data: Vec<i8>, extra: Vec<(String, Value)>) -> Value {
    let mut entries: Vec<(String, Value)> = vec![
        ("Version".into(), Value::Int(version)),
        ("Width".into(), Value::Short(2)),
        ("Height".into(), Value::Short(2)),
        ("Length".into(), Value::Short(3)),
        (
            "Palette".into(),
            palette(&[("air", 0), ("minecraft:stone", 1)]),
        ),
        ("BlockData".into(), Value::ByteArray(ByteArray::new(data))),
    ];
    entries.extend(extra);
    compound(entries)
}

fn nested_v3(blocks: Option<Value>, entities: Option<Value>) -> Value {
    let mut schematic: Vec<(String, Value)> = vec![
        ("Version".into(), Value::Int(3)),
        ("DataVersion".into(), Value::Int(3700)),
        ("Width".into(), Value::Short(2)),
        ("Height".into(), Value::Short(2)),
        ("Length".into(), Value::Short(3)),
    ];
    if let Some(blocks) = blocks {
        schematic.push(("Blocks".into(), blocks));
    }
    if let Some(entities) = entities {
        schematic.push(("Entities".into(), entities));
    }
    compound([("Schematic", compound(schematic))])
}

fn v3_blocks(data: Vec<i8>) -> Value {
    compound([
        (
            "Palette",
            palette(&[("minecraft:air", 0), ("minecraft:stone", 1)]),
        ),
        ("Data", Value::ByteArray(ByteArray::new(data))),
    ])
}

fn test_entity(id: &str, pos: [f64; 3], extra: Vec<(String, Value)>) -> Value {
    let mut fields = vec![
        ("Id".into(), Value::String(id.into())),
        (
            "Pos".into(),
            Value::List(pos.into_iter().map(Value::Double).collect()),
        ),
    ];
    fields.extend(extra);
    compound(fields)
}

#[test]
fn reads_v1_v2_and_nested_v3_with_hand_calculated_x_fastest_coordinates() {
    // The 12 cells use X-fastest order. Stone occupies linear entries 1, 2, 4, and 7.
    let cells = [0u32, 1, 1, 0, 1, 0, 0, 1, 0, 0, 0, 0];
    let data = encode_varints(&cells);
    let v1 = v1_v2(1, data.clone(), vec![]);
    let v2 = v1_v2(
        2,
        data.clone(),
        vec![("DataVersion".into(), Value::Int(3465))],
    );
    let v3 = nested_v3(Some(v3_blocks(data)), None);

    for (version, root, expected_data_version) in
        [(1, v1, None), (2, v2, Some(3465)), (3, v3, Some(3700))]
    {
        let parsed = parse_schem(Cursor::new(gzip_nbt(&root))).unwrap();
        assert_eq!(
            parsed.data_version, expected_data_version,
            "version {version}"
        );
        assert_eq!(parsed.regions.len(), 1);
        assert_eq!(parsed.regions[0].size, [2, 2, 3]);
        assert_eq!(parsed.regions[0].palette[0].name, "minecraft:air");
        let structure = build_structure(&parsed).unwrap();
        assert_eq!(structure.size, [2, 2, 3]);
        let mut actual: Vec<_> = structure
            .blocks
            .iter()
            .map(|block| {
                (
                    block.pos,
                    structure.palette[block.state as usize].name.as_str(),
                )
            })
            .collect();
        actual.sort_by_key(|(pos, _)| *pos);
        assert_eq!(
            actual,
            vec![
                ([0, 0, 1], "minecraft:stone"),
                ([0, 0, 2], "minecraft:stone"),
                ([1, 0, 0], "minecraft:stone"),
                ([1, 1, 0], "minecraft:stone"),
            ],
            "version {version}"
        );
    }
}

#[test]
fn interprets_unsigned_short_dimensions_and_rejects_global_palette_fallback() {
    for (raw, expected) in [(-32768i16, 32768), (-1i16, 65535)] {
        let root = compound([(
            "Schematic",
            compound([
                ("Version", Value::Int(3)),
                ("DataVersion", Value::Int(3700)),
                ("Width", Value::Short(raw)),
                ("Height", Value::Short(1)),
                ("Length", Value::Short(1)),
            ]),
        )]);
        let parsed = parse_schem(Cursor::new(gzip_nbt(&root))).unwrap();
        assert_eq!(parsed.regions[0].size, [expected as i32, 1, 1]);
        assert_eq!(
            parsed.regions[0].packed_states.len(),
            (expected as usize * 2 + 63) / 64
        );
    }

    let no_local_palette = v1_v2(1, vec![0; 12], vec![]);
    let Value::Compound(mut root) = no_local_palette else {
        unreachable!()
    };
    root.remove("Palette");
    let error = parse_schem(Cursor::new(gzip_nbt(&Value::Compound(root)))).unwrap_err();
    assert_eq!(error.stage, Stage::Validate);
    assert!(error.message.contains("local Palette"));
}

#[test]
fn remaps_sparse_varint_palette_ids_and_packs_across_a_long_boundary() {
    // Sparse wire IDs are sorted and remapped to compact IDs. Five states require three packed
    // bits, so the 22nd decoded state straddles the 64-bit boundary at bit 63.
    let ids = [7u32, 127, 128, 16_512, 900_000];
    let sequence: Vec<u32> = (0..22).map(|i| ids[i % ids.len()]).collect();
    let mut entries: Vec<_> = ids
        .iter()
        .enumerate()
        .map(|(i, id)| (format!("mod:block_{i}"), *id as i32))
        .collect();
    entries.reverse(); // NBT compound iteration order is not a semantic index order.
    let mut root = v1_v2(
        2,
        encode_varints(&sequence),
        vec![("DataVersion".into(), Value::Int(3700))],
    );
    let Value::Compound(ref mut root) = root else {
        unreachable!()
    };
    root.insert("Width".into(), Value::Short(22));
    root.insert("Height".into(), Value::Short(1));
    root.insert("Length".into(), Value::Short(1));
    root.insert(
        "Palette".into(),
        compound(entries.into_iter().map(|(name, id)| (name, Value::Int(id)))),
    );
    let parsed = parse_schem(Cursor::new(gzip_nbt(&Value::Compound(root.clone())))).unwrap();
    assert_eq!(parsed.regions[0].palette.len(), 5);
    assert_eq!(parsed.regions[0].packed_states.len(), 2);
    let actual: Vec<_> = (0..22)
        .map(|i| bluepanda_core::parser::decode_palette_index(&parsed.regions[0], i).unwrap())
        .collect();
    assert_eq!(actual, (0..22).map(|i| i % 5).collect::<Vec<_>>());
    assert_eq!(parsed.regions[0].palette[0].name, "mod:block_0");
}

#[test]
fn rejects_truncated_oversized_overflowing_and_unreferenced_varints() {
    let malformed = [
        (vec![0x80u8 as i8], "truncated VarInt"),
        // A continuation bit in byte five proves the encoding would need a sixth byte.
        (
            vec![
                0x80u8 as i8,
                0x80u8 as i8,
                0x80u8 as i8,
                0x80u8 as i8,
                0x87u8 as i8,
            ],
            "exceeds five bytes",
        ),
        (
            vec![0xffu8 as i8, 0xffu8 as i8, 0xffu8 as i8, 0xffu8 as i8, 0x10],
            "overflows 32 bits",
        ),
        (vec![0x02], "missing palette index 2"),
        (vec![0x00, 0x00], "extra bytes"),
    ];
    for (bytes, message) in malformed {
        // One cell keeps each byte sequence aligned to exactly one requested palette reference.
        let mut root = v1_v2(2, bytes, vec![("DataVersion".into(), Value::Int(3700))]);
        let Value::Compound(ref mut root) = root else {
            unreachable!()
        };
        root.insert("Width".into(), Value::Short(1));
        root.insert("Height".into(), Value::Short(1));
        root.insert("Length".into(), Value::Short(1));
        let error = parse_schem(Cursor::new(gzip_nbt(&Value::Compound(root.clone())))).unwrap_err();
        assert_eq!(error.stage, Stage::Validate, "{message}: {error}");
        assert!(error.message.contains(message), "{message}: {error}");
    }

    let mut negative = v1_v2(1, vec![0], vec![]);
    let Value::Compound(ref mut c) = negative else {
        unreachable!()
    };
    c.insert("Width".into(), Value::Short(1));
    c.insert("Height".into(), Value::Short(1));
    c.insert("Length".into(), Value::Short(1));
    c.insert("Palette".into(), palette(&[("minecraft:air", -1)]));
    let error = parse_schem(Cursor::new(gzip_nbt(&negative))).unwrap_err();
    assert!(error.message.contains("non-negative Int"));
}

#[test]
fn rejects_malformed_state_keys_and_invalid_palette_max() {
    for state in [
        "mod:",
        ":stone",
        "mod:block[outer[inner=x]]",
        "mod:block[axis=x=y]",
        "mod:block[axis=x,axis=z]",
    ] {
        let root = v1_v2(
            1,
            vec![0; 12],
            vec![("Palette".into(), palette(&[(state, 0)]))],
        );
        let error = parse_schem(Cursor::new(gzip_nbt(&root))).unwrap_err();
        assert_eq!(error.stage, Stage::Validate, "state {state:?}: {error}");
    }

    let valid_unknown_mod = v1_v2(
        1,
        vec![0; 12],
        vec![(
            "Palette".into(),
            palette(&[("mod:block[property=value]", 0)]),
        )],
    );
    let parsed = parse_schem(Cursor::new(gzip_nbt(&valid_unknown_mod))).unwrap();
    assert_eq!(parsed.regions[0].palette[0].name, "mod:block");
    assert_eq!(parsed.regions[0].palette[0].properties["property"], "value");

    let mut missing_data_version = nested_v3(None, None);
    let Value::Compound(ref mut outer) = missing_data_version else {
        unreachable!()
    };
    let Value::Compound(inner) = outer.get_mut("Schematic").unwrap() else {
        unreachable!()
    };
    inner.remove("DataVersion");
    let error = parse_schem(Cursor::new(gzip_nbt(&missing_data_version))).unwrap_err();
    assert!(error.message.contains("DataVersion"));

    for palette_max in [Value::Int(0), Value::Int(-1), Value::String("2".into())] {
        let root = compound([
            ("Version", Value::Int(2)),
            ("DataVersion", Value::Int(3700)),
            ("Width", Value::Short(1)),
            ("Height", Value::Short(1)),
            ("Length", Value::Short(1)),
            ("Palette", palette(&[("minecraft:air", 0)])),
            ("PaletteMax", palette_max),
            ("BlockData", Value::ByteArray(ByteArray::new(vec![0]))),
        ]);
        assert!(parse_schem(Cursor::new(gzip_nbt(&root))).is_err());
    }

    let outside_max = compound([
        ("Version", Value::Int(2)),
        ("DataVersion", Value::Int(3700)),
        ("Width", Value::Short(1)),
        ("Height", Value::Short(1)),
        ("Length", Value::Short(1)),
        (
            "Palette",
            palette(&[("minecraft:air", 0), ("minecraft:stone", 2)]),
        ),
        ("PaletteMax", Value::Int(2)),
        ("BlockData", Value::ByteArray(ByteArray::new(vec![0]))),
    ]);
    assert!(parse_schem(Cursor::new(gzip_nbt(&outside_max))).is_err());
}

#[test]
fn validates_data_lengths_versions_and_block_entity_entity_payloads() {
    let missing_data_version = v1_v2(2, vec![0; 12], vec![]);
    assert!(parse_schem(Cursor::new(gzip_nbt(&missing_data_version)))
        .unwrap_err()
        .message
        .contains("DataVersion"));

    let mut short_data = v1_v2(1, vec![0], vec![]);
    let Value::Compound(ref mut c) = short_data else {
        unreachable!()
    };
    c.insert("Width".into(), Value::Short(2));
    c.insert("Height".into(), Value::Short(1));
    c.insert("Length".into(), Value::Short(1));
    let error = parse_schem(Cursor::new(gzip_nbt(&short_data))).unwrap_err();
    assert!(error.message.contains("at least 2"));

    let be_v2 = compound([
        ("Pos", Value::IntArray(IntArray::new(vec![1, 0, 0]))),
        ("Id", Value::String("mod:machine".into())),
        ("Energy", Value::Long(9_007_199_254_740_993)),
        ("Flags", Value::ByteArray(ByteArray::new(vec![-1, 2]))),
    ]);
    let mut v2 = v1_v2(
        2,
        encode_varints(&[0; 12]),
        vec![
            ("DataVersion".into(), Value::Int(3700)),
            ("BlockEntities".into(), Value::List(vec![be_v2])),
            (
                "Entities".into(),
                Value::List(vec![test_entity("mod:orb", [0.25, 1.5, 2.75], vec![])]),
            ),
        ],
    );
    let Value::Compound(ref mut c) = v2 else {
        unreachable!()
    };
    // Place the block entity on a non-air block and retain the original typed payload fields.
    c.insert(
        "BlockData".into(),
        Value::ByteArray(ByteArray::new(encode_varints(&[
            0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ]))),
    );
    let parsed = parse_schem(Cursor::new(gzip_nbt(&v2))).unwrap();
    let Value::Compound(be) = &parsed.regions[0].block_entities[0] else {
        panic!()
    };
    assert_eq!(be.get("id"), Some(&Value::String("mod:machine".into())));
    assert_eq!(be.get("Energy"), Some(&Value::Long(9_007_199_254_740_993)));
    assert_eq!(
        be.get("Flags"),
        Some(&Value::ByteArray(ByteArray::new(vec![-1, 2])))
    );
    assert!(!be.contains_key("Pos") && be.get("x") == Some(&Value::Int(1)));
    assert_eq!(parsed.regions[0].entities.len(), 1);

    let nested_be = compound([
        ("Pos", Value::IntArray(IntArray::new(vec![1, 0, 0]))),
        ("Id", Value::String("mod:chest".into())),
        (
            "Data",
            compound([
                ("CustomName", Value::String("typed name".into())),
                ("Precise", Value::Double(0.125)),
            ]),
        ),
    ]);
    let nested_entity = test_entity(
        "mod:drone",
        [0.5, 0.25, 1.75],
        vec![(
            "Data".into(),
            compound([
                ("Health", Value::Float(3.5)),
                ("Exact", Value::Long(8_000_000_000)),
            ]),
        )],
    );
    let mut blocks = match v3_blocks(encode_varints(&[0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])) {
        Value::Compound(c) => c,
        _ => unreachable!(),
    };
    blocks.insert("BlockEntities".into(), Value::List(vec![nested_be]));
    let v3 = nested_v3(
        Some(Value::Compound(blocks)),
        Some(Value::List(vec![nested_entity])),
    );
    let parsed = parse_schem(Cursor::new(gzip_nbt(&v3))).unwrap();
    let Value::Compound(be) = &parsed.regions[0].block_entities[0] else {
        panic!()
    };
    assert_eq!(
        be.get("CustomName"),
        Some(&Value::String("typed name".into()))
    );
    assert_eq!(be.get("Precise"), Some(&Value::Double(0.125)));
    let Value::Compound(entity) = &parsed.regions[0].entities[0] else {
        panic!()
    };
    assert_eq!(entity.get("Health"), Some(&Value::Float(3.5)));
    assert_eq!(entity.get("Exact"), Some(&Value::Long(8_000_000_000)));
}

#[test]
fn rejects_conflicting_ids_and_block_entity_coordinates() {
    let conflicting_be = compound([
        ("Pos", Value::IntArray(IntArray::new(vec![0, 0, 0]))),
        ("Id", Value::String("mod:machine".into())),
        ("id", Value::String("mod:other".into())),
    ]);
    let root = v1_v2(
        2,
        vec![0; 12],
        vec![
            ("DataVersion".into(), Value::Int(3700)),
            ("BlockEntities".into(), Value::List(vec![conflicting_be])),
        ],
    );
    assert!(parse_schem(Cursor::new(gzip_nbt(&root)))
        .unwrap_err()
        .message
        .contains("Id conflicts"));

    let coordinate_conflict = compound([
        ("Pos", Value::IntArray(IntArray::new(vec![0, 0, 0]))),
        ("Id", Value::String("mod:machine".into())),
        ("x", Value::Int(1)),
    ]);
    let root = v1_v2(
        1,
        vec![0; 12],
        vec![(
            "TileEntities".into(),
            Value::List(vec![coordinate_conflict]),
        )],
    );
    assert!(parse_schem(Cursor::new(gzip_nbt(&root)))
        .unwrap_err()
        .message
        .contains("Pos conflicts"));

    let wrong_entity_position = compound([
        ("Id", Value::String("minecraft:pig".into())),
        (
            "Pos",
            Value::List(vec![
                Value::Float(0.0),
                Value::Float(0.0),
                Value::Float(0.0),
            ]),
        ),
    ]);
    let root = v1_v2(
        2,
        vec![0; 12],
        vec![
            ("DataVersion".into(), Value::Int(3700)),
            ("Entities".into(), Value::List(vec![wrong_entity_position])),
        ],
    );
    let error = parse_schem(Cursor::new(gzip_nbt(&root))).unwrap_err();
    assert_eq!(error.stage, Stage::Validate);
    assert!(error.message.contains("Pos"));
}

#[test]
fn supports_v3_entity_only_and_routes_all_three_canonical_roots() {
    let entity = test_entity(
        "minecraft:armor_stand",
        [0.25, 0.5, 0.75],
        vec![("CustomName".into(), Value::String("test".into()))],
    );
    let root = nested_v3(None, Some(Value::List(vec![entity])));
    let parsed = parse_schematic(Cursor::new(gzip_nbt(&root))).unwrap();
    assert_eq!(parsed.regions[0].palette[0].name, "minecraft:air");
    assert!(parsed.regions[0].entities.len() == 1);
    let structure = build_structure(&parsed).unwrap();
    assert!(structure.blocks.is_empty());
    assert_eq!(structure.entities[0].pos, [0.25, 0.5, 0.75]);

    let v1 = v1_v2(1, vec![0; 12], vec![]);
    let v2 = v1_v2(
        2,
        vec![0; 12],
        vec![("DataVersion".into(), Value::Int(3700))],
    );
    for root in [v1, v2, nested_v3(None, None)] {
        assert!(parse_schematic(Cursor::new(gzip_nbt(&root))).is_ok());
    }
}

#[test]
fn forwards_version_specific_warnings_without_applying_sponge_offset() {
    let tile = compound([
        ("ContentVersion", Value::Int(1)),
        ("Pos", Value::IntArray(IntArray::new(vec![0, 0, 0]))),
        ("Id", Value::String("mod:machine".into())),
        ("CustomLong", Value::Long(4_294_967_297)),
    ]);
    let v1 = v1_v2(
        1,
        vec![0; 12],
        vec![
            (
                "Metadata".into(),
                compound([("Name", Value::String("fixture".into()))]),
            ),
            (
                "Offset".into(),
                Value::IntArray(IntArray::new(vec![4, -2, 8])),
            ),
            ("TileEntities".into(), Value::List(vec![tile])),
        ],
    );
    let parsed = parse_schem(Cursor::new(gzip_nbt(&v1))).unwrap();
    assert!(parsed
        .warnings
        .iter()
        .any(|w| w.message.contains("metadata")));
    assert!(parsed.warnings.iter().any(|w| w.message.contains("Offset")));
    assert!(parsed
        .warnings
        .iter()
        .any(|w| w.message.contains("ContentVersion")));
    assert_eq!(parsed.regions[0].position, [0, 0, 0]);
    let Value::Compound(be) = &parsed.regions[0].block_entities[0] else {
        panic!()
    };
    assert!(!be.contains_key("ContentVersion"));
    assert_eq!(be.get("CustomLong"), Some(&Value::Long(4_294_967_297)));

    // Version 2 treats an identically named payload field as user data, not a v1 wrapper.
    let tile = compound([
        ("Pos", Value::IntArray(IntArray::new(vec![0, 0, 0]))),
        ("Id", Value::String("mod:machine".into())),
        ("ContentVersion", Value::Int(19)),
    ]);
    let v2 = v1_v2(
        2,
        vec![0; 12],
        vec![
            ("DataVersion".into(), Value::Int(3700)),
            ("BlockEntities".into(), Value::List(vec![tile])),
        ],
    );
    let parsed = parse_schem(Cursor::new(gzip_nbt(&v2))).unwrap();
    let Value::Compound(be) = &parsed.regions[0].block_entities[0] else {
        panic!()
    };
    assert_eq!(be.get("ContentVersion"), Some(&Value::Int(19)));
    assert!(!parsed
        .warnings
        .iter()
        .any(|w| w.message.contains("ContentVersion")));
}

#[test]
fn rejects_trailing_nbt_bytes_trailing_gzip_bytes_and_bad_gzip_crc() {
    let valid = v1_v2(1, vec![0; 12], vec![]);

    let mut nbt_with_garbage = fastnbt::to_bytes(&valid).unwrap();
    nbt_with_garbage.extend_from_slice(&[0x7f, 0x01]);
    let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
    gzip.write_all(&nbt_with_garbage).unwrap();
    let error = parse_schem(Cursor::new(gzip.finish().unwrap())).unwrap_err();
    assert_eq!(error.stage, Stage::Parse);

    let mut gzip_with_garbage = gzip_nbt(&valid);
    gzip_with_garbage.extend_from_slice(b"extra");
    let error = parse_schematic(Cursor::new(gzip_with_garbage)).unwrap_err();
    assert_eq!(error.stage, Stage::Decompress);
    assert!(error.message.contains("trailing"));

    let mut bad_crc = gzip_nbt(&valid);
    let last = bad_crc.last_mut().unwrap();
    *last ^= 0x80;
    let error = parse_schem(Cursor::new(bad_crc)).unwrap_err();
    assert_eq!(error.stage, Stage::Decompress);
}

#[test]
fn converts_v3_entities_and_block_entities_with_warnings_and_typed_roundtrip() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("typed.schem");
    let output = temp.path().join("typed.nbt");
    let block_entity = compound([
        ("Pos", Value::IntArray(IntArray::new(vec![1, 0, 0]))),
        ("Id", Value::String("mod:machine".into())),
        (
            "Data",
            compound([
                ("Energy", Value::Long(9_007_199_254_740_993)),
                ("Flags", Value::ByteArray(ByteArray::new(vec![1, -2]))),
            ]),
        ),
    ]);
    let entity = test_entity(
        "mod:drone",
        [0.25, 1.5, 2.75],
        vec![("Data".into(), compound([("Health", Value::Float(3.5))]))],
    );
    let mut blocks = match v3_blocks(encode_varints(&[0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])) {
        Value::Compound(c) => c,
        _ => unreachable!(),
    };
    blocks.insert("BlockEntities".into(), Value::List(vec![block_entity]));
    let mut outer = match nested_v3(
        Some(Value::Compound(blocks)),
        Some(Value::List(vec![entity])),
    ) {
        Value::Compound(c) => c,
        _ => unreachable!(),
    };
    let Value::Compound(inner) = outer.get_mut("Schematic").unwrap() else {
        unreachable!()
    };
    inner.insert(
        "Offset".into(),
        Value::IntArray(IntArray::new(vec![4, 0, -3])),
    );
    inner.insert(
        "Metadata".into(),
        compound([("Name", Value::String("fixture".into()))]),
    );
    inner.insert("Biomes".into(), compound(Vec::<(String, Value)>::new()));
    write_nbt(&input, &Value::Compound(outer));

    let result =
        bluepanda_core::convert_file(&input, &output, &ConversionOptions::default(), |_| {})
            .unwrap();
    assert_eq!((result.blocks, result.entities), (1, 1));
    for warning in ["metadata", "Offset", "biomes"] {
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.message.to_lowercase().contains(&warning.to_lowercase())),
            "missing {warning} warning: {:?}",
            result.warnings
        );
    }

    let output_structure: Structure = read_gzip_nbt(fs::File::open(output).unwrap()).unwrap();
    assert_eq!(output_structure.size, [2, 2, 3]);
    assert_eq!(output_structure.data_version, Some(3700));
    assert_eq!(output_structure.entities.len(), 1);
    assert_eq!(output_structure.entities[0].pos, [0.25, 1.5, 2.75]);
    let Value::Compound(entity_nbt) = &output_structure.entities[0].nbt else {
        panic!()
    };
    assert_eq!(
        entity_nbt.get("id"),
        Some(&Value::String("mod:drone".into()))
    );
    assert_eq!(entity_nbt.get("Health"), Some(&Value::Float(3.5)));
    let Value::Compound(be_nbt) = output_structure.blocks[0].nbt.as_ref().unwrap() else {
        panic!()
    };
    assert_eq!(be_nbt.get("id"), Some(&Value::String("mod:machine".into())));
    assert_eq!(
        be_nbt.get("Energy"),
        Some(&Value::Long(9_007_199_254_740_993))
    );
    assert_eq!(
        be_nbt.get("Flags"),
        Some(&Value::ByteArray(ByteArray::new(vec![1, -2])))
    );
}

fn litematic_root() -> Value {
    let block_state = compound([("Name", Value::String("minecraft:stone".into()))]);
    let region = compound([
        (
            "Position",
            compound([
                ("x", Value::Int(0)),
                ("y", Value::Int(0)),
                ("z", Value::Int(0)),
            ]),
        ),
        (
            "Size",
            compound([
                ("x", Value::Int(1)),
                ("y", Value::Int(1)),
                ("z", Value::Int(1)),
            ]),
        ),
        ("BlockStatePalette", Value::List(vec![block_state])),
        ("BlockStates", Value::LongArray(LongArray::new(vec![0i64]))),
        ("TileEntities", Value::List(vec![])),
        ("Entities", Value::List(vec![])),
    ]);
    compound([
        ("Version", Value::Int(6)),
        ("SubVersion", Value::Int(1)),
        ("Regions", compound([("sample", region)])),
    ])
}

#[test]
fn mixed_batch_continues_after_bad_schematic_and_protects_collisions() {
    let temp = tempfile::tempdir().unwrap();
    let litematic = temp.path().join("house.litematic");
    let schem = temp.path().join("garden.schem");
    let broken = temp.path().join("broken.schem");
    write_nbt(&litematic, &litematic_root());
    let source_schem = v2_with_warnings();
    write_nbt(&schem, &source_schem);
    fs::write(&broken, b"not gzip").unwrap();
    let output = temp.path().join("converted");
    let batch = convert_batch(
        &[litematic.clone(), schem.clone(), broken.clone()],
        &output,
        &ConversionOptions::default(),
        |_| {},
    );
    assert_eq!(batch.results.len(), 3);
    assert!(batch.results[0].result.is_ok());
    let schematic_result = batch.results[1].result.as_ref().unwrap();
    assert!(schematic_result
        .warnings
        .iter()
        .any(|w| w.message.contains("metadata")));
    assert!(batch.results[2].result.is_err());
    assert!(output.join("house.litematic.nbt").exists());
    assert!(output.join("garden.schem.nbt").exists());
    let structure: Structure =
        read_gzip_nbt(fs::File::open(output.join("garden.schem.nbt")).unwrap()).unwrap();
    assert_eq!(structure.size, [1, 1, 1]);
    assert_eq!(structure.data_version, Some(3700));
    assert_eq!(structure.blocks.len(), 1);
    assert_eq!(
        structure.palette[structure.blocks[0].state as usize].name,
        "minecraft:stone"
    );

    let left = temp.path().join("a/house.schem");
    let right = temp.path().join("b/house.schem");
    fs::create_dir_all(left.parent().unwrap()).unwrap();
    fs::create_dir_all(right.parent().unwrap()).unwrap();
    write_nbt(&left, &source_schem);
    write_nbt(&right, &source_schem);
    let collision_out = temp.path().join("collisions");
    let existing = collision_out.join("house.schem.nbt");
    fs::create_dir_all(&collision_out).unwrap();
    fs::write(&existing, b"retain existing output").unwrap();
    let batch = convert_batch(
        &[left, right, schem],
        &collision_out,
        &ConversionOptions { overwrite: true },
        |_| {},
    );
    assert!(batch.results[0].result.is_err());
    assert!(batch.results[1].result.is_err());
    assert!(batch.results[2].result.is_ok());
    assert_eq!(fs::read(existing).unwrap(), b"retain existing output");
}

fn v2_with_warnings() -> Value {
    compound([
        ("Version", Value::Int(2)),
        ("DataVersion", Value::Int(3700)),
        ("Width", Value::Short(1)),
        ("Height", Value::Short(1)),
        ("Length", Value::Short(1)),
        ("Palette", palette(&[("minecraft:stone", 0)])),
        ("BlockData", Value::ByteArray(ByteArray::new(vec![0]))),
        (
            "Metadata",
            compound([("Name", Value::String("garden".into()))]),
        ),
    ])
}
