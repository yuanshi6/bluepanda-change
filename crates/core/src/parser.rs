use std::io::Read;

use fastnbt::Value;
use flate2::read::GzDecoder;

use crate::types::{
    BlockState, ConversionError, ConversionWarning, NbtValue, ParsedSchematic, Region, Stage,
};

/// Decompress and parse a supported legacy Litematica 6.1 or 7.1 schematic.
/// The input must be GZIP-compressed NBT as produced by Litematica.
pub fn parse_litematic<R: Read>(reader: R) -> Result<ParsedSchematic, ConversionError> {
    let mut decoder = GzDecoder::new(reader);
    let mut bytes = Vec::new();
    decoder
        .read_to_end(&mut bytes)
        .map_err(|e| ConversionError::new(Stage::Decompress, format!("invalid GZIP input: {e}")))?;

    let root: Value = fastnbt::from_bytes(&bytes)
        .map_err(|e| ConversionError::new(Stage::Parse, format!("invalid NBT: {e}")))?;
    let root = compound(&root, "root", Stage::Validate)?;
    let version = int_field(root, "Version", "root", Stage::Validate)?;
    let sub_version = int_field(root, "SubVersion", "root", Stage::Validate)?;
    if !matches!(version, 6 | 7) || sub_version != 1 {
        return Err(ConversionError::new(
            Stage::Validate,
            format!("unsupported Litematica format Version={version}, SubVersion={sub_version}; supported formats are 6.1 and 7.1"),
        ));
    }
    let data_version = match root.get("MinecraftDataVersion") {
        None => None,
        Some(Value::Int(value)) => Some(*value),
        Some(other) => {
            return Err(wrong_type(
                "MinecraftDataVersion",
                "Int",
                other,
                "root",
                Stage::Validate,
            ))
        }
    };
    let regions_value = root
        .get("Regions")
        .ok_or_else(|| ConversionError::new(Stage::Validate, "missing Regions compound"))?;
    let regions_compound = compound(regions_value, "Regions", Stage::Validate)?;
    let mut regions = Vec::with_capacity(regions_compound.len());
    let mut warnings = Vec::new();
    if root.contains_key("Metadata") {
        warnings.push(ConversionWarning::new(
            "Litematica metadata is not represented in Minecraft Structure NBT",
        ));
    }
    // NBT compounds have no semantic ordering. Sort names so merged palettes, diagnostics, and
    // output bytes are repeatable across runs despite HashMap iteration randomization.
    let mut ordered_regions: Vec<_> = regions_compound.iter().collect();
    ordered_regions.sort_by(|(left, _), (right, _)| left.cmp(right));
    for (name, value) in ordered_regions {
        let region = parse_region(name, value)?;
        if region_has_pending_ticks(value) {
            warnings.push(
                ConversionWarning::new(
                    "pending block or fluid ticks are not represented in Minecraft Structure NBT",
                )
                .in_region(name),
            );
        }
        regions.push(region);
    }
    Ok(ParsedSchematic {
        data_version,
        regions,
        warnings,
    })
}

