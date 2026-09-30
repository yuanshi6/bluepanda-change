use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Read};

use fastnbt::Value;
use flate2::bufread::GzDecoder;

use crate::types::{
    BlockState, ConversionError, ConversionWarning, NbtValue, ParsedSchematic, Region, Stage,
};

/// Decompress and parse a supported legacy Litematica 6.1 or 7.1 schematic.
/// The input must be GZIP-compressed NBT as produced by Litematica.
pub fn parse_litematic<R: Read>(reader: R) -> Result<ParsedSchematic, ConversionError> {
    let root = read_gzip_nbt_value(reader)?;
    parse_litematic_value(&root)
}

/// Parse a GZIP-compressed Litematica or Sponge schematic by its NBT layout.
pub fn parse_schematic<R: Read>(reader: R) -> Result<ParsedSchematic, ConversionError> {
    let root = read_gzip_nbt_value(reader)?;
    let root_compound = compound(&root, "root", Stage::Validate)?;
    if root_compound.contains_key("Schematic") {
        parse_schem_value(&root)
    } else if root_compound.contains_key("Regions") {
        parse_litematic_value(&root)
    } else if root_compound.contains_key("Version") {
        parse_schem_value(&root)
    } else {
        Err(ConversionError::new(
            Stage::Validate,
            "unrecognized schematic NBT layout",
        ))
    }
}

/// Parse a GZIP-compressed Sponge schematic (specification versions 1, 2, or 3).
pub fn parse_schem<R: Read>(reader: R) -> Result<ParsedSchematic, ConversionError> {
    let root = read_gzip_nbt_value(reader)?;
    parse_schem_value(&root)
}

fn read_gzip_nbt_value<R: Read>(reader: R) -> Result<Value, ConversionError> {
    let mut decoder = GzDecoder::new(BufReader::new(reader));
    let mut bytes = Vec::new();
    decoder
        .read_to_end(&mut bytes)
        .map_err(|e| ConversionError::new(Stage::Decompress, format!("invalid GZIP input: {e}")))?;
    let mut source = decoder.into_inner();
    if !source
        .fill_buf()
        .map_err(|e| {
            ConversionError::new(
                Stage::Decompress,
                format!("cannot validate GZIP trailer: {e}"),
            )
        })?
        .is_empty()
    {
        return Err(ConversionError::new(
            Stage::Decompress,
            "trailing bytes after GZIP-compressed NBT",
        ));
    }

    let mut cursor = std::io::Cursor::new(bytes.as_slice());
    let root = fastnbt::from_reader(&mut cursor)
        .map_err(|e| ConversionError::new(Stage::Parse, format!("invalid NBT: {e}")))?;
    if cursor.position() != bytes.len() as u64 {
        return Err(ConversionError::new(
            Stage::Parse,
            "trailing bytes after NBT root",
        ));
    }
    Ok(root)
}

