use bluepanda_core::transform::{build_structure, build_structure_with_warnings};
use bluepanda_core::types::{BlockState, NbtValue, ParsedSchematic, Region, Stage};
use bluepanda_core::writer::{verify_structure, write_structure};
use std::collections::{BTreeMap, HashMap};

fn state(name: &str) -> BlockState {
    BlockState {
        name: name.into(),
        properties: BTreeMap::new(),
    }
}

fn region(
    name: &str,
    position: [i32; 3],
    size: [i32; 3],
    palette: Vec<BlockState>,
    indices: &[usize],
) -> Region {
    let volume = indices.len();
    let bits = (usize::BITS - palette.len().saturating_sub(1).leading_zeros()).max(2) as usize;
    let mut packed = vec![0i64; (volume * bits).div_ceil(64)];
    for (i, n) in indices.iter().enumerate() {
        for bit in 0..bits {
            if (n >> bit) & 1 != 0 {
                let absolute = i * bits + bit;
                packed[absolute / 64] |= 1i64 << (absolute % 64);
            }
        }
    }
    Region {
        name: name.into(),
        position,
        size,
        palette,
        packed_states: packed,
        block_entities: vec![],
        entities: vec![],
    }
}

fn schematic(regions: Vec<Region>) -> ParsedSchematic {
    ParsedSchematic {
        data_version: Some(3700),
        regions,
        warnings: vec![],
    }
}

fn compound(entries: impl IntoIterator<Item = (String, NbtValue)>) -> NbtValue {
    NbtValue::Compound(entries.into_iter().collect::<HashMap<_, _>>())
}

#[test]
fn asymmetric_xyz_order_negative_size_and_nonair_only_output() {
    let palette: Vec<_> = (0..12).map(|i| state(&format!("mod:block_{i}"))).collect();
    let r = region(
        "neg",
        [5, 3, -2],
        [-2, 2, 3],
        palette,
        &(0..12).collect::<Vec<_>>(),
    );
    let out = build_structure(&schematic(vec![r])).unwrap();
    assert_eq!(out.size, [2, 2, 3]);
    // X is fastest, then Z, then Y. Check every coordinate in a 2x2x3 region.
    assert_eq!(out.blocks.len(), 12);
    for i in 0..12 {
        let x = i % 2;
        let z = (i / 2) % 3;
        let y = i / 6;
        let block = out
            .blocks
            .iter()
            .find(|b| b.pos == [x as i32, y as i32, z as i32])
            .unwrap();
        assert_eq!(
            out.palette[block.state as usize].name,
            format!("mod:block_{i}")
        );
    }
}

#[test]
fn all_air_keeps_bounds_and_uses_valid_placeholder_palette() {
    let out = build_structure(&schematic(vec![region(
        "air",
        [9, -4, 20],
        [3, 1, 2],
        vec![state("minecraft:air")],
        &[0; 6],
    )]))
    .unwrap();
    assert_eq!(out.size, [3, 1, 2]);
    assert!(out.blocks.is_empty());
    assert_eq!(out.palette.len(), 1);
    assert_eq!(out.palette[0].name, "minecraft:air");
    assert_eq!(out.data_version, Some(3700));
}

#[test]
fn structure_void_is_preserved_as_a_block() {
    let out = build_structure(&schematic(vec![region(
        "void",
        [0, 0, 0],
        [1, 1, 1],
        vec![state("minecraft:structure_void")],
        &[0],
    )]))
    .unwrap();
    assert_eq!(out.blocks.len(), 1);
    assert_eq!(
        out.palette[out.blocks[0].state as usize].name,
        "minecraft:structure_void"
    );
}

#[test]
fn overlapping_air_is_compared_and_conflicts_with_nonair() {
    let a = region(
        "air",
        [0, 0, 0],
        [1, 1, 1],
        vec![state("minecraft:air")],
        &[0],
    );
    let b = region(
        "stone",
        [0, 0, 0],
        [1, 1, 1],
        vec![state("minecraft:air"), state("minecraft:stone")],
        &[1],
    );
    let err = build_structure(&schematic(vec![a, b])).unwrap_err();
    assert_eq!(err.stage, Stage::Merge);
    assert_eq!(err.position, Some([0, 0, 0]));
}

