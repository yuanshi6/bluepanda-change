use crate::types::{
    BlockState, ConversionError, ConversionWarning, NbtValue, ParsedSchematic, Region, Stage,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Structure {
    pub size: [i32; 3],
    pub palette: Vec<StructurePaletteEntry>,
    pub blocks: Vec<StructureBlock>,
    pub entities: Vec<StructureEntity>,
    #[serde(rename = "DataVersion", skip_serializing_if = "Option::is_none")]
    pub data_version: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StructurePaletteEntry {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(
        rename = "Properties",
        default,
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub properties: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructureBlock {
    pub pos: [i32; 3],
    pub state: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nbt: Option<NbtValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructureEntity {
    pub pos: [f64; 3],
    #[serde(rename = "blockPos")]
    pub block_pos: [i32; 3],
    pub nbt: NbtValue,
}

#[derive(Debug, Clone)]
pub struct TransformOutput {
    pub structure: Structure,
    pub warnings: Vec<ConversionWarning>,
}

#[derive(Debug, Clone, PartialEq)]
struct Voxel {
    state: BlockState,
    block_entity: Option<NbtValue>,
    region: String,
}

pub fn build_structure(parsed: &ParsedSchematic) -> Result<Structure, ConversionError> {
    Ok(build_structure_with_warnings(parsed)?.structure)
}

pub fn build_structure_with_warnings(
    parsed: &ParsedSchematic,
) -> Result<TransformOutput, ConversionError> {
    build_structure_with_progress(parsed, |_, _| {})
}

pub(crate) fn build_structure_with_progress(
    parsed: &ParsedSchematic,
    mut progress: impl FnMut(usize, usize),
) -> Result<TransformOutput, ConversionError> {
    if parsed.regions.is_empty() {
        return Err(ConversionError::new(
            Stage::Validate,
            "schematic contains no regions",
        ));
    }
    let mut regions: Vec<&Region> = parsed.regions.iter().collect();
    regions.sort_by(|a, b| a.name.cmp(&b.name));
    let mut mins = [i64::MAX; 3];
    let mut maxs = [i64::MIN; 3];
    let mut region_bounds = Vec::with_capacity(parsed.regions.len());
    for r in &regions {
        let (min, max, dims) = bounds(r)?;
        for axis in 0..3 {
            mins[axis] = mins[axis].min(min[axis]);
            maxs[axis] = maxs[axis].max(max[axis]);
        }
        region_bounds.push((min, max, dims));
    }
    let mut out_size = [0i32; 3];
    for axis in 0..3 {
        let n = maxs[axis]
            .checked_sub(mins[axis])
            .and_then(|v| v.checked_add(1))
            .ok_or_else(|| ConversionError::new(Stage::Normalize, "overall size overflow"))?;
        out_size[axis] = i32::try_from(n).map_err(|_| {
            ConversionError::new(
                Stage::Normalize,
                "overall size exceeds Structure NBT i32 dimensions",
            )
        })?;
    }

    // Per-region packed arrays remain compact; only occupied voxel coordinates are materialized.
    let mut voxels: HashMap<[i64; 3], Voxel> = HashMap::new();
    let mut processed: Vec<(&Region, [i64; 3], [i64; 3], HashMap<[i64; 3], NbtValue>)> = Vec::new();
    let warnings = parsed.warnings.clone();
    let mut entity_by_uuid: HashMap<String, ([i64; 3], NbtValue)> = HashMap::new();
    let mut entities: Vec<([i64; 3], [f64; 3], Option<[i32; 3]>, NbtValue)> = Vec::new();
    let total_cells = region_bounds
        .iter()
        .try_fold(0usize, |sum, (_, _, dims)| {
            dims[0]
                .checked_mul(dims[1])
                .and_then(|value| value.checked_mul(dims[2]))
                .and_then(|count| sum.checked_add(usize::try_from(count).ok()?))
        })
        .ok_or_else(|| ConversionError::new(Stage::Decode, "total region volume overflow"))?;
    let mut completed_cells = 0usize;
    for (r, (min, max, dims)) in regions.into_iter().zip(region_bounds.iter()) {
        let count = dims[0]
            .checked_mul(dims[1])
            .and_then(|v| v.checked_mul(dims[2]))
            .and_then(|v| usize::try_from(v).ok())
            .ok_or_else(|| {
                ConversionError::new(Stage::Decode, "region volume overflow")
                    .in_region(r.name.clone())
            })?;
        if r.palette.is_empty() {
            return Err(
                ConversionError::new(Stage::Decode, "empty block-state palette")
                    .in_region(r.name.clone()),
            );
        }
        let mut block_entities: HashMap<[i64; 3], NbtValue> = HashMap::new();
        let mut used_block_entities = std::collections::HashSet::new();
        for raw in &r.block_entities {
            let (local, data) = normalize_block_entity(raw, r, *min)?;
            if local
                .iter()
                .enumerate()
                .any(|(a, v)| *v < min[a] || *v > max[a])
            {
                return Err(ConversionError::new(
                    Stage::Validate,
                    "block entity position is outside region bounds",
                )
                .in_region(r.name.clone()));
            }
            if let Some(old) = block_entities.insert(local, data.clone()) {
                if old != data {
                    return Err(ConversionError::new(
                        Stage::Merge,
                        "conflicting block entities within region",
                    )
                    .in_region(r.name.clone()));
                }
            }
        }
        for i in 0..count {
            if i % 16_384 == 0 {
                progress(completed_cells.saturating_add(i), total_cells.max(1));
            }
            let palette_index = crate::parser::decode_palette_index(r, i)?;
            let state = r
                .palette
                .get(palette_index)
                .ok_or_else(|| {
                    ConversionError::new(Stage::Decode, "palette index out of range")
                        .in_region(r.name.clone())
                })?
                .clone();
            let x = i as i64 % dims[0];
            let z = (i as i64 / dims[0]) % dims[2];
            let y = i as i64 / (dims[0] * dims[2]);
            let pos = [min[0] + x, min[1] + y, min[2] + z];
            let be = block_entities.get(&pos).cloned();
            if be.is_some() {
                used_block_entities.insert(pos);
            }
            for (old_region, old_min, old_max, old_bes) in &processed {
                if (0..3).all(|a| pos[a] >= old_min[a] && pos[a] <= old_max[a]) {
                    let old_dims = dimensions(old_region)?;
                    let ox = pos[0] - old_min[0];
                    let oy = pos[1] - old_min[1];
                    let oz = pos[2] - old_min[2];
                    let old_linear = usize::try_from(
                        oy.checked_mul(old_dims[0])
                            .and_then(|v| v.checked_mul(old_dims[2]))
                            .and_then(|v| v.checked_add(oz * old_dims[0] + ox))
                            .ok_or_else(|| {
                                ConversionError::new(Stage::Decode, "overlap index overflow")
                            })?,
                    )
                    .map_err(|_| {
                        ConversionError::new(
                            Stage::Decode,
                            "overlap index exceeds addressable range",
                        )
                    })?;
                    let old_index = crate::parser::decode_palette_index(old_region, old_linear)?;
                    let old_state = old_region.palette.get(old_index).ok_or_else(|| {
                        ConversionError::new(Stage::Decode, "overlap palette index out of range")
                            .in_region(old_region.name.clone())
                    })?;
                    if old_state != &state || old_bes.get(&pos) != be.as_ref() {
                        return Err(ConversionError::new(
                            Stage::Merge,
                            format!(
                                "overlapping regions '{}' and '{}' contain different block data",
                                old_region.name, r.name
                            ),
                        )
                        .in_region(r.name.clone())
                        .at(to_i32_pos(pos)?));
                    }
                }
            }
            if !is_ordinary_air(&state) {
                if let Some(existing) = voxels.get(&pos) {
                    if existing.state != state || existing.block_entity != be {
                        return Err(ConversionError::new(
                            Stage::Merge,
                            format!(
                                "overlapping regions '{}' and '{}' contain different block data",
                                existing.region, r.name
                            ),
                        )
                        .in_region(r.name.clone())
                        .at(to_i32_pos(pos)?));
                    }
                } else {
                    voxels.insert(
                        pos,
                        Voxel {
                            state,
                            block_entity: be,
                            region: r.name.clone(),
                        },
                    );
                }
            }
        }
        completed_cells = completed_cells.saturating_add(count);
        progress(completed_cells, total_cells.max(1));
        if let Some(pos) = block_entities
            .keys()
            .find(|p| !used_block_entities.contains(*p))
            .copied()
        {
            return Err(ConversionError::new(
                Stage::Validate,
                "block entity does not correspond to an in-region block position",
            )
            .in_region(r.name.clone())
            .at(to_i32_pos(pos)?));
        }
        processed.push((r, *min, *max, block_entities));
        for raw in &r.entities {
            let (world_pos, normalized_pos, anchor, data) = normalize_entity(raw, r, mins)?;
            if let Some(uuid) = entity_uuid(&data) {
                if let Some((old_pos, old_data)) = entity_by_uuid.get(&uuid) {
                    if *old_pos != world_pos || *old_data != data {
                        return Err(ConversionError::new(
                            Stage::Merge,
                            "entities with the same UUID have different normalized data",
                        )
                        .in_region(r.name.clone())
                        .at(to_i32_pos(world_pos)?));
                    }
                    continue;
                }
                entity_by_uuid.insert(uuid, (world_pos, data.clone()));
            }
            entities.push((world_pos, normalized_pos, anchor, data));
        }
    }

    // Palette entries are assigned deterministically in first-encounter order. Air remains part of
    // overlap comparisons but ordinary air records are omitted from Structure.blocks.
    let air = BlockState {
        name: "minecraft:air".into(),
        properties: BTreeMap::new(),
    };
    let mut palette: Vec<BlockState> = Vec::new();
    let mut palette_map: HashMap<String, i32> = HashMap::new();
    let mut blocks = Vec::new();
    let mut ordered: Vec<_> = voxels.into_iter().collect();
    ordered.sort_by_key(|(p, _)| (p[1], p[0], p[2]));
    for (world, voxel) in ordered {
        let key = state_key(&voxel.state);
        let state_index = if let Some(i) = palette_map.get(&key) {
            *i
        } else {
            let i = i32::try_from(palette.len()).map_err(|_| {
                ConversionError::new(Stage::Write, "palette exceeds i32 index range")
            })?;
            palette.push(voxel.state.clone());
            palette_map.insert(key, i);
            i
        };
        let rel = relative_pos(world, mins)?;
        blocks.push(StructureBlock {
            pos: rel,
            state: state_index,
            nbt: voxel.block_entity,
        });
    }
    if palette.is_empty() {
        palette.push(air);
    }
    let palette = palette
        .into_iter()
        .map(|s| StructurePaletteEntry {
            name: s.name,
            properties: s.properties,
        })
        .collect();
    let mut output_entities = Vec::with_capacity(entities.len());
    for (_, pos, anchor, data) in entities {
        let floor_pos = checked_floor_pos(pos)?;
        let block_pos = anchor.unwrap_or(floor_pos);
        output_entities.push(StructureEntity {
            pos,
            block_pos,
            nbt: data,
        });
    }
    Ok(TransformOutput {
        structure: Structure {
            size: out_size,
            palette,
            blocks,
            entities: output_entities,
            data_version: parsed.data_version,
        },
        warnings,
    })
}

fn bounds(r: &Region) -> Result<([i64; 3], [i64; 3], [i64; 3]), ConversionError> {
    let mut min = [0; 3];
    let mut max = [0; 3];
    let mut dims = [0; 3];
    for a in 0..3 {
        if r.size[a] == 0 {
            return Err(
                ConversionError::new(Stage::Validate, "region has zero size")
                    .in_region(r.name.clone()),
            );
        }
        let p = i64::from(r.position[a]);
        let s = i64::from(r.size[a]);
        let end = p
            .checked_add(if s > 0 { s - 1 } else { s + 1 })
            .ok_or_else(|| {
                ConversionError::new(Stage::Normalize, "region endpoint overflow")
                    .in_region(r.name.clone())
            })?;
        min[a] = p.min(end);
        max[a] = p.max(end);
        dims[a] = max[a] - min[a] + 1;
        if dims[a] != s.abs() {
            return Err(ConversionError::new(
                Stage::Validate,
                "region dimensions are inconsistent",
            )
            .in_region(r.name.clone()));
        }
    }
    Ok((min, max, dims))
}

fn dimensions(r: &Region) -> Result<[i64; 3], ConversionError> {
    Ok(bounds(r)?.2)
}
fn is_ordinary_air(s: &BlockState) -> bool {
    matches!(
        s.name.as_str(),
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
    )
}
fn state_key(s: &BlockState) -> String {
    let mut key = String::new();
    append_key_part(&mut key, &s.name);
    for (k, v) in &s.properties {
        append_key_part(&mut key, k);
        append_key_part(&mut key, v);
    }
    key
}
fn append_key_part(out: &mut String, part: &str) {
    out.push_str(&part.len().to_string());
    out.push(':');
    out.push_str(part);
}

fn normalize_block_entity(
    raw: &NbtValue,
    r: &Region,
    min: [i64; 3],
) -> Result<([i64; 3], NbtValue), ConversionError> {
    let mut nbt = raw.clone();
    let c = compound_mut(&mut nbt).ok_or_else(|| {
        ConversionError::new(Stage::Validate, "block entity entry is not a compound")
            .in_region(r.name.clone())
    })?;
    let mut p = [0i64; 3];
    for (a, k) in ["x", "y", "z"].iter().enumerate() {
        let local = numeric_i64(c.get(*k)).ok_or_else(|| {
            ConversionError::new(Stage::Validate, format!("block entity missing integer {k}"))
                .in_region(r.name.clone())
        })?;
        p[a] = min[a].checked_add(local).ok_or_else(|| {
            ConversionError::new(Stage::Normalize, "block entity position overflow")
                .in_region(r.name.clone())
        })?;
        c.remove(*k);
    }
    Ok((p, nbt))
}

fn normalize_entity(
    raw: &NbtValue,
    r: &Region,
    global_min: [i64; 3],
) -> Result<([i64; 3], [f64; 3], Option<[i32; 3]>, NbtValue), ConversionError> {
    let mut data = raw.clone();
    let c = compound_mut(&mut data).ok_or_else(|| {
        ConversionError::new(Stage::Validate, "entity entry is not a compound")
            .in_region(r.name.clone())
    })?;
    let coords = match c.get("Pos") {
        Some(NbtValue::List(v)) if v.len() == 3 => v,
        _ => {
            return Err(ConversionError::new(
                Stage::Validate,
                "entity has no three-element Pos list",
            )
            .in_region(r.name.clone()))
        }
    };
    let delta = [
        i64::from(r.position[0]) - global_min[0],
        i64::from(r.position[1]) - global_min[1],
        i64::from(r.position[2]) - global_min[2],
    ];
    let mut pos = [0f64; 3];
    for a in 0..3 {
        let n = numeric_f64(&coords[a]).ok_or_else(|| {
            ConversionError::new(Stage::Validate, "entity Pos contains a non-number")
                .in_region(r.name.clone())
        })?;
        pos[a] = n + delta[a] as f64;
    }
    let mut world = [0i64; 3];
    for a in 0..3 {
        let source_world = numeric_f64(&coords[a]).expect("validated above") + r.position[a] as f64;
        world[a] = checked_floor_i64(source_world).map_err(|e| e.in_region(r.name.clone()))?;
    }
    let mut shifted = Vec::with_capacity(3);
    for a in 0..3 {
        shifted.push(match &coords[a] {
            NbtValue::Float(_) => NbtValue::Float(pos[a] as f32),
            _ => NbtValue::Double(pos[a]),
        });
    }
    c.insert("Pos".into(), NbtValue::List(shifted));
    let legacy_anchor = shift_legacy_triplet(c, "Tile", delta, &r.name)?;
    for prefix in ["Tile", "Sleeping"] {
        if prefix == "Tile" {
            continue;
        } // TileXYZ were already shifted and collected as an anchor.
        for (a, suffix) in ["X", "Y", "Z"].iter().enumerate() {
            let key = format!("{prefix}{suffix}");
            if c.contains_key(&key) {
                let shifted = shift_int_coordinate(c, &key, delta[a], &r.name)?;
                c.insert(key, NbtValue::Int(shifted));
            }
        }
    }
    let block_anchor = shift_block_pos_field(c, "block_pos", delta, &r.name, false)?;
    let _home_anchor = shift_block_pos_field(c, "home_pos", delta, &r.name, false)?;
    let _leash_anchor = shift_block_pos_field(c, "leash", delta, &r.name, true)?;
    if let (Some(modern), Some(legacy)) = (block_anchor, legacy_anchor) {
        if modern != legacy {
            return Err(ConversionError::new(
                Stage::Normalize,
                "entity block_pos and TileXYZ anchors disagree after normalization",
            )
            .in_region(r.name.clone()));
        }
    }
    checked_floor_pos(pos).map_err(|e| e.in_region(r.name.clone()))?;
    Ok((world, pos, block_anchor.or(legacy_anchor), data))
}

fn shift_legacy_triplet(
    compound: &mut HashMap<String, NbtValue>,
    prefix: &str,
    delta: [i64; 3],
    region: &str,
) -> Result<Option<[i32; 3]>, ConversionError> {
    let suffixes = ["X", "Y", "Z"];
    let mut values = [0i32; 3];
    let mut present = [false; 3];
    for axis in 0..3 {
        let key = format!("{prefix}{}", suffixes[axis]);
        if compound.contains_key(&key) {
            values[axis] = shift_int_coordinate(compound, &key, delta[axis], region)?;
            present[axis] = true;
        }
    }
    Ok(present.iter().all(|p| *p).then_some(values))
}

fn shift_int_coordinate(
    compound: &mut HashMap<String, NbtValue>,
    key: &str,
    delta: i64,
    region: &str,
) -> Result<i32, ConversionError> {
    let Some(NbtValue::Int(value)) = compound.get(key) else {
        return Err(ConversionError::new(
            Stage::Normalize,
            format!("entity field {key} must be an Int"),
        )
        .in_region(region.to_string()));
    };
    let shifted = i64::from(*value).checked_add(delta).ok_or_else(|| {
        ConversionError::new(
            Stage::Normalize,
            format!("entity field {key} overflows i64"),
        )
        .in_region(region.to_string())
    })?;
    let shifted = i32::try_from(shifted).map_err(|_| {
        ConversionError::new(Stage::Normalize, format!("entity field {key} exceeds i32"))
            .in_region(region.to_string())
    })?;
    compound.insert(key.to_string(), NbtValue::Int(shifted));
    Ok(shifted)
}

fn shift_block_pos_field(
    compound: &mut HashMap<String, NbtValue>,
    key: &str,
    delta: [i64; 3],
    region: &str,
    allow_leash_references: bool,
) -> Result<Option<[i32; 3]>, ConversionError> {
    let Some(value) = compound.get(key).cloned() else {
        return Ok(None);
    };
    let NbtValue::IntArray(array) = value else {
        if allow_leash_references && matches!(value, NbtValue::Compound(_)) {
            return Ok(None);
        }
        return Err(ConversionError::new(
            Stage::Normalize,
            format!("entity field {key} must use BlockPos CODEC int-array encoding"),
        )
        .in_region(region.to_string()));
    };
    let coords = array.to_vec();
    if allow_leash_references && coords.len() == 4 {
        // A four-int leash payload is a UUID reference, not a BlockPos.
        return Ok(None);
    }
    if coords.len() != 3 {
        return Err(ConversionError::new(
            Stage::Normalize,
            format!("entity coordinate field {key} must contain exactly three ints"),
        )
        .in_region(region.to_string()));
    }
    let mut shifted = [0i32; 3];
    let mut encoded = Vec::with_capacity(3);
    for axis in 0..3 {
        let n = i64::from(coords[axis])
            .checked_add(delta[axis])
            .ok_or_else(|| {
                ConversionError::new(
                    Stage::Normalize,
                    format!("entity coordinate field {key} overflows i64"),
                )
                .in_region(region.to_string())
            })?;
        shifted[axis] = i32::try_from(n).map_err(|_| {
            ConversionError::new(
                Stage::Normalize,
                format!("entity coordinate field {key} exceeds i32"),
            )
            .in_region(region.to_string())
        })?;
        encoded.push(shifted[axis]);
    }
    compound.insert(
        key.to_string(),
        NbtValue::IntArray(fastnbt::IntArray::new(encoded)),
    );
    Ok(Some(shifted))
}

fn checked_floor_i64(value: f64) -> Result<i64, ConversionError> {
    if !value.is_finite() {
        return Err(ConversionError::new(
            Stage::Normalize,
            "entity world position is not finite",
        ));
    }
    let floor = value.floor();
    let limit = 9_223_372_036_854_775_808.0f64;
    if floor < -limit || floor >= limit {
        return Err(ConversionError::new(
            Stage::Normalize,
            "entity world coordinate exceeds i64",
        ));
    }
    Ok(floor as i64)
}

fn compound_mut(v: &mut NbtValue) -> Option<&mut HashMap<String, NbtValue>> {
    if let NbtValue::Compound(c) = v {
        Some(c)
    } else {
        None
    }
}
fn numeric_i64(v: Option<&NbtValue>) -> Option<i64> {
    match v? {
        NbtValue::Byte(x) => Some(*x as i64),
        NbtValue::Short(x) => Some(*x as i64),
        NbtValue::Int(x) => Some(*x as i64),
        NbtValue::Long(x) => Some(*x),
        _ => None,
    }
}
fn numeric_f64(v: &NbtValue) -> Option<f64> {
    match v {
        NbtValue::Byte(x) => Some(*x as f64),
        NbtValue::Short(x) => Some(*x as f64),
        NbtValue::Int(x) => Some(*x as f64),
        NbtValue::Long(x) => Some(*x as f64),
        NbtValue::Float(x) => Some(*x as f64),
        NbtValue::Double(x) => Some(*x),
        _ => None,
    }
}
fn entity_uuid(v: &NbtValue) -> Option<String> {
    if let NbtValue::Compound(c) = v {
        if let Some(x) = c.get("UUID") {
            return Some(format!("{x:?}"));
        }
        if let (Some(a), Some(b)) = (c.get("UUIDMost"), c.get("UUIDLeast")) {
            return Some(format!("{a:?}:{b:?}"));
        }
    }
    None
}
fn checked_floor_pos(pos: [f64; 3]) -> Result<[i32; 3], ConversionError> {
    let mut p = [0; 3];
    for i in 0..3 {
        if !pos[i].is_finite() {
            return Err(ConversionError::new(
                Stage::Normalize,
                "entity position is not finite",
            ));
        }
        let n = pos[i].floor();
        if n < i32::MIN as f64 || n > i32::MAX as f64 {
            return Err(ConversionError::new(
                Stage::Normalize,
                "entity block position exceeds i32",
            ));
        }
        p[i] = n as i32;
    }
    Ok(p)
}
fn relative_pos(p: [i64; 3], origin: [i64; 3]) -> Result<[i32; 3], ConversionError> {
    let mut out = [0; 3];
    for a in 0..3 {
        out[a] = i32::try_from(p[a].checked_sub(origin[a]).ok_or_else(|| {
            ConversionError::new(Stage::Normalize, "relative coordinate overflow")
        })?)
        .map_err(|_| ConversionError::new(Stage::Normalize, "relative coordinate exceeds i32"))?;
    }
    Ok(out)
}
fn to_i32_pos(p: [i64; 3]) -> Result<[i32; 3], ConversionError> {
    let mut out = [0; 3];
    for a in 0..3 {
        out[a] = i32::try_from(p[a])
            .map_err(|_| ConversionError::new(Stage::Normalize, "coordinate exceeds i32"))?;
    }
    Ok(out)
}