fn parse_region(name: &str, value: &Value) -> Result<Region, ConversionError> {
    let c = compound(value, "region", Stage::Validate).map_err(|e| e.in_region(name))?;
    let position = vec3_field(c, "Position", name)?;
    let size = vec3_field(c, "Size", name)?;
    if size.iter().any(|v| *v == 0) {
        return Err(ConversionError::new(
            Stage::Validate,
            "region size components must be non-zero",
        )
        .in_region(name));
    }
    for axis in 0..3 {
        let extent = i64::from(size[axis]).abs();
        let endpoint = i64::from(position[axis]).checked_add(if size[axis] < 0 {
            -extent + 1
        } else {
            extent - 1
        });
        if endpoint.is_none_or(|v| i32::try_from(v).is_err()) {
            return Err(ConversionError::new(
                Stage::Validate,
                "region coordinate extent overflows i32",
            )
            .in_region(name));
        }
    }

    let palette_value = c.get("BlockStatePalette").ok_or_else(|| {
        ConversionError::new(Stage::Validate, "missing BlockStatePalette").in_region(name)
    })?;
    let palette_list = list(palette_value, "BlockStatePalette", name)?;
    if palette_list.is_empty() {
        return Err(
            ConversionError::new(Stage::Validate, "BlockStatePalette cannot be empty")
                .in_region(name),
        );
    }
    let mut palette = Vec::with_capacity(palette_list.len());
    for (i, state) in palette_list.iter().enumerate() {
        let state = compound(state, "BlockStatePalette entry", Stage::Validate)
            .map_err(|e| e.in_region(name))?;
        let state_name = string_field(state, "Name", name)?;
        let properties = match state.get("Properties") {
            None => Default::default(),
            Some(Value::Compound(p)) => {
                let mut props = std::collections::BTreeMap::new();
                for (key, value) in p {
                    match value {
                        Value::String(v) => {
                            props.insert(key.clone(), v.clone());
                        }
                        other => {
                            return Err(wrong_type(
                                &format!("BlockStatePalette[{i}].Properties.{key}"),
                                "String",
                                other,
                                name,
                                Stage::Validate,
                            ))
                        }
                    }
                }
                props
            }
            Some(other) => {
                return Err(wrong_type(
                    "Properties",
                    "Compound",
                    other,
                    name,
                    Stage::Validate,
                ))
            }
        };
        palette.push(BlockState {
            name: state_name.to_owned(),
            properties,
        });
    }

    let volume = region_volume(size).ok_or_else(|| {
        ConversionError::new(Stage::Validate, "region volume overflows addressable size")
            .in_region(name)
    })?;
    let bits = bits_per_block(palette.len());
    let expected_longs = volume
        .checked_mul(bits)
        .and_then(|n| n.checked_add(63))
        .map(|n| n / 64)
        .ok_or_else(|| {
            ConversionError::new(Stage::Validate, "packed block-state length overflows")
                .in_region(name)
        })?;
    let packed_value = c.get("BlockStates").ok_or_else(|| {
        ConversionError::new(Stage::Validate, "missing BlockStates long array").in_region(name)
    })?;
    let packed_states = match packed_value {
        Value::LongArray(a) => a.to_vec(),
        other => {
            return Err(wrong_type(
                "BlockStates",
                "LongArray",
                other,
                name,
                Stage::Validate,
            ))
        }
    };
    if packed_states.len() != expected_longs {
        return Err(ConversionError::new(Stage::Validate, format!("BlockStates has {} longs; expected {expected_longs} for {volume} blocks and {bits} bits per entry", packed_states.len())).in_region(name));
    }
    let region = Region {
        name: name.to_owned(),
        position,
        size,
        palette,
        packed_states,
        block_entities: optional_compound_list(c, "TileEntities", name)?,
        entities: optional_compound_list(c, "Entities", name)?,
    };
    for index in 0..volume {
        let palette_index = decode_palette_index(&region, index)?;
        if palette_index >= region.palette.len() {
            let pos = linear_index_to_xyz(index, size);
            return Err(ConversionError::new(
                Stage::Decode,
                format!(
                    "palette index {palette_index} is outside palette of length {}",
                    region.palette.len()
                ),
            )
            .in_region(name)
            .at(pos));
        }
    }
    Ok(region)
}

/// Number of cells in a region, interpreting signed size components as extents.
pub fn region_volume(size: [i32; 3]) -> Option<usize> {
    size.into_iter().try_fold(1usize, |n, v| {
        n.checked_mul(usize::try_from(i64::from(v).abs()).ok()?)
    })
}