#[test]
fn overlapping_identical_blocks_merge_without_duplicate_records() {
    let a = region(
        "one",
        [0, 0, 0],
        [1, 1, 1],
        vec![state("minecraft:stone")],
        &[0],
    );
    let b = region(
        "two",
        [0, 0, 0],
        [1, 1, 1],
        vec![state("minecraft:stone")],
        &[0],
    );
    let out = build_structure(&schematic(vec![a, b])).unwrap();
    assert_eq!(out.blocks.len(), 1);
}

#[test]
fn block_entity_coordinates_are_removed_before_overlap_comparison() {
    let mut a = region(
        "one",
        [10, 0, 0],
        [1, 1, 1],
        vec![state("minecraft:chest")],
        &[0],
    );
    let mut b = region(
        "two",
        [9, 0, 0],
        [2, 1, 1],
        vec![state("minecraft:air"), state("minecraft:chest")],
        &[0, 1],
    );
    let block_entity = || {
        compound([
            ("id".into(), NbtValue::String("minecraft:chest".into())),
            ("x".into(), NbtValue::Int(0)),
            ("y".into(), NbtValue::Int(0)),
            ("z".into(), NbtValue::Int(0)),
            ("CustomName".into(), NbtValue::String("箱".into())),
            ("Items".into(), NbtValue::List(vec![])),
            ("typedLong".into(), NbtValue::Long(9_007_199_254_740_993)),
            (
                "typedBytes".into(),
                NbtValue::ByteArray(fastnbt::ByteArray::new(vec![1, -2])),
            ),
        ])
    };
    a.block_entities.push(block_entity());
    let mut b_entity = block_entity();
    let NbtValue::Compound(b_entity) = &mut b_entity else {
        panic!("compound expected")
    };
    b_entity.insert("x".into(), NbtValue::Int(1));
    b.block_entities.push(NbtValue::Compound(b_entity.clone()));
    let out = build_structure(&schematic(vec![a, b])).unwrap();
    let be = out.blocks[0].nbt.as_ref().unwrap();
    let NbtValue::Compound(be) = be else {
        panic!("compound NBT expected")
    };
    assert!(!be.contains_key("x") && !be.contains_key("y") && !be.contains_key("z"));
    assert_eq!(be.get("CustomName"), Some(&NbtValue::String("箱".into())));
    assert_eq!(
        be.get("typedLong"),
        Some(&NbtValue::Long(9_007_199_254_740_993))
    );
    assert_eq!(
        be.get("typedBytes"),
        Some(&NbtValue::ByteArray(fastnbt::ByteArray::new(vec![1, -2])))
    );
}

#[test]
fn entity_positions_and_integer_coordinate_fields_shift_and_uuid_deduplicates() {
    let mut r = region(
        "entity",
        [5, 0, 0],
        [2, 1, 1],
        vec![state("minecraft:air")],
        &[0, 0],
    );
    let entity = || {
        compound([
            ("id".into(), NbtValue::String("minecraft:item_frame".into())),
            (
                "Pos".into(),
                NbtValue::List(vec![
                    NbtValue::Double(0.5),
                    NbtValue::Double(0.0),
                    NbtValue::Double(0.25),
                ]),
            ),
            ("TileX".into(), NbtValue::Int(0)),
            ("TileY".into(), NbtValue::Int(0)),
            ("TileZ".into(), NbtValue::Int(0)),
            ("SleepingX".into(), NbtValue::Int(1)),
            ("SleepingY".into(), NbtValue::Int(0)),
            ("SleepingZ".into(), NbtValue::Int(0)),
            (
                "UUID".into(),
                NbtValue::IntArray(fastnbt::IntArray::new(vec![1, 2, 3, 4])),
            ),
        ])
    };
    r.entities.extend([entity(), entity()]);
    let out = build_structure(&schematic(vec![r])).unwrap();
    assert_eq!(out.entities.len(), 1);
    assert_eq!(out.entities[0].pos, [0.5, 0.0, 0.25]);
    assert_eq!(out.entities[0].block_pos, [0, 0, 0]);
    let NbtValue::Compound(nbt) = &out.entities[0].nbt else {
        panic!("compound expected")
    };
    assert_eq!(nbt.get("TileX"), Some(&NbtValue::Int(0)));
    assert_eq!(nbt.get("SleepingX"), Some(&NbtValue::Int(1)));
}