fn parse_litematic_value(root: &Value) -> Result<ParsedSchematic, ConversionError> {
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
    warnings.push(ConversionWarning::new(
        "Litematica region names are not represented in Minecraft Structure NBT",
    ));
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

// An empty V3 schematic can declare enormous dimensions without carrying block data. Limit this
// special case so a tiny file cannot trigger billions of transform iterations.
const MAX_EMPTY_SPONGE_CELLS: usize = 16_777_216;

fn parse_schem_value(root: &Value) -> Result<ParsedSchematic, ConversionError> {
    let root = compound(root, "root", Stage::Validate)?;
    if let Some(schematic) = root.get("Schematic") {
        let schematic = compound(schematic, "Schematic", Stage::Validate)?;
        let version = int_field(schematic, "Version", "Schematic", Stage::Validate)?;
        if version != 3 {
            return Err(ConversionError::new(
                Stage::Validate,
                format!("nested Sponge Schematic supports Version=3, got {version}"),
            ));
        }
        return parse_schem_v3(schematic);
    }
    let version = int_field(root, "Version", "root", Stage::Validate)?;
    match version {
        1 | 2 => parse_schem_v1_v2(root, version),
        other => Err(ConversionError::new(
            Stage::Validate,
            format!(
                "unsupported Sponge schematic Version={other}; supported versions are 1, 2, and 3"
            ),
        )),
    }
}

fn parse_schem_v1_v2(
    root: &HashMap<String, Value>,
    version: i32,
) -> Result<ParsedSchematic, ConversionError> {
    let size = sponge_dimensions(root, "root")?;
    let volume = checked_sponge_volume(size)?;
    let palette = root.get("Palette").ok_or_else(|| {
        ConversionError::new(
            Stage::Validate,
            format!("Sponge Version={version} requires a local Palette; global palettes are unsupported"),
        )
    })?;
    let block_data = root
        .get("BlockData")
        .ok_or_else(|| ConversionError::new(Stage::Validate, "missing BlockData byte array"))?;
    let (palette, remap, bits) = parse_sponge_palette(palette, "Palette")?;
    validate_palette_max(root, &remap)?;
    let packed_states = decode_sponge_varints(block_data, volume, &remap, bits)?;
    let mut warnings = sponge_warnings(root, version == 2)?;
    let block_entity_field = if version == 1 {
        "TileEntities"
    } else {
        "BlockEntities"
    };
    let (block_entities, stripped_content_version) =
        sponge_block_entities(root.get(block_entity_field), false, version == 1)?;
    if stripped_content_version {
        warnings.push(ConversionWarning::new(
            "Sponge V1 TileEntities ContentVersion wrappers are not represented in Minecraft Structure NBT",
        ));
    }
    let entities = sponge_entities(root.get("Entities"), false)?;
    Ok(ParsedSchematic {
        data_version: sponge_data_version(root, version == 1)?,
        regions: vec![Region {
            name: "Sponge schematic".into(),
            position: [0; 3],
            size,
            palette,
            packed_states,
            block_entities,
            entities,
        }],
        warnings: std::mem::take(&mut warnings),
    })
}

fn parse_schem_v3(schematic: &HashMap<String, Value>) -> Result<ParsedSchematic, ConversionError> {
    let size = sponge_dimensions(schematic, "Schematic")?;
    let volume = checked_sponge_volume(size)?;
    let (palette, packed_states, block_entities) = match schematic.get("Blocks") {
        None => {
            if volume > MAX_EMPTY_SPONGE_CELLS {
                return Err(ConversionError::new(
                    Stage::Validate,
                    format!("empty Sponge schematic contains {volume} cells; safety limit is {MAX_EMPTY_SPONGE_CELLS}"),
                ));
            }
            let palette = vec![BlockState {
                name: "minecraft:air".into(),
                properties: Default::default(),
            }];
            let words = volume
                .checked_mul(bits_per_block(palette.len()))
                .and_then(|n| n.checked_add(63))
                .map(|n| n / 64)
                .ok_or_else(|| {
                    ConversionError::new(
                        Stage::Validate,
                        "schematic volume overflows packed storage",
                    )
                })?;
            (palette, vec![0; words], Vec::new())
        }
        Some(blocks) => {
            let blocks = compound(blocks, "Blocks", Stage::Validate)?;
            let palette_value = blocks.get("Palette").ok_or_else(|| {
                ConversionError::new(Stage::Validate, "Blocks is missing Palette")
            })?;
            let data = blocks.get("Data").ok_or_else(|| {
                ConversionError::new(Stage::Validate, "Blocks is missing Data byte array")
            })?;
            let (palette, remap, bits) = parse_sponge_palette(palette_value, "Blocks.Palette")?;
            let packed = decode_sponge_varints(data, volume, &remap, bits)?;
            let (entities, _) = sponge_block_entities(blocks.get("BlockEntities"), true, false)?;
            (palette, packed, entities)
        }
    };
    let mut warnings = sponge_warnings(schematic, true)?;
    if schematic.contains_key("Biomes") {
        warnings.push(ConversionWarning::new(
            "Sponge biomes are not represented in Minecraft Structure NBT",
        ));
    }
    let entities = sponge_entities(schematic.get("Entities"), true)?;
    Ok(ParsedSchematic {
        data_version: sponge_data_version(schematic, false)?,
        regions: vec![Region {
            name: "Sponge schematic".into(),
            position: [0; 3],
            size,
            palette,
            packed_states,
            block_entities,
            entities,
        }],
        warnings,
    })
}

fn sponge_data_version(
    schematic: &HashMap<String, Value>,
    optional: bool,
) -> Result<Option<i32>, ConversionError> {
    match schematic.get("DataVersion") {
        Some(Value::Int(value)) => Ok(Some(*value)),
        Some(other) => Err(wrong_type(
            "DataVersion",
            "Int",
            other,
            "Schematic",
            Stage::Validate,
        )),
        None if optional => Ok(None),
        None => Err(ConversionError::new(
            Stage::Validate,
            "missing DataVersion required by this Sponge schematic version",
        )),
    }
}

fn sponge_dimensions(
    schematic: &HashMap<String, Value>,
    region: &str,
) -> Result<[i32; 3], ConversionError> {
    let mut size = [0; 3];
    for (axis, key) in ["Width", "Height", "Length"].iter().enumerate() {
        size[axis] = match schematic.get(*key) {
            Some(Value::Short(value)) => i32::from(*value as u16),
            Some(other) => {
                return Err(wrong_type(
                    key,
                    "unsigned Short",
                    other,
                    region,
                    Stage::Validate,
                ))
            }
            None => {
                return Err(
                    ConversionError::new(Stage::Validate, format!("missing {key}"))
                        .in_region(region),
                )
            }
        };
        if size[axis] == 0 {
            return Err(
                ConversionError::new(Stage::Validate, format!("{key} must be non-zero"))
                    .in_region(region),
            );
        }
    }
    Ok(size)
}

fn checked_sponge_volume(size: [i32; 3]) -> Result<usize, ConversionError> {
    let volume = size
        .into_iter()
        .try_fold(1usize, |n, side| n.checked_mul(side as usize))
        .ok_or_else(|| {
            ConversionError::new(
                Stage::Validate,
                "schematic volume overflows addressable size",
            )
        })?;
    Ok(volume)
}

fn parse_sponge_palette(
    value: &Value,
    field: &str,
) -> Result<(Vec<BlockState>, HashMap<u32, usize>, usize), ConversionError> {
    let entries = compound(value, field, Stage::Validate)?;
    if entries.is_empty() {
        return Err(ConversionError::new(
            Stage::Validate,
            "Palette cannot be empty",
        ));
    }
    let mut by_id = BTreeMap::new();
    let mut used_ids = std::collections::HashSet::new();
    for (state_text, value) in entries {
        let id = match value {
            Value::Int(id) if *id >= 0 => *id as u32,
            other => {
                return Err(wrong_type(
                    "palette index",
                    "non-negative Int",
                    other,
                    field,
                    Stage::Validate,
                ))
            }
        };
        if !used_ids.insert(id) {
            return Err(ConversionError::new(
                Stage::Validate,
                format!("Palette assigns index {id} more than once"),
            ));
        }
        by_id.insert(id, parse_sponge_state(state_text)?);
    }
    let mut palette = Vec::with_capacity(by_id.len());
    let mut remap = HashMap::with_capacity(by_id.len());
    for (new_id, (old_id, state)) in by_id.into_iter().enumerate() {
        remap.insert(old_id, new_id);
        palette.push(state);
    }
    let bits = bits_per_block(palette.len());
    Ok((palette, remap, bits))
}

fn validate_palette_max(
    schematic: &HashMap<String, Value>,
    remap: &HashMap<u32, usize>,
) -> Result<(), ConversionError> {
    let Some(value) = schematic.get("PaletteMax") else {
        return Ok(());
    };
    let max = match value {
        Value::Int(max) if *max > 0 => *max as u32,
        Value::Int(_) => {
            return Err(ConversionError::new(
                Stage::Validate,
                "PaletteMax must be a positive Int",
            ))
        }
        other => {
            return Err(wrong_type(
                "PaletteMax",
                "positive Int",
                other,
                "root",
                Stage::Validate,
            ))
        }
    };
    if let Some(index) = remap.keys().find(|index| **index >= max) {
        return Err(ConversionError::new(
            Stage::Validate,
            format!("Palette index {index} is outside PaletteMax {max}"),
        ));
    }
    Ok(())
}

fn parse_sponge_state(text: &str) -> Result<BlockState, ConversionError> {
    let (name, props) = match text.split_once('[') {
        None => (text, None),
        Some((name, rest)) if rest.ends_with(']') => (name, Some(&rest[..rest.len() - 1])),
        Some(_) => {
            return Err(ConversionError::new(
                Stage::Validate,
                format!("invalid block state palette key {text:?}"),
            ))
        }
    };
    if name.is_empty() || name.contains(']') || name.contains('[') {
        return Err(ConversionError::new(
            Stage::Validate,
            format!("invalid block state palette key {text:?}"),
        ));
    }
    let mut properties = BTreeMap::new();
    if let Some(props) = props {
        if !props.is_empty() {
            for property in props.split(',') {
                let (key, value) = property.split_once('=').ok_or_else(|| {
                    ConversionError::new(
                        Stage::Validate,
                        format!("invalid block property in palette key {text:?}"),
                    )
                })?;
                if key.is_empty()
                    || value.is_empty()
                    || value.contains('=')
                    || value
                        .chars()
                        .any(|c| matches!(c, '[' | ']' | ',' | '=') || c.is_whitespace())
                    || key
                        .chars()
                        .any(|c| matches!(c, '[' | ']' | ',' | '=') || c.is_whitespace())
                    || properties
                        .insert(key.to_owned(), value.to_owned())
                        .is_some()
                {
                    return Err(ConversionError::new(
                        Stage::Validate,
                        format!("invalid or duplicate block property in palette key {text:?}"),
                    ));
                }
            }
        }
    }
    let mut namespace_path = name.split(':');
    let first = namespace_path.next().unwrap_or_default();
    let second = namespace_path.next();
    let (namespace, path) = match second {
        Some(path) if !path.is_empty() && namespace_path.next().is_none() => (first, path),
        Some(_) => {
            return Err(ConversionError::new(
                Stage::Validate,
                format!("invalid block state resource location {name:?}"),
            ))
        }
        None => ("minecraft", first),
    };
    let valid_namespace = |part: &str| {
        !part.is_empty()
            && part.chars().all(|c| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-' | '.')
            })
    };
    let valid_path = |part: &str| {
        !part.is_empty()
            && part.chars().all(|c| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-' | '.' | '/')
            })
    };
    if !valid_namespace(namespace) || !valid_path(path) {
        return Err(ConversionError::new(
            Stage::Validate,
            format!("invalid block state resource location {name:?}"),
        ));
    }
    let name = format!("{namespace}:{path}");
    Ok(BlockState { name, properties })
}