/// Litematica stores compact values as one continuous bit stream; entries may straddle longs.
pub fn decode_palette_index(
    region: &Region,
    linear_index: usize,
) -> Result<usize, ConversionError> {
    let volume = region_volume(region.size).ok_or_else(|| {
        ConversionError::new(Stage::Decode, "region volume overflow").in_region(&region.name)
    })?;
    if linear_index >= volume {
        return Err(ConversionError::new(
            Stage::Decode,
            format!("block index {linear_index} is outside region volume {volume}"),
        )
        .in_region(&region.name));
    }
    let bits = bits_per_block(region.palette.len());
    let bit_start = linear_index.checked_mul(bits).ok_or_else(|| {
        ConversionError::new(Stage::Decode, "packed bit offset overflow").in_region(&region.name)
    })?;
    let word_index = bit_start / 64;
    let bit_offset = bit_start % 64;
    let first = *region.packed_states.get(word_index).ok_or_else(|| {
        ConversionError::new(Stage::Decode, "BlockStates ended before region volume")
            .in_region(&region.name)
    })? as u64;
    let mut value = first >> bit_offset;
    if bit_offset + bits > 64 {
        let next = *region.packed_states.get(word_index + 1).ok_or_else(|| {
            ConversionError::new(Stage::Decode, "BlockStates ended inside packed value")
                .in_region(&region.name)
        })? as u64;
        value |= next << (64 - bit_offset);
    }
    let mask = if bits == 64 {
        u64::MAX
    } else {
        (1u64 << bits) - 1
    };
    Ok((value & mask) as usize)
}

/// Decode and validate every palette reference in a region.
pub fn decode_all_palette_indices(region: &Region) -> Result<Vec<usize>, ConversionError> {
    let volume = region_volume(region.size).ok_or_else(|| {
        ConversionError::new(Stage::Decode, "region volume overflow").in_region(&region.name)
    })?;
    let mut decoded = Vec::with_capacity(volume);
    for index in 0..volume {
        let palette_index = decode_palette_index(region, index)?;
        if palette_index >= region.palette.len() {
            return Err(ConversionError::new(
                Stage::Decode,
                format!(
                    "palette index {palette_index} is outside palette of length {}",
                    region.palette.len()
                ),
            )
            .in_region(&region.name)
            .at(linear_index_to_xyz(index, region.size)));
        }
        decoded.push(palette_index);
    }
    Ok(decoded)
}

pub fn bits_per_block(palette_len: usize) -> usize {
    (usize::BITS - palette_len.saturating_sub(1).leading_zeros()).max(2) as usize
}

/// The packed stream order is X fastest, then Z, then Y (index = y*X*Z + z*X + x).
pub fn linear_index_to_xyz(index: usize, signed_size: [i32; 3]) -> [i32; 3] {
    let sx = i64::from(signed_size[0]).abs() as usize;
    let sz = i64::from(signed_size[2]).abs() as usize;
    let layer = sx.saturating_mul(sz);
    let y = index / layer;
    let rem = index % layer;
    let x = rem % sx;
    let z = rem / sx;
    [x as i32, y as i32, z as i32]
}

fn region_has_pending_ticks(value: &Value) -> bool {
    let Value::Compound(c) = value else {
        return false;
    };
    ["PendingBlockTicks", "PendingFluidTicks"]
        .iter()
        .any(|key| c.contains_key(*key))
}

fn compound<'a>(
    v: &'a Value,
    field: &str,
    stage: Stage,
) -> Result<&'a std::collections::HashMap<String, Value>, ConversionError> {
    match v {
        Value::Compound(c) => Ok(c),
        other => Err(wrong_type(field, "Compound", other, "", stage)),
    }
}

fn list<'a>(v: &'a Value, field: &str, region: &str) -> Result<&'a [Value], ConversionError> {
    match v {
        Value::List(values) => Ok(values),
        other => Err(wrong_type(field, "List", other, region, Stage::Validate)),
    }
}

fn int_field(
    c: &std::collections::HashMap<String, Value>,
    key: &str,
    where_: &str,
    stage: Stage,
) -> Result<i32, ConversionError> {
    match c.get(key) {
        Some(Value::Int(v)) => Ok(*v),
        Some(other) => Err(wrong_type(key, "Int", other, where_, stage)),
        None => Err(ConversionError::new(stage, format!("missing {key}")).in_region(where_)),
    }
}

fn string_field<'a>(
    c: &'a std::collections::HashMap<String, Value>,
    key: &str,
    where_: &str,
) -> Result<&'a str, ConversionError> {
    match c.get(key) {
        Some(Value::String(v)) => Ok(v),
        Some(other) => Err(wrong_type(key, "String", other, where_, Stage::Validate)),
        None => {
            Err(ConversionError::new(Stage::Validate, format!("missing {key}")).in_region(where_))
        }
    }
}