#[test]
fn negative_size_entity_pos_uses_region_position_delta() {
    let mut r = region(
        "negative-entity",
        [10, 20, 30],
        [-3, 2, -4],
        vec![state("minecraft:air")],
        &[0; 24],
    );
    r.entities.push(compound([
        ("id".into(), NbtValue::String("minecraft:mob".into())),
        (
            "Pos".into(),
            NbtValue::List(vec![
                NbtValue::Double(-1.5),
                NbtValue::Double(0.5),
                NbtValue::Double(-2.5),
            ]),
        ),
    ]));
    let out = build_structure(&schematic(vec![r])).unwrap();
    assert_eq!(out.size, [3, 2, 4]);
    assert_eq!(out.entities[0].pos, [0.5, 0.5, 0.5]);
}

#[test]
fn entity_delta_uses_region_position_when_global_min_is_elsewhere() {
    let anchor = region(
        "anchor",
        [0, 0, 0],
        [1, 1, 1],
        vec![state("minecraft:air")],
        &[0],
    );
    let mut remote = region(
        "remote",
        [10, 0, 0],
        [1, 1, 1],
        vec![state("minecraft:air")],
        &[0],
    );
    remote.entities.push(compound([
        ("id".into(), NbtValue::String("minecraft:mob".into())),
        (
            "Pos".into(),
            NbtValue::List(vec![
                NbtValue::Double(0.5),
                NbtValue::Double(0.0),
                NbtValue::Double(0.0),
            ]),
        ),
        ("TileX".into(), NbtValue::Int(0)),
        ("TileY".into(), NbtValue::Int(0)),
        ("TileZ".into(), NbtValue::Int(0)),
    ]));
    let out = build_structure(&schematic(vec![anchor, remote])).unwrap();
    assert_eq!(out.entities[0].pos, [10.5, 0.0, 0.0]);
    assert_eq!(out.entities[0].block_pos, [10, 0, 0]);
}