fn decode_sponge_varints(
    value: &Value,
    volume: usize,
    remap: &HashMap<u32, usize>,
    bits: usize,
) -> Result<Vec<i64>, ConversionError> {
    let bytes = match value {
        Value::ByteArray(bytes) => bytes,
        other => {
            return Err(wrong_type(
                "BlockData",
                "ByteArray",
                other,
                "Sponge schematic",
                Stage::Validate,
            ))
        }
    };
    let bytes = bytes.as_ref();
    if bytes.len() < volume {
        return Err(ConversionError::new(
            Stage::Validate,
            format!(
                "BlockData has {} bytes; at least {volume} are required for {volume} entries",
                bytes.len()
            ),
        ));
    }
    let word_count = volume
        .checked_mul(bits)
        .and_then(|n| n.checked_add(63))
        .map(|n| n / 64)
        .ok_or_else(|| {
            ConversionError::new(Stage::Validate, "packed block-state length overflows")
        })?;
    let mut words = Vec::new();
    words.try_reserve_exact(word_count).map_err(|e| {
        ConversionError::new(
            Stage::Validate,
            format!("cannot allocate packed block data: {e}"),
        )
    })?;
    words.resize(word_count, 0u64);
    let mut cursor = 0usize;
    for index in 0..volume {
        let palette_id = read_sponge_varint(bytes, &mut cursor)?;
        let mapped = remap.get(&palette_id).copied().ok_or_else(|| {
            ConversionError::new(
                Stage::Validate,
                format!("BlockData entry {index} references missing palette index {palette_id}"),
            )
        })?;
        let bit_start = index * bits;
        let word = bit_start / 64;
        let offset = bit_start % 64;
        words[word] |= (mapped as u64) << offset;
        if offset + bits > 64 {
            words[word + 1] |= (mapped as u64) >> (64 - offset);
        }
    }
    if cursor != bytes.len() {
        return Err(ConversionError::new(
            Stage::Validate,
            format!("BlockData contains extra bytes after {volume} entries"),
        ));
    }
    Ok(words.into_iter().map(|word| word as i64).collect())
}