fn vec3_field(
    c: &std::collections::HashMap<String, Value>,
    key: &str,
    region: &str,
) -> Result<[i32; 3], ConversionError> {
    let v = c.get(key).ok_or_else(|| {
        ConversionError::new(Stage::Validate, format!("missing {key}")).in_region(region)
    })?;
    let c = match v {
        Value::Compound(c) => c,
        other => {
            return Err(wrong_type(
                key,
                "Compound {x:Int,y:Int,z:Int}",
                other,
                region,
                Stage::Validate,
            ))
        }
    };
    Ok([
        int_field(c, "x", region, Stage::Validate)?,
        int_field(c, "y", region, Stage::Validate)?,
        int_field(c, "z", region, Stage::Validate)?,
    ])
}

fn optional_compound_list(
    c: &std::collections::HashMap<String, Value>,
    key: &str,
    region: &str,
) -> Result<Vec<NbtValue>, ConversionError> {
    let Some(value) = c.get(key) else {
        return Ok(Vec::new());
    };
    let entries = list(value, key, region)?;
    entries
        .iter()
        .enumerate()
        .map(|(index, v)| match v {
            Value::Compound(_) => Ok(v.clone()),
            other => Err(wrong_type(
                &format!("{key}[{index}]"),
                "Compound",
                other,
                region,
                Stage::Validate,
            )),
        })
        .collect()
}