#[test]
fn modern_blockpos_fields_shift_and_codec_shaped_leash_refs_stay_typed() {
    let mut r = region(
        "modern",
        [10, 20, 30],
        [-3, 2, -4],
        vec![state("minecraft:air")],
        &[0; 24],
    );
    // Synthetic mod entity: its typed BlockPos anchor is authoritative even though the
    // converter cannot infer the entity's concrete type from its identifier.
    r.entities.push(compound([
        (
            "id".into(),
            NbtValue::String("example:attached_entity".into()),
        ),
        (
            "Pos".into(),
            NbtValue::List(vec![
                NbtValue::Double(-1.5),
                NbtValue::Double(0.5),
                NbtValue::Double(-2.5),
            ]),
        ),
        (
            "block_pos".into(),
            NbtValue::IntArray(fastnbt::IntArray::new(vec![0, 0, 0])),
        ),
        (
            "home_pos".into(),
            NbtValue::IntArray(fastnbt::IntArray::new(vec![1, 2, 3])),
        ),
        (
            "leash".into(),
            NbtValue::IntArray(fastnbt::IntArray::new(vec![4, 5, 6])),
        ),
        ("TileX".into(), NbtValue::Int(0)),
        ("TileY".into(), NbtValue::Int(0)),
        ("TileZ".into(), NbtValue::Int(0)),
        ("SleepingX".into(), NbtValue::Int(3)),
        ("SleepingY".into(), NbtValue::Int(4)),
        ("SleepingZ".into(), NbtValue::Int(5)),
        (
            "custom".into(),
            NbtValue::Compound(HashMap::from([
                ("text".into(), NbtValue::String("雪だるま 🐼".into())),
                (
                    "components".into(),
                    NbtValue::List(vec![
                        NbtValue::Byte(1),
                        NbtValue::Long(9_007_199_254_740_993),
                    ]),
                ),
            ])),
        ),
    ]));
    let uuid_leash = compound([
        ("id".into(), NbtValue::String("mod:uuid_leash".into())),
        (
            "Pos".into(),
            NbtValue::List(vec![
                NbtValue::Double(0.0),
                NbtValue::Double(0.0),
                NbtValue::Double(0.0),
            ]),
        ),
        (
            "leash".into(),
            NbtValue::IntArray(fastnbt::IntArray::new(vec![1, 2, 3, 4])),
        ),
    ]);
    let compound_leash = compound([
        ("id".into(), NbtValue::String("mod:entity_ref_leash".into())),
        (
            "Pos".into(),
            NbtValue::List(vec![
                NbtValue::Double(0.0),
                NbtValue::Double(0.0),
                NbtValue::Double(0.0),
            ]),
        ),
        (
            "leash".into(),
            NbtValue::Compound(HashMap::from([(
                "UUID".into(),
                NbtValue::IntArray(fastnbt::IntArray::new(vec![1, 2, 3, 4])),
            )])),
        ),
    ]);
    r.entities.push(uuid_leash.clone());
    r.entities.push(compound_leash.clone());
    let out = build_structure(&schematic(vec![r])).unwrap();
    let entity=out.entities.iter().find(|e| matches!(&e.nbt,NbtValue::Compound(c) if c.get("id")==Some(&NbtValue::String("example:attached_entity".into())))).unwrap();
    assert_eq!(entity.block_pos, [2, 0, 3]);
    let NbtValue::Compound(nbt) = &entity.nbt else {
        panic!("compound expected")
    };
    assert_eq!(
        nbt.get("block_pos"),
        Some(&NbtValue::IntArray(fastnbt::IntArray::new(vec![2, 0, 3])))
    );
    assert_eq!(
        nbt.get("home_pos"),
        Some(&NbtValue::IntArray(fastnbt::IntArray::new(vec![3, 2, 6])))
    );
    assert_eq!(
        nbt.get("leash"),
        Some(&NbtValue::IntArray(fastnbt::IntArray::new(vec![6, 5, 9])))
    );
    assert_eq!(nbt.get("SleepingX"), Some(&NbtValue::Int(5)));
    assert_eq!(
        nbt.get("custom"),
        Some(&NbtValue::Compound(HashMap::from([
            ("text".into(), NbtValue::String("雪だるま 🐼".into())),
            (
                "components".into(),
                NbtValue::List(vec![
                    NbtValue::Byte(1),
                    NbtValue::Long(9_007_199_254_740_993)
                ])
            ),
        ])))
    );
    let NbtValue::Compound(uuid_data)=&out.entities.iter().find(|e| matches!(&e.nbt,NbtValue::Compound(c) if c.get("id")==Some(&NbtValue::String("mod:uuid_leash".into())))).unwrap().nbt else {panic!("compound expected")};
    assert_eq!(
        uuid_data.get("leash"),
        Some(&NbtValue::IntArray(fastnbt::IntArray::new(vec![
            1, 2, 3, 4
        ])))
    );
    let NbtValue::Compound(ref_data)=&out.entities.iter().find(|e| matches!(&e.nbt,NbtValue::Compound(c) if c.get("id")==Some(&NbtValue::String("mod:entity_ref_leash".into())))).unwrap().nbt else {panic!("compound expected")};
    assert_eq!(
        ref_data.get("leash"),
        Some(&NbtValue::Compound(HashMap::from([(
            "UUID".into(),
            NbtValue::IntArray(fastnbt::IntArray::new(vec![1, 2, 3, 4]))
        )])))
    );
}