fn read_sponge_varint(bytes: &[i8], cursor: &mut usize) -> Result<u32, ConversionError> {
    let start = *cursor;
    let mut result = 0u32;
    for byte_index in 0..5 {
        let byte = *bytes.get(*cursor).ok_or_else(|| {
            ConversionError::new(
                Stage::Validate,
                format!("BlockData has a truncated VarInt at byte {start}"),
            )
        })? as u8;
        *cursor += 1;
        if byte_index == 4 && (byte & 0x80) != 0 {
            return Err(ConversionError::new(
                Stage::Validate,
                format!("BlockData VarInt at byte {start} exceeds five bytes"),
            ));
        }
        if byte_index == 4 && (byte & 0x70) != 0 {
            return Err(ConversionError::new(
                Stage::Validate,
                format!("BlockData VarInt at byte {start} overflows 32 bits"),
            ));
        }
        result |= u32::from(byte & 0x7f) << (byte_index * 7);
        if byte & 0x80 == 0 {
            return Ok(result);
        }
    }
    Err(ConversionError::new(
        Stage::Validate,
        format!("BlockData VarInt at byte {start} exceeds five bytes"),
    ))
}

fn sponge_block_entities(
    value: Option<&Value>,
    nested_data: bool,
    strip_v1_content_version: bool,
) -> Result<(Vec<NbtValue>, bool), ConversionError> {
    let Some(value) = value else {
        return Ok((Vec::new(), false));
    };
    let entries = list(value, "BlockEntities", "Sponge schematic")?;
    let mut stripped_content_version = false;
    let normalized = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let c = match entry {
                Value::Compound(c) => c,
                other => {
                    return Err(wrong_type(
                        &format!("BlockEntities[{index}]"),
                        "Compound",
                        other,
                        "Sponge schematic",
                        Stage::Validate,
                    ))
                }
            };
            let mut out = c.clone();
            if strip_v1_content_version {
                if let Some(content_version) = out.remove("ContentVersion") {
                    if !matches!(content_version, Value::Int(_)) {
                        return Err(wrong_type(
                            "ContentVersion",
                            "Int",
                            &content_version,
                            "TileEntities",
                            Stage::Validate,
                        ));
                    }
                    stripped_content_version = true;
                }
            }
            let pos = match out.remove("Pos") {
                Some(Value::IntArray(pos)) if pos.len() == 3 => pos.to_vec(),
                Some(other) => {
                    return Err(wrong_type(
                        "BlockEntities.Pos",
                        "IntArray of length 3",
                        &other,
                        "Sponge schematic",
                        Stage::Validate,
                    ))
                }
                None => {
                    return Err(ConversionError::new(
                        Stage::Validate,
                        "BlockEntities entry is missing Pos",
                    ))
                }
            };
            let id = take_sponge_id(&mut out, "BlockEntities")?;
            for (key, coordinate) in [("x", pos[0]), ("y", pos[1]), ("z", pos[2])] {
                match out.get(key) {
                    Some(Value::Int(old)) if *old == coordinate => (),
                    Some(_) => {
                        return Err(ConversionError::new(
                            Stage::Validate,
                            format!("BlockEntities.Pos conflicts with payload {key}"),
                        ))
                    }
                    None => {
                        out.insert(key.into(), Value::Int(coordinate));
                    }
                }
            }
            if nested_data {
                merge_sponge_data(&mut out, "BlockEntities")?;
            }
            put_sponge_id(&mut out, id, "BlockEntities")?;
            Ok(Value::Compound(out))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((normalized, stripped_content_version))
}