fn wrong_type(
    field: &str,
    expected: &str,
    got: &Value,
    region: &str,
    stage: Stage,
) -> ConversionError {
    let type_name = match got {
        Value::Byte(_) => "Byte",
        Value::Short(_) => "Short",
        Value::Int(_) => "Int",
        Value::Long(_) => "Long",
        Value::Float(_) => "Float",
        Value::Double(_) => "Double",
        Value::String(_) => "String",
        Value::ByteArray(_) => "ByteArray",
        Value::IntArray(_) => "IntArray",
        Value::LongArray(_) => "LongArray",
        Value::List(_) => "List",
        Value::Compound(_) => "Compound",
    };
    let err = ConversionError::new(
        stage,
        format!("{field} must be {expected}, got {type_name}"),
    );
    if region.is_empty() {
        err
    } else {
        err.in_region(region)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{write::GzEncoder, Compression};
    use std::{collections::HashMap, io::Write};

    fn comp(entries: impl IntoIterator<Item = (String, Value)>) -> Value {
        Value::Compound(entries.into_iter().collect())
    }
    fn point(x: i32, y: i32, z: i32) -> Value {
        comp([
            ("x".into(), Value::Int(x)),
            ("y".into(), Value::Int(y)),
            ("z".into(), Value::Int(z)),
        ])
    }
    fn state(name: &str) -> Value {
        comp([("Name".into(), Value::String(name.into()))])
    }
    fn packed(values: &[usize], bits: usize) -> Vec<i64> {
        let mut words = vec![0u64; (values.len() * bits + 63) / 64];
        for (idx, value) in values.iter().copied().enumerate() {
            let bit = idx * bits;
            let word = bit / 64;
            let shift = bit % 64;
            words[word] |= (value as u64) << shift;
            if shift + bits > 64 {
                words[word + 1] |= (value as u64) >> (64 - shift);
            }
        }
        words.into_iter().map(|v| v as i64).collect()
    }
    fn input(
        size: [i32; 3],
        palette: Vec<Value>,
        blocks: Vec<i64>,
        version: Option<i32>,
    ) -> Vec<u8> {
        let region = comp([
            ("Position".into(), point(-3, 4, 9)),
            ("Size".into(), point(size[0], size[1], size[2])),
            ("BlockStatePalette".into(), Value::List(palette)),
            (
                "BlockStates".into(),
                Value::LongArray(fastnbt::LongArray::new(blocks)),
            ),
            ("Entities".into(), Value::List(Vec::new())),
            ("TileEntities".into(), Value::List(Vec::new())),
        ]);
        let mut root = HashMap::from([
            ("Version".into(), Value::Int(6)),
            ("SubVersion".into(), Value::Int(1)),
            ("Regions".into(), comp([("region".into(), region)])),
        ]);
        if let Some(v) = version {
            root.insert("MinecraftDataVersion".into(), Value::Int(v));
        }
        let nbt = fastnbt::to_bytes(&Value::Compound(root)).unwrap();
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        gz.write_all(&nbt).unwrap();
        gz.finish().unwrap()
    }
    fn raw_value(bytes: &[u8]) -> Value {
        use std::io::Read;
        let mut raw = Vec::new();
        flate2::read::GzDecoder::new(bytes)
            .read_to_end(&mut raw)
            .unwrap();
        fastnbt::from_bytes(&raw).unwrap()
    }
    fn gzip_value(value: Value) -> Vec<u8> {
        let nbt = fastnbt::to_bytes(&value).unwrap();
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        gz.write_all(&nbt).unwrap();
        gz.finish().unwrap()
    }

    #[test]
    fn parses_missing_data_version_and_all_air_region() {
        let bytes = input([1, 1, 1], vec![state("minecraft:air")], vec![0], None);
        let parsed = parse_litematic(bytes.as_slice()).unwrap();
        assert_eq!(parsed.data_version, None);
        assert_eq!(parsed.regions[0].palette[0].name, "minecraft:air");
        assert_eq!(decode_palette_index(&parsed.regions[0], 0).unwrap(), 0);
    }

    #[test]
    fn passes_data_version_through_without_coercion() {
        let bytes = input(
            [1, 1, 1],
            vec![state("minecraft:stone")],
            vec![0],
            Some(3700),
        );
        assert_eq!(
            parse_litematic(bytes.as_slice()).unwrap().data_version,
            Some(3700)
        );
        let mut root = HashMap::from([
            ("Version".into(), Value::Int(6)),
            ("SubVersion".into(), Value::Int(1)),
            ("MinecraftDataVersion".into(), Value::Long(3700)),
            ("Regions".into(), comp(Vec::<(String, Value)>::new())),
        ]);
        let nbt = fastnbt::to_bytes(&Value::Compound(std::mem::take(&mut root))).unwrap();
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        gz.write_all(&nbt).unwrap();
        let bytes = gz.finish().unwrap();
        assert!(parse_litematic(bytes.as_slice())
            .unwrap_err()
            .message
            .contains("must be Int"));
    }

    #[test]
    fn decodes_values_across_long_boundary() {
        let palette = (0..5)
            .map(|n| state(&format!("test:block_{n}")))
            .collect::<Vec<_>>();
        let expected = (0..32).map(|n| n % 5).collect::<Vec<_>>();
        let bytes = input([32, 1, 1], palette, packed(&expected, 3), Some(3700));
        let parsed = parse_litematic(bytes.as_slice()).unwrap();
        for (index, value) in expected.iter().enumerate() {
            assert_eq!(
                decode_palette_index(&parsed.regions[0], index).unwrap(),
                *value
            );
        }
    }

    #[test]
    fn rejects_zero_extent_and_wrong_array_length() {
        let zero = input([0, 1, 1], vec![state("minecraft:air")], vec![], None);
        assert!(parse_litematic(zero.as_slice())
            .unwrap_err()
            .message
            .contains("non-zero"));
        let too_many = input([1, 1, 1], vec![state("minecraft:air")], vec![0, 0], None);
        assert!(parse_litematic(too_many.as_slice())
            .unwrap_err()
            .message
            .contains("expected 1"));
        let too_few = input([1, 1, 1], vec![state("minecraft:air")], vec![], None);
        assert!(parse_litematic(too_few.as_slice())
            .unwrap_err()
            .message
            .contains("expected 1"));
    }

    #[test]
    fn palette_bit_width_boundaries_and_max_ids() {
        let cases = [
            1usize, 2, 3, 4, 5, 8, 9, 16, 17, 32, 33, 64, 65, 128, 129, 256, 257,
        ];
        for palette_len in cases {
            let bits = bits_per_block(palette_len);
            let expected_bits = (usize::BITS - (palette_len - 1).leading_zeros()).max(2) as usize;
            assert_eq!(bits, expected_bits, "palette length {palette_len}");
            let max_id = palette_len - 1;
            let region = Region {
                name: "boundary".into(),
                position: [0; 3],
                size: [1, 1, 1],
                palette: (0..palette_len)
                    .map(|n| BlockState {
                        name: format!("test:{n}"),
                        properties: Default::default(),
                    })
                    .collect(),
                packed_states: packed(&[max_id], bits),
                block_entities: vec![],
                entities: vec![],
            };
            assert_eq!(
                decode_palette_index(&region, 0).unwrap(),
                max_id,
                "palette length {palette_len}"
            );
        }
    }

    #[test]
    fn rejects_out_of_range_palette_id_and_handles_signed_long() {
        let invalid = Region {
            name: "bad-index".into(),
            position: [0; 3],
            size: [1, 1, 1],
            palette: vec![BlockState {
                name: "minecraft:air".into(),
                properties: Default::default(),
            }],
            packed_states: packed(&[3], 2),
            block_entities: vec![],
            entities: vec![],
        };
        assert!(decode_all_palette_indices(&invalid)
            .unwrap_err()
            .message
            .contains("outside palette"));

        let words = packed(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 8], 6);
        assert!(words[0] < 0, "test data should set the Long sign bit");
        let signed = Region {
            name: "signed-long".into(),
            position: [0; 3],
            size: [11, 1, 1],
            palette: (0..33)
                .map(|n| BlockState {
                    name: format!("test:{n}"),
                    properties: Default::default(),
                })
                .collect(),
            packed_states: words,
            block_entities: vec![],
            entities: vec![],
        };
        assert_eq!(decode_palette_index(&signed, 10).unwrap(), 8);
    }

    #[test]
    fn rejects_unsupported_format_and_array_in_place_of_palette_list() {
        let valid = input([1, 1, 1], vec![state("minecraft:air")], vec![0], None);
        let mut root = raw_value(&valid);
        let Value::Compound(root_compound) = &mut root else {
            panic!("compound root")
        };
        root_compound.insert("Version".into(), Value::Int(8));
        assert!(parse_litematic(gzip_value(root).as_slice())
            .unwrap_err()
            .message
            .contains("unsupported Litematica"));

        let mut root = raw_value(&valid);
        let Value::Compound(root_compound) = &mut root else {
            panic!("compound root")
        };
        let Value::Compound(regions) = root_compound.get_mut("Regions").unwrap() else {
            panic!("regions compound")
        };
        let Value::Compound(region) = regions.get_mut("region").unwrap() else {
            panic!("region compound")
        };
        region.insert(
            "BlockStatePalette".into(),
            Value::IntArray(fastnbt::IntArray::new(vec![0])),
        );
        assert!(parse_litematic(gzip_value(root).as_slice())
            .unwrap_err()
            .message
            .contains("BlockStatePalette must be List"));
    }

    #[test]
    fn asymmetric_linear_indices_are_x_fastest() {
        let expected = [
            [0, 0, 0],
            [1, 0, 0],
            [0, 0, 1],
            [1, 0, 1],
            [0, 0, 2],
            [1, 0, 2],
        ];
        for (index, coordinate) in expected.into_iter().enumerate() {
            assert_eq!(linear_index_to_xyz(index, [2, 1, 3]), coordinate);
        }
    }

    #[test]
    fn accepts_v7_1_same_region_layout_and_keeps_data_version() {
        let valid = input([2, 1, 1], vec![state("minecraft:air")], vec![0], None);
        let mut root = raw_value(&valid);
        let Value::Compound(root_compound) = &mut root else {
            panic!("compound root")
        };
        root_compound.insert("Version".into(), Value::Int(7));
        root_compound.insert("SubVersion".into(), Value::Int(1));
        root_compound.insert("MinecraftDataVersion".into(), Value::Int(3955));
        let parsed = parse_litematic(gzip_value(root).as_slice()).unwrap();
        assert_eq!(parsed.data_version, Some(3955));
        assert_eq!(parsed.regions.len(), 1);
        assert_eq!(parsed.regions[0].position, [-3, 4, 9]);
        assert_eq!(parsed.regions[0].size, [2, 1, 1]);
        assert_eq!(parsed.regions[0].packed_states, vec![0]);
    }

    #[test]
    fn rejects_v8_and_unknown_subversions() {
        let valid = input([1, 1, 1], vec![state("minecraft:air")], vec![0], None);
        for (version, sub_version) in [(8, 1), (7, 2), (7, 0), (6, 2), (6, 0)] {
            let mut root = raw_value(&valid);
            let Value::Compound(root_compound) = &mut root else {
                panic!("compound root")
            };
            root_compound.insert("Version".into(), Value::Int(version));
            root_compound.insert("SubVersion".into(), Value::Int(sub_version));
            let err = parse_litematic(gzip_value(root).as_slice()).unwrap_err();
            assert_eq!(err.stage, Stage::Validate);
            assert!(err.message.contains("supported formats are 6.1 and 7.1"));
        }
    }

    #[test]
    fn v7_preserves_properties_and_typed_opaque_nbt_with_unicode_nul_and_arrays() {
        let valid = input([1, 1, 1], vec![state("minecraft:air")], vec![0], None);
        let mut root = raw_value(&valid);
        let Value::Compound(root_compound) = &mut root else {
            panic!("compound root")
        };
        root_compound.insert("Version".into(), Value::Int(7));
        root_compound.insert("SubVersion".into(), Value::Int(1));
        root_compound.insert("MinecraftDataVersion".into(), Value::Int(3955));
        let Value::Compound(regions) = root_compound.get_mut("Regions").unwrap() else {
            panic!("regions compound")
        };
        let Value::Compound(region) = regions.get_mut("region").unwrap() else {
            panic!("region compound")
        };

        let unusual = "汉字𠀀\0雪".to_string();
        let mut properties = HashMap::new();
        properties.insert("custom_property".into(), Value::String(unusual.clone()));
        region.insert(
            "BlockStatePalette".into(),
            Value::List(vec![comp([
                ("Name".into(), Value::String("mod:方块𠀀".into())),
                ("Properties".into(), Value::Compound(properties)),
            ])]),
        );

        let mut payload = HashMap::new();
        payload.insert("nestedText".into(), Value::String(unusual.clone()));
        payload.insert("byte".into(), Value::Byte(-7));
        payload.insert("short".into(), Value::Short(300));
        payload.insert("int".into(), Value::Int(70_000));
        payload.insert("long".into(), Value::Long(9_007_199_254_740_993));
        payload.insert("float".into(), Value::Float(1.25));
        payload.insert("double".into(), Value::Double(-0.125));
        payload.insert(
            "bytes".into(),
            Value::ByteArray(fastnbt::ByteArray::new(vec![0, -1, 2])),
        );
        payload.insert(
            "ints".into(),
            Value::IntArray(fastnbt::IntArray::new(vec![i32::MIN, 0, i32::MAX])),
        );
        payload.insert(
            "longs".into(),
            Value::LongArray(fastnbt::LongArray::new(vec![i64::MIN, 0, i64::MAX])),
        );
        let entity = Value::Compound(HashMap::from([
            ("id".into(), Value::String("minecraft:item_display".into())),
            (
                "Pos".into(),
                Value::List(vec![
                    Value::Double(0.25),
                    Value::Double(-1.5),
                    Value::Double(2.0),
                ]),
            ),
            ("components".into(), Value::Compound(payload.clone())),
        ]));
        region.insert("Entities".into(), Value::List(vec![entity.clone()]));
        let block_entity = Value::Compound(HashMap::from([
            ("id".into(), Value::String("mod:custom_block_entity".into())),
            ("x".into(), Value::Int(0)),
            ("y".into(), Value::Int(0)),
            ("z".into(), Value::Int(0)),
            ("opaque".into(), Value::Compound(payload)),
        ]));
        region.insert(
            "TileEntities".into(),
            Value::List(vec![block_entity.clone()]),
        );

        let parsed = parse_litematic(gzip_value(root).as_slice()).unwrap();
        let parsed_region = &parsed.regions[0];
        assert_eq!(parsed_region.palette[0].name, "mod:方块𠀀");
        assert_eq!(
            parsed_region.palette[0].properties.get("custom_property"),
            Some(&unusual)
        );
        assert_eq!(parsed_region.entities, vec![entity]);
        assert_eq!(parsed_region.block_entities, vec![block_entity]);
    }
}