#[test]
fn modern_anchor_conflicts_and_invalid_blockpos_shapes_fail_with_field_name() {
    let make = |block_pos: NbtValue, tile_x: i32| {
        let mut r = region(
            "bad-anchor",
            [10, 0, 0],
            [-2, 1, 1],
            vec![state("minecraft:air")],
            &[0, 0],
        );
        r.entities.push(compound([
            ("id".into(), NbtValue::String("mod:entity".into())),
            (
                "Pos".into(),
                NbtValue::List(vec![
                    NbtValue::Double(-0.5),
                    NbtValue::Double(0.0),
                    NbtValue::Double(0.0),
                ]),
            ),
            ("block_pos".into(), block_pos),
            ("TileX".into(), NbtValue::Int(tile_x)),
            ("TileY".into(), NbtValue::Int(0)),
            ("TileZ".into(), NbtValue::Int(0)),
        ]));
        r
    };
    let mismatch = make(NbtValue::IntArray(fastnbt::IntArray::new(vec![0, 0, 0])), 1);
    assert!(build_structure(&schematic(vec![mismatch]))
        .unwrap_err()
        .message
        .contains("block_pos and TileXYZ"));
    let bad_len = make(NbtValue::IntArray(fastnbt::IntArray::new(vec![1, 2])), 0);
    let err = build_structure(&schematic(vec![bad_len])).unwrap_err();
    assert!(err.message.contains("block_pos"));
    let bad_type = make(
        NbtValue::List(vec![NbtValue::Int(1), NbtValue::Int(2), NbtValue::Int(3)]),
        0,
    );
    let err = build_structure(&schematic(vec![bad_type])).unwrap_err();
    assert!(err.message.contains("block_pos"));
}

#[test]
fn same_entity_uuid_with_different_payload_fails_and_uuid_less_entities_are_kept() {
    let mut r = region(
        "entities",
        [0, 0, 0],
        [1, 1, 1],
        vec![state("minecraft:air")],
        &[0],
    );
    let make = |name: Option<&str>, uuid: bool| {
        let mut fields = vec![
            ("id".into(), NbtValue::String("minecraft:item".into())),
            (
                "Pos".into(),
                NbtValue::List(vec![
                    NbtValue::Double(0.0),
                    NbtValue::Double(0.0),
                    NbtValue::Double(0.0),
                ]),
            ),
        ];
        if let Some(name) = name {
            fields.push(("CustomName".into(), NbtValue::String(name.into())));
        }
        if uuid {
            fields.push((
                "UUID".into(),
                NbtValue::IntArray(fastnbt::IntArray::new(vec![1, 2, 3, 4])),
            ));
        }
        compound(fields)
    };
    r.entities
        .extend([make(Some("a"), true), make(Some("b"), true)]);
    let err = build_structure(&schematic(vec![r.clone()])).unwrap_err();
    assert_eq!(err.stage, Stage::Merge);
    r.entities = vec![make(None, false), make(None, false)];
    assert_eq!(
        build_structure(&schematic(vec![r])).unwrap().entities.len(),
        2
    );
}

#[test]
fn rejects_bad_entity_coordinate_fields_and_overall_i32_size_overflow() {
    let mut r = region(
        "bad-entity",
        [0, 0, 0],
        [1, 1, 1],
        vec![state("minecraft:air")],
        &[0],
    );
    r.entities.push(compound([
        ("id".into(), NbtValue::String("minecraft:item".into())),
        (
            "Pos".into(),
            NbtValue::List(vec![
                NbtValue::Double(0.0),
                NbtValue::Double(0.0),
                NbtValue::Double(0.0),
            ]),
        ),
        ("SleepingX".into(), NbtValue::String("bad".into())),
    ]));
    assert_eq!(
        build_structure(&schematic(vec![r])).unwrap_err().stage,
        Stage::Normalize
    );
    let left = region(
        "left",
        [i32::MIN, 0, 0],
        [1, 1, 1],
        vec![state("minecraft:stone")],
        &[0],
    );
    let right = region(
        "right",
        [i32::MAX, 0, 0],
        [1, 1, 1],
        vec![state("minecraft:stone")],
        &[0],
    );
    assert_eq!(
        build_structure(&schematic(vec![left, right]))
            .unwrap_err()
            .stage,
        Stage::Normalize
    );
}

#[test]
fn structure_writer_round_trips_gzip_nbt_and_omits_missing_data_version() {
    let mut parsed = schematic(vec![region(
        "a",
        [0, 0, 0],
        [1, 1, 1],
        vec![state("minecraft:stone")],
        &[0],
    )]);
    parsed.data_version = None;
    let built = build_structure_with_warnings(&parsed).unwrap();
    let mut bytes = Vec::new();
    write_structure(&mut bytes, &built.structure).unwrap();
    verify_structure(bytes.as_slice(), &built.structure).unwrap();
    assert_eq!(built.structure.data_version, None);
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    assert!(verify_structure(bytes.as_slice(), &built.structure).is_err());
}