fn sponge_entities(
    value: Option<&Value>,
    nested_data: bool,
) -> Result<Vec<NbtValue>, ConversionError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let entries = list(value, "Entities", "Sponge schematic")?;
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let c = match entry {
                Value::Compound(c) => c,
                other => {
                    return Err(wrong_type(
                        &format!("Entities[{index}]"),
                        "Compound",
                        other,
                        "Sponge schematic",
                        Stage::Validate,
                    ))
                }
            };
            let mut out = c.clone();
            match out.get("Pos") {
                Some(Value::List(pos))
                    if pos.len() == 3 && pos.iter().all(|v| matches!(v, Value::Double(_))) =>
                {
                    ()
                }
                Some(other) => {
                    return Err(wrong_type(
                        "Entities.Pos",
                        "List of 3 Doubles",
                        other,
                        "Sponge schematic",
                        Stage::Validate,
                    ))
                }
                None => {
                    return Err(ConversionError::new(
                        Stage::Validate,
                        "Entities entry is missing Pos",
                    ))
                }
            }
            let id = take_sponge_id(&mut out, "Entities")?;
            if nested_data {
                merge_sponge_data(&mut out, "Entities")?;
            }
            put_sponge_id(&mut out, id, "Entities")?;
            Ok(Value::Compound(out))
        })
        .collect()
}

fn take_sponge_id(
    compound: &mut HashMap<String, Value>,
    field: &str,
) -> Result<String, ConversionError> {
    match compound.remove("Id") {
        Some(Value::String(id)) if !id.is_empty() => Ok(id),
        Some(other) => Err(wrong_type(
            "Id",
            "non-empty String",
            &other,
            field,
            Stage::Validate,
        )),
        None => Err(ConversionError::new(
            Stage::Validate,
            format!("{field} entry is missing Id"),
        )),
    }
}

fn put_sponge_id(
    compound: &mut HashMap<String, Value>,
    id: String,
    field: &str,
) -> Result<(), ConversionError> {
    match compound.get("id") {
        Some(Value::String(old)) if *old == id => Ok(()),
        Some(_) => Err(ConversionError::new(
            Stage::Validate,
            format!("{field} Id conflicts with payload id"),
        )),
        None => {
            compound.insert("id".into(), Value::String(id));
            Ok(())
        }
    }
}

fn merge_sponge_data(
    compound: &mut HashMap<String, Value>,
    field: &str,
) -> Result<(), ConversionError> {
    let Some(data) = compound.remove("Data") else {
        return Ok(());
    };
    let data = match data {
        Value::Compound(data) => data,
        other => {
            return Err(wrong_type(
                "Data",
                "Compound",
                &other,
                field,
                Stage::Validate,
            ))
        }
    };
    for (key, value) in data {
        if let Some(old) = compound.get(&key) {
            if old != &value {
                return Err(ConversionError::new(
                    Stage::Validate,
                    format!("{field} Data.{key} conflicts with a sibling field"),
                ));
            }
        } else {
            compound.insert(key, value);
        }
    }
    Ok(())
}

fn sponge_warnings(
    schematic: &HashMap<String, Value>,
    include_biomes: bool,
) -> Result<Vec<ConversionWarning>, ConversionError> {
    let mut warnings = Vec::new();
    if schematic.contains_key("Metadata") {
        warnings.push(ConversionWarning::new(
            "Sponge schematic metadata is not represented in Minecraft Structure NBT",
        ));
    }
    if include_biomes
        && (schematic.contains_key("BiomePalette") || schematic.contains_key("BiomeData"))
    {
        warnings.push(ConversionWarning::new(
            "Sponge biomes are not represented in Minecraft Structure NBT",
        ));
    }
    if let Some(offset) = schematic.get("Offset") {
        match offset {
            Value::IntArray(values) if values.len() == 3 => {
                if values.iter().any(|value| *value != 0) {
                    warnings.push(ConversionWarning::new(
                        "non-zero Sponge Offset is not applied to the normalized structure",
                    ));
                }
            }
            other => {
                return Err(wrong_type(
                    "Offset",
                    "IntArray of length 3",
                    other,
                    "Sponge schematic",
                    Stage::Validate,
                ))
            }
        }
    }
    Ok(warnings)
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
        gzip_bytes(&nbt)
    }
    fn gzip_bytes(bytes: &[u8]) -> Vec<u8> {
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        gz.write_all(bytes).unwrap();
        gz.finish().unwrap()
    }

    fn varints(values: &[u32]) -> Vec<i8> {
        let mut out = Vec::new();
        for value in values {
            let mut value = *value;
            loop {
                let mut byte = (value & 0x7f) as u8;
                value >>= 7;
                if value != 0 {
                    byte |= 0x80;
                }
                out.push(byte as i8);
                if value == 0 {
                    break;
                }
            }
        }
        out
    }

    fn sponge_palette(entries: &[(&str, i32)]) -> Value {
        comp(
            entries
                .iter()
                .map(|(state, index)| ((*state).into(), Value::Int(*index))),
        )
    }

    fn sponge_v1_v2(version: i32) -> Value {
        let mut root = HashMap::from([
            ("Version".into(), Value::Int(version)),
            ("Width".into(), Value::Short(2)),
            ("Height".into(), Value::Short(2)),
            ("Length".into(), Value::Short(3)),
            (
                "Palette".into(),
                sponge_palette(&[("stone", 7), ("mod:block[k=v]", 2)]),
            ),
            (
                "BlockData".into(),
                Value::ByteArray(fastnbt::ByteArray::new(varints(&[
                    7, 2, 7, 2, 7, 2, 7, 2, 7, 2, 7, 2,
                ]))),
            ),
            (
                "Offset".into(),
                Value::IntArray(fastnbt::IntArray::new(vec![3, 0, 0])),
            ),
            (
                "Metadata".into(),
                comp([("Name".into(), Value::String("test".into()))]),
            ),
        ]);
        if version == 2 {
            root.insert("DataVersion".into(), Value::Int(3955));
        }
        if version == 1 {
            root.insert(
                "TileEntities".into(),
                Value::List(vec![comp([
                    (
                        "Pos".into(),
                        Value::IntArray(fastnbt::IntArray::new(vec![1, 0, 0])),
                    ),
                    ("Id".into(), Value::String("minecraft:chest".into())),
                    ("ContentVersion".into(), Value::Int(1)),
                    ("CustomTag".into(), Value::Long(9_007_199_254_740_993)),
                ])]),
            );
        } else {
            root.insert(
                "BlockEntities".into(),
                Value::List(vec![comp([
                    (
                        "Pos".into(),
                        Value::IntArray(fastnbt::IntArray::new(vec![1, 0, 0])),
                    ),
                    ("Id".into(), Value::String("minecraft:chest".into())),
                    (
                        "ContentVersion".into(),
                        Value::String("custom payload".into()),
                    ),
                    ("CustomTag".into(), Value::Long(9_007_199_254_740_993)),
                ])]),
            );
            root.insert(
                "Entities".into(),
                Value::List(vec![comp([
                    (
                        "Pos".into(),
                        Value::List(vec![
                            Value::Double(1.25),
                            Value::Double(0.5),
                            Value::Double(2.0),
                        ]),
                    ),
                    ("Id".into(), Value::String("mod:entity".into())),
                    (
                        "Motion".into(),
                        Value::List(vec![
                            Value::Double(0.0),
                            Value::Double(0.1),
                            Value::Double(0.0),
                        ]),
                    ),
                ])]),
            );
        }
        Value::Compound(root)
    }

    fn sponge_v3() -> Value {
        let blocks = comp([
            (
                "Palette".into(),
                sponge_palette(&[("minecraft:air", 0), ("mod:block[prop=x]", 3)]),
            ),
            (
                "Data".into(),
                Value::ByteArray(fastnbt::ByteArray::new(varints(&[0, 3]))),
            ),
            (
                "BlockEntities".into(),
                Value::List(vec![comp([
                    (
                        "Pos".into(),
                        Value::IntArray(fastnbt::IntArray::new(vec![1, 0, 0])),
                    ),
                    ("Id".into(), Value::String("mod:be".into())),
                    (
                        "Data".into(),
                        comp([(
                            "payload".into(),
                            Value::ByteArray(fastnbt::ByteArray::new(vec![1, -1])),
                        )]),
                    ),
                ])]),
            ),
        ]);
        let schematic = comp([
            ("Version".into(), Value::Int(3)),
            ("DataVersion".into(), Value::Int(3955)),
            ("Width".into(), Value::Short(2)),
            ("Height".into(), Value::Short(1)),
            ("Length".into(), Value::Short(1)),
            ("Blocks".into(), blocks),
            (
                "Entities".into(),
                Value::List(vec![comp([
                    (
                        "Pos".into(),
                        Value::List(vec![
                            Value::Double(0.25),
                            Value::Double(0.0),
                            Value::Double(0.0),
                        ]),
                    ),
                    ("Id".into(), Value::String("minecraft:bat".into())),
                    ("Data".into(), comp([("health".into(), Value::Float(4.0))])),
                ])]),
            ),
        ]);
        comp([("Schematic".into(), schematic)])
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

    #[test]
    fn parses_sponge_v1_and_v2_sparse_palette_and_normalizes_v1_payloads() {
        let v1 = parse_schem(gzip_value(sponge_v1_v2(1)).as_slice()).unwrap();
        assert_eq!(v1.data_version, None);
        assert_eq!(v1.regions[0].size, [2, 2, 3]);
        assert_eq!(v1.regions[0].position, [0, 0, 0]);
        assert_eq!(v1.regions[0].palette[0].name, "mod:block");
        assert_eq!(v1.regions[0].palette[0].properties.get("k").unwrap(), "v");
        assert_eq!(v1.regions[0].palette[1].name, "minecraft:stone");
        assert_eq!(decode_palette_index(&v1.regions[0], 0).unwrap(), 1);
        assert_eq!(decode_palette_index(&v1.regions[0], 1).unwrap(), 0);
        let be = &v1.regions[0].block_entities[0];
        let Value::Compound(be) = be else {
            panic!("compound")
        };
        assert_eq!(be.get("x"), Some(&Value::Int(1)));
        assert_eq!(be.get("id"), Some(&Value::String("minecraft:chest".into())));
        assert_eq!(
            be.get("CustomTag"),
            Some(&Value::Long(9_007_199_254_740_993))
        );
        assert_eq!(be.get("ContentVersion"), None);
        assert!(v1
            .warnings
            .iter()
            .any(|warning| warning.message.contains("TileEntities ContentVersion")));
        assert!(v1.warnings.iter().any(|w| w.message.contains("Offset")));

        let v2 = parse_schematic(gzip_value(sponge_v1_v2(2)).as_slice()).unwrap();
        assert_eq!(v2.data_version, Some(3955));
        assert_eq!(v2.regions[0].block_entities.len(), 1);
        assert_eq!(v2.regions[0].entities.len(), 1);
        let Value::Compound(entity) = &v2.regions[0].entities[0] else {
            panic!("compound")
        };
        assert_eq!(entity.get("Id"), None);
        assert_eq!(entity.get("id"), Some(&Value::String("mod:entity".into())));
        assert!(matches!(entity.get("Motion"), Some(Value::List(_))));
        let Value::Compound(block_entity) = &v2.regions[0].block_entities[0] else {
            panic!("compound")
        };
        assert_eq!(
            block_entity.get("ContentVersion"),
            Some(&Value::String("custom payload".into()))
        );
    }

    #[test]
    fn parses_sponge_v3_nested_data_and_preserves_nbt_types() {
        let parsed = parse_schematic(gzip_value(sponge_v3()).as_slice()).unwrap();
        assert_eq!(parsed.data_version, Some(3955));
        assert_eq!(parsed.regions[0].palette[1].name, "mod:block");
        assert_eq!(
            parsed.regions[0].palette[1].properties.get("prop").unwrap(),
            "x"
        );
        let be = &parsed.regions[0].block_entities[0];
        let Value::Compound(be) = be else {
            panic!("compound")
        };
        assert_eq!(be.get("id"), Some(&Value::String("mod:be".into())));
        assert_eq!(be.get("x"), Some(&Value::Int(1)));
        assert_eq!(be.get("Data"), None);
        assert_eq!(
            be.get("payload"),
            Some(&Value::ByteArray(fastnbt::ByteArray::new(vec![1, -1])))
        );
        let entity = &parsed.regions[0].entities[0];
        let Value::Compound(entity) = entity else {
            panic!("compound")
        };
        assert_eq!(entity.get("health"), Some(&Value::Float(4.0)));
        assert_eq!(
            entity.get("id"),
            Some(&Value::String("minecraft:bat".into()))
        );
    }

    #[test]
    fn accepts_empty_sponge_v3_with_entities_but_caps_declared_empty_volume() {
        let mut root = match sponge_v3() {
            Value::Compound(root) => root,
            _ => unreachable!(),
        };
        {
            let Value::Compound(schematic) = root.get_mut("Schematic").unwrap() else {
                panic!("compound")
            };
            schematic.remove("Blocks");
        }
        let parsed = parse_schem(gzip_value(Value::Compound(root.clone())).as_slice()).unwrap();
        assert_eq!(parsed.regions[0].size, [2, 1, 1]);
        assert_eq!(parsed.regions[0].palette[0].name, "minecraft:air");
        assert!(parsed.regions[0].block_entities.is_empty());
        assert_eq!(parsed.regions[0].entities.len(), 1);

        let Value::Compound(schematic) = root.get_mut("Schematic").unwrap() else {
            panic!("compound")
        };
        schematic.insert("Width".into(), Value::Short(i16::MAX));
        schematic.insert("Height".into(), Value::Short(i16::MAX));
        schematic.insert("Length".into(), Value::Short(i16::MAX));
        let err = parse_schem(gzip_value(Value::Compound(root)).as_slice()).unwrap_err();
        assert!(err.message.contains("empty Sponge schematic"));
    }

    #[test]
    fn sponge_rejects_missing_local_palette_and_malformed_or_unmatched_varints() {
        let mut root = match sponge_v1_v2(1) {
            Value::Compound(root) => root,
            _ => unreachable!(),
        };
        root.remove("Palette");
        let err = parse_schem(gzip_value(Value::Compound(root)).as_slice()).unwrap_err();
        assert!(err.message.contains("local Palette"));

        for bytes in [
            vec![],
            vec![0x80],
            vec![0x80, 0x80, 0x80, 0x80, 0x80],
            vec![0xff, 0xff, 0xff, 0xff, 0x10],
        ] {
            let err = decode_sponge_varints(
                &Value::ByteArray(fastnbt::ByteArray::new(
                    bytes.into_iter().map(|b| b as i8).collect(),
                )),
                1,
                &HashMap::from([(0, 0)]),
                2,
            )
            .unwrap_err();
            assert_eq!(err.stage, Stage::Validate);
        }
        let err = decode_sponge_varints(
            &Value::ByteArray(fastnbt::ByteArray::new(varints(&[2]))),
            1,
            &HashMap::from([(0, 0)]),
            2,
        )
        .unwrap_err();
        assert!(err.message.contains("missing palette index"));
        let err = decode_sponge_varints(
            &Value::ByteArray(fastnbt::ByteArray::new(varints(&[0, 0]))),
            1,
            &HashMap::from([(0, 0)]),
            2,
        )
        .unwrap_err();
        assert!(err.message.contains("extra bytes"));
        assert_eq!(
            decode_sponge_varints(
                &Value::ByteArray(fastnbt::ByteArray::new(vec![-128, 0])),
                1,
                &HashMap::from([(0, 0)]),
                2,
            )
            .unwrap(),
            vec![0]
        );

        let err = parse_schem(
            gzip_value(comp([
                ("Version".into(), Value::Int(1)),
                ("Width".into(), Value::Short(1)),
                ("Height".into(), Value::Short(1)),
                ("Length".into(), Value::Short(1)),
                ("Palette".into(), sponge_palette(&[("minecraft:air", -1)])),
                (
                    "BlockData".into(),
                    Value::ByteArray(fastnbt::ByteArray::new(vec![0])),
                ),
            ]))
            .as_slice(),
        )
        .unwrap_err();
        assert!(err.message.contains("non-negative Int"));
    }

    #[test]
    fn sponge_coordinates_and_nested_payload_collisions_are_rejected() {
        let mut root = match sponge_v1_v2(1) {
            Value::Compound(root) => root,
            _ => unreachable!(),
        };
        let Value::List(entities) = root.get_mut("TileEntities").unwrap() else {
            panic!("list")
        };
        let Value::Compound(be) = &mut entities[0] else {
            panic!("compound")
        };
        be.insert("x".into(), Value::Int(2));
        let err = parse_schem(gzip_value(Value::Compound(root)).as_slice()).unwrap_err();
        assert!(err.message.contains("conflicts with payload x"));

        let mut root = match sponge_v3() {
            Value::Compound(root) => root,
            _ => unreachable!(),
        };
        let Value::Compound(schematic) = root.get_mut("Schematic").unwrap() else {
            panic!("compound")
        };
        let Value::List(entities) = schematic.get_mut("Entities").unwrap() else {
            panic!("list")
        };
        let Value::Compound(entity) = &mut entities[0] else {
            panic!("compound")
        };
        entity.insert(
            "Data".into(),
            comp([("id".into(), Value::String("mod:wrong".into()))]),
        );
        let err = parse_schem(gzip_value(Value::Compound(root)).as_slice()).unwrap_err();
        assert!(err.message.contains("conflicts with payload id"));
    }

    #[test]
    fn sponge_dimensions_decode_unsigned_shorts() {
        let dimensions = HashMap::from([
            ("Width".into(), Value::Short(-1)),
            ("Height".into(), Value::Short(1)),
            ("Length".into(), Value::Short(2)),
        ]);
        assert_eq!(
            sponge_dimensions(&dimensions, "root").unwrap(),
            [65_535, 1, 2]
        );
    }

    #[test]
    fn sponge_varints_repack_across_compact_long_boundaries() {
        let values: Vec<u32> = (0..40).collect();
        let remap: HashMap<u32, usize> = values
            .iter()
            .enumerate()
            .map(|(index, value)| (*value, index))
            .collect();
        let packed = decode_sponge_varints(
            &Value::ByteArray(fastnbt::ByteArray::new(varints(&values))),
            values.len(),
            &remap,
            bits_per_block(values.len()),
        )
        .unwrap();
        let region = Region {
            name: "packed-boundaries".into(),
            position: [0; 3],
            size: [values.len() as i32, 1, 1],
            palette: values
                .iter()
                .map(|index| BlockState {
                    name: format!("mod:block_{index}"),
                    properties: Default::default(),
                })
                .collect(),
            packed_states: packed,
            block_entities: Vec::new(),
            entities: Vec::new(),
        };
        for (index, expected) in values.iter().enumerate() {
            assert_eq!(
                decode_palette_index(&region, index).unwrap(),
                *expected as usize
            );
        }
    }

    #[test]
    fn validates_optional_palette_max_and_block_state_resource_syntax() {
        let mut root = match sponge_v1_v2(1) {
            Value::Compound(root) => root,
            _ => unreachable!(),
        };
        root.insert("PaletteMax".into(), Value::Int(8));
        assert!(parse_schem(gzip_value(Value::Compound(root.clone())).as_slice()).is_ok());
        root.insert("PaletteMax".into(), Value::Int(7));
        assert!(
            parse_schem(gzip_value(Value::Compound(root.clone())).as_slice())
                .unwrap_err()
                .message
                .contains("outside PaletteMax")
        );
        root.insert("PaletteMax".into(), Value::String("8".into()));
        assert!(parse_schem(gzip_value(Value::Compound(root)).as_slice())
            .unwrap_err()
            .message
            .contains("PaletteMax"));

        for bad in [
            "stone[facing=north]]",
            "stone[prop=[x]",
            "stone[a=b=c]",
            "stone[a=]",
            ":stone",
            "mod:",
            "bad namespace:stone",
        ] {
            assert!(parse_sponge_state(bad).is_err(), "accepted {bad:?}");
        }
        assert_eq!(
            parse_sponge_state("modded:block[odd_property=unregistered]")
                .unwrap()
                .name,
            "modded:block"
        );
        assert!(parse_sponge_state("stone[]").unwrap().properties.is_empty());
    }

    #[test]
    fn gzip_crc_and_trailing_bytes_are_checked() {
        let mut valid = gzip_value(sponge_v3());
        let last = valid.len() - 8;
        valid[last] ^= 0x80;
        assert_eq!(
            parse_schematic(valid.as_slice()).unwrap_err().stage,
            Stage::Decompress
        );

        let mut trailing_gzip = gzip_value(sponge_v3());
        trailing_gzip.push(0);
        let err = parse_schematic(trailing_gzip.as_slice()).unwrap_err();
        assert_eq!(err.stage, Stage::Decompress);

        let mut nbt = fastnbt::to_bytes(&sponge_v3()).unwrap();
        nbt.push(0);
        let err = parse_schematic(gzip_bytes(&nbt).as_slice()).unwrap_err();
        assert_eq!(err.stage, Stage::Parse);
    }
}
