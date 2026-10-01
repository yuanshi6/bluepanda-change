use crate::parser::{decode_palette_index, linear_index_to_xyz, parse_schematic_value_preview};
use crate::transform::{Structure, StructureBlock, StructureEntity, StructurePaletteEntry};
use crate::types::{
    BlockState, CancellationToken, ConversionError, ConversionWarning, ParsedSchematic,
    PreviewBlock, PreviewDocument, PreviewFormat, PreviewRegion, PreviewScene, ProgressEvent,
    Region, Stage,
};
use fastnbt::Value;
use flate2::bufread::GzDecoder;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Cursor, Read};
use std::path::Path;

pub const MAX_PREVIEW_NBT_BYTES: usize = 256 * 1024 * 1024;
pub const MAX_PREVIEW_VISIBLE_BLOCKS: usize = 1_000_000;
/// Bounds the additional per-region candidates retained so hidden overlapping regions can be
/// revealed without allowing overlap-heavy inputs to multiply preview memory without limit.
pub const MAX_PREVIEW_REGION_BLOCKS: usize = 1_000_000;

/// Load a local schematic or Minecraft Java Structure file into a normalized sparse preview.
/// This function never writes files or invokes conversion output code.
pub fn load_preview(
    path: &Path,
    cancel: &CancellationToken,
    progress: impl FnMut(ProgressEvent),
) -> Result<PreviewDocument, ConversionError> {
    load_preview_impl(path, cancel, progress, false).map(|scene| scene.document)
}

/// Load a preview document and retain per-region block candidates for interactive region
/// visibility changes. `document.blocks` keeps its historical merged, earlier-region-wins
/// semantics; `region_blocks` includes every non-air region candidate, including overlaps.
pub fn load_preview_scene(
    path: &Path,
    cancel: &CancellationToken,
    progress: impl FnMut(ProgressEvent),
) -> Result<PreviewScene, ConversionError> {
    load_preview_impl(path, cancel, progress, true)
}

fn load_preview_impl(
    path: &Path,
    cancel: &CancellationToken,
    mut progress: impl FnMut(ProgressEvent),
    retain_region_blocks: bool,
) -> Result<PreviewScene, ConversionError> {
    check_cancelled(cancel)?;
    report(path, Stage::Read, 0, 1, &mut progress);
    let file = File::open(path).map_err(|e| {
        ConversionError::new(Stage::Read, format!("cannot open preview input: {e}"))
    })?;
    let compressed_len = file.metadata().map(|m| m.len()).unwrap_or(0) as usize;
    report(path, Stage::Read, 1, 1, &mut progress);

    report(
        path,
        Stage::Decompress,
        0,
        compressed_len.max(1),
        &mut progress,
    );
    let bytes = read_nbt_payload(file, cancel, |done| {
        report(
            path,
            Stage::Decompress,
            done,
            compressed_len.max(1),
            &mut progress,
        )
    })?;
    report(
        path,
        Stage::Decompress,
        bytes.len(),
        bytes.len().max(1),
        &mut progress,
    );
    check_cancelled(cancel)?;

    report(path, Stage::Parse, 0, 1, &mut progress);
    let mut cursor = Cursor::new(bytes.as_slice());
    let root: Value = fastnbt::from_reader(&mut cursor)
        .map_err(|e| ConversionError::new(Stage::Parse, format!("invalid NBT: {e}")))?;
    if cursor.position() != bytes.len() as u64 {
        return Err(ConversionError::new(
            Stage::Parse,
            "trailing bytes after NBT root",
        ));
    }
    check_cancelled(cancel)?;
    report(path, Stage::Parse, 1, 1, &mut progress);

    let structure_input = is_structure_root(&root);
    if structure_input {
        report(path, Stage::Decode, 0, 1, &mut progress);
        let (structure, variants) = parse_structure_value(&root, cancel)?;
        let preview = structure_preview(
            structure,
            variants,
            cancel,
            path,
            &mut progress,
            retain_region_blocks,
        )?;
        report(path, Stage::Decode, 1, 1, &mut progress);
        Ok(preview)
    } else {
        let mut parser_progress =
            |done: usize, total: usize| report(path, Stage::Decode, done, total, &mut progress);
        let (parsed, format) = parse_schematic_value_preview(&root, cancel, &mut parser_progress)?;
        schematic_preview(
            parsed,
            format,
            cancel,
            path,
            &mut progress,
            retain_region_blocks,
        )
    }
}

fn report(
    path: &Path,
    stage: Stage,
    completed: usize,
    total: usize,
    progress: &mut impl FnMut(ProgressEvent),
) {
    progress(ProgressEvent {
        input: path.to_path_buf(),
        stage,
        completed,
        total,
    });
}

fn check_cancelled(cancel: &CancellationToken) -> Result<(), ConversionError> {
    if cancel.is_cancelled() {
        Err(ConversionError::new(Stage::Cancelled, "preview cancelled"))
    } else {
        Ok(())
    }
}

fn read_nbt_payload(
    file: File,
    cancel: &CancellationToken,
    mut report_decompressed: impl FnMut(usize),
) -> Result<Vec<u8>, ConversionError> {
    let mut source = BufReader::new(file);
    let is_gzip = source
        .fill_buf()
        .map_err(|e| ConversionError::new(Stage::Read, format!("cannot inspect input: {e}")))?
        .starts_with(&[0x1f, 0x8b]);
    let mut output = Vec::new();
    if is_gzip {
        let mut decoder = GzDecoder::new(source);
        read_limited(&mut decoder, &mut output, cancel, &mut report_decompressed)?;
        source = decoder.into_inner();
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
    } else {
        read_limited(&mut source, &mut output, cancel, &mut report_decompressed)?;
    }
    Ok(output)
}

fn read_limited(
    reader: &mut impl Read,
    output: &mut Vec<u8>,
    cancel: &CancellationToken,
    report_decompressed: &mut impl FnMut(usize),
) -> Result<(), ConversionError> {
    let mut buffer = [0; 16 * 1024];
    loop {
        check_cancelled(cancel)?;
        let count = reader.read(&mut buffer).map_err(|e| {
            ConversionError::new(Stage::Decompress, format!("cannot read NBT payload: {e}"))
        })?;
        if count == 0 {
            return Ok(());
        }
        let new_len = output.len().checked_add(count).ok_or_else(|| {
            ConversionError::new(Stage::Preview, "preview decompressed NBT size overflow")
        })?;
        if new_len > MAX_PREVIEW_NBT_BYTES {
            return Err(ConversionError::new(
                Stage::Preview,
                format!(
                    "preview decompressed NBT limit exceeded ({} MiB)",
                    MAX_PREVIEW_NBT_BYTES / (1024 * 1024)
                ),
            ));
        }
        output.extend_from_slice(&buffer[..count]);
        report_decompressed(output.len());
    }
}

fn is_structure_root(root: &Value) -> bool {
    let Value::Compound(root) = root else {
        return false;
    };
    root.contains_key("size")
        && (root.contains_key("blocks")
            || root.contains_key("palette")
            || root.contains_key("palettes"))
}

#[derive(Deserialize)]
struct StructureInput {
    size: [i32; 3],
    blocks: Vec<StructureBlock>,
    #[serde(default)]
    entities: Vec<StructureEntity>,
    #[serde(rename = "DataVersion")]
    data_version: Option<i32>,
}

fn parse_structure_value(
    root: &Value,
    cancel: &CancellationToken,
) -> Result<(Structure, usize), ConversionError> {
    let Value::Compound(compound) = root else {
        return Err(ConversionError::new(
            Stage::Preview,
            "Java Structure root must be a compound",
        ));
    };
    validate_structure_list_fields(compound)?;
    if let Some(Value::Int(_)) | None = compound.get("DataVersion") {
    } else {
        return Err(ConversionError::new(
            Stage::Preview,
            "Structure DataVersion must be an Int",
        ));
    }
    check_cancelled(cancel)?;
    let input: StructureInput = fastnbt::from_value(root).map_err(|e| {
        ConversionError::new(Stage::Preview, format!("invalid Java Structure NBT: {e}"))
    })?;
    check_cancelled(cancel)?;
    let (palette, variants) = select_structure_palette(compound, cancel)?;
    let structure = Structure {
        size: input.size,
        palette,
        blocks: input.blocks,
        entities: input.entities,
        data_version: input.data_version,
    };
    crate::writer::validate_structure_at(&structure, Stage::Preview)?;
    let mut positions = HashSet::with_capacity(structure.blocks.len());
    for (index, block) in structure.blocks.iter().enumerate() {
        if index % 16_384 == 0 {
            check_cancelled(cancel)?;
        }
        if !positions.insert(block.pos) {
            return Err(ConversionError::new(
                Stage::Preview,
                "Structure contains duplicate block coordinates",
            )
            .at(block.pos));
        }
    }
    Ok((structure, variants))
}

fn select_structure_palette(
    root: &HashMap<String, Value>,
    cancel: &CancellationToken,
) -> Result<(Vec<StructurePaletteEntry>, usize), ConversionError> {
    let palettes = match root.get("palettes") {
        Some(Value::List(values)) => Some(values),
        Some(_) => {
            return Err(ConversionError::new(
                Stage::Preview,
                "Structure palettes must be a List of palette Lists",
            ))
        }
        None => None,
    };
    let variants = palettes.map_or(1, Vec::len);
    let selected = if let Some(palette) = root.get("palette") {
        palette
    } else {
        palettes.and_then(|values| values.first()).ok_or_else(|| {
            ConversionError::new(Stage::Preview, "Structure is missing palette or palettes")
        })?
    };
    check_cancelled(cancel)?;
    let palette = fastnbt::from_value(selected).map_err(|e| {
        ConversionError::new(Stage::Preview, format!("invalid Structure palette: {e}"))
    })?;
    check_cancelled(cancel)?;
    Ok((palette, variants))
}

fn validate_structure_list_fields(root: &HashMap<String, Value>) -> Result<(), ConversionError> {
    let validate_vec3 = |value: Option<&Value>, field: &str| -> Result<(), ConversionError> {
        let Some(Value::List(values)) = value else {
            return Err(ConversionError::new(
                Stage::Preview,
                format!("Structure {field} must be a List<Int> of length 3"),
            ));
        };
        if values.len() != 3 || values.iter().any(|v| !matches!(v, Value::Int(_))) {
            return Err(ConversionError::new(
                Stage::Preview,
                format!("Structure {field} must be a List<Int> of length 3"),
            ));
        }
        Ok(())
    };
    validate_vec3(root.get("size"), "size")?;
    let Some(Value::List(blocks)) = root.get("blocks") else {
        return Err(ConversionError::new(
            Stage::Preview,
            "Structure blocks must be a List",
        ));
    };
    if blocks.len() > MAX_PREVIEW_VISIBLE_BLOCKS {
        return Err(ConversionError::new(
            Stage::Preview,
            format!("preview Structure block record limit exceeded ({MAX_PREVIEW_VISIBLE_BLOCKS})"),
        ));
    }
    for (index, block) in blocks.iter().enumerate() {
        let Value::Compound(block) = block else {
            return Err(ConversionError::new(
                Stage::Preview,
                format!("Structure blocks[{index}] must be a compound"),
            ));
        };
        validate_vec3(block.get("pos"), &format!("blocks[{index}].pos"))?;
    }
    if let Some(entities) = root.get("entities") {
        let Value::List(entities) = entities else {
            return Err(ConversionError::new(
                Stage::Preview,
                "Structure entities must be a List",
            ));
        };
        for (index, entity) in entities.iter().enumerate() {
            let Value::Compound(entity) = entity else {
                return Err(ConversionError::new(
                    Stage::Preview,
                    format!("Structure entities[{index}] must be a compound"),
                ));
            };
            validate_vec3(
                entity.get("blockPos"),
                &format!("entities[{index}].blockPos"),
            )?;
        }
    }
    Ok(())
}

fn structure_preview(
    structure: Structure,
    variants: usize,
    cancel: &CancellationToken,
    path: &Path,
    progress: &mut impl FnMut(ProgressEvent),
    retain_region_blocks: bool,
) -> Result<PreviewScene, ConversionError> {
    check_cancelled(cancel)?;
    let size = structure.size;
    let data_version = structure.data_version;
    let block_entities_count = structure
        .blocks
        .iter()
        .filter(|block| block.nbt.is_some())
        .count();
    let mut palette = Vec::with_capacity(structure.palette.len());
    let mut palette_ids = HashMap::<BlockState, u32>::new();
    let mut remap = Vec::with_capacity(structure.palette.len());
    for entry in structure.palette {
        let state = BlockState {
            name: entry.name,
            properties: entry.properties,
        };
        let id = palette_id(&state, &mut palette, &mut palette_ids)?;
        remap.push(id);
    }
    let mut blocks = Vec::new();
    let mut region_blocks = Vec::new();
    let total_records = structure.blocks.len();
    for (index, block) in structure.blocks.into_iter().enumerate() {
        if index % 16_384 == 0 {
            check_cancelled(cancel)?;
            report(path, Stage::Decode, index, total_records.max(1), progress);
        }
        let state = remap[block.state as usize];
        if is_air(&palette[state as usize]) {
            continue;
        }
        if blocks.len() >= MAX_PREVIEW_VISIBLE_BLOCKS {
            return Err(ConversionError::new(
                Stage::Preview,
                format!("preview visible block limit exceeded ({MAX_PREVIEW_VISIBLE_BLOCKS})"),
            ));
        }
        let preview_block = PreviewBlock {
            position: block.pos,
            state,
            region_id: 0,
        };
        if retain_region_blocks && region_blocks.len() >= MAX_PREVIEW_REGION_BLOCKS {
            return Err(ConversionError::new(
                Stage::Preview,
                format!(
                    "preview region block candidate limit exceeded ({MAX_PREVIEW_REGION_BLOCKS})"
                ),
            ));
        }
        if retain_region_blocks {
            region_blocks.push(preview_block.clone());
        }
        blocks.push(preview_block);
    }
    blocks.sort_by_key(|b| (b.position[1], b.position[0], b.position[2]));
    let visible = blocks.len();
    report(
        path,
        Stage::Decode,
        total_records,
        total_records.max(1),
        progress,
    );
    let mut warnings = Vec::new();
    if variants > 1 {
        warnings.push(ConversionWarning::new(format!(
            "Structure contains {variants} palette variants; preview shows the first variant"
        )));
    }
    let document = PreviewDocument {
        format: PreviewFormat::JavaStructure,
        origin: [0; 3],
        size,
        palette,
        regions: vec![PreviewRegion {
            id: 0,
            name: "Java Structure".into(),
            source_origin: [0; 3],
            position: [0; 3],
            size,
            block_count: visible,
            entities_count: structure.entities.len(),
        }],
        blocks,
        entities_count: structure.entities.len(),
        warnings,
    };
    Ok(PreviewScene {
        document,
        data_version,
        block_entities_count,
        region_blocks,
    })
}

fn schematic_preview(
    parsed: ParsedSchematic,
    format: PreviewFormat,
    cancel: &CancellationToken,
    path: &Path,
    progress: &mut impl FnMut(ProgressEvent),
    retain_region_blocks: bool,
) -> Result<PreviewScene, ConversionError> {
    check_cancelled(cancel)?;
    if parsed.regions.is_empty() {
        return Err(ConversionError::new(
            Stage::Preview,
            "schematic contains no regions",
        ));
    }
    let mut regions: Vec<&Region> = parsed.regions.iter().collect();
    regions.sort_by(|a, b| a.name.cmp(&b.name));
    let bounds: Vec<_> = regions
        .iter()
        .map(|region| region_bounds(region))
        .collect::<Result<_, _>>()?;
    let mut origin = [i64::MAX; 3];
    let mut max = [i64::MIN; 3];
    for (_, min, end) in &bounds {
        for axis in 0..3 {
            origin[axis] = origin[axis].min(min[axis]);
            max[axis] = max[axis].max(end[axis]);
        }
    }
    let mut size = [0i32; 3];
    for axis in 0..3 {
        size[axis] = i32::try_from(
            max[axis]
                .checked_sub(origin[axis])
                .and_then(|v| v.checked_add(1))
                .ok_or_else(|| ConversionError::new(Stage::Preview, "preview size overflow"))?,
        )
        .map_err(|_| ConversionError::new(Stage::Preview, "preview dimension exceeds i32 range"))?;
    }

    let mut palette = Vec::new();
    let mut palette_ids = HashMap::<BlockState, u32>::new();
    let mut region_palettes = Vec::with_capacity(regions.len());
    for region in &regions {
        let ids = region
            .palette
            .iter()
            .map(|state| palette_id(state, &mut palette, &mut palette_ids))
            .collect::<Result<Vec<_>, _>>()?;
        region_palettes.push(ids);
    }
    let mut output_regions = Vec::with_capacity(regions.len());
    let mut blocks = HashMap::<[i32; 3], PreviewBlock>::new();
    let mut overlap_count = vec![0usize; regions.len()];
    let mut conflict_count = vec![0usize; regions.len()];
    let mut source_non_air_counts = vec![0usize; regions.len()];
    let mut block_entities_count = 0usize;
    let mut region_blocks = Vec::new();
    let mut total_cells = 0usize;
    for (region_index, (region, (dims, min, _))) in regions.iter().zip(&bounds).enumerate() {
        block_entities_count = block_entities_count
            .checked_add(region.block_entities.len())
            .ok_or_else(|| {
                ConversionError::new(Stage::Preview, "preview block entity count overflow")
            })?;
        let volume = region_volume_checked(*dims)?;
        total_cells = total_cells
            .checked_add(volume)
            .ok_or_else(|| ConversionError::new(Stage::Preview, "preview cell count overflow"))?;
        if total_cells > crate::parser::PREVIEW_MAX_SOURCE_CELLS {
            return Err(ConversionError::new(
                Stage::Preview,
                format!(
                    "preview source cell limit exceeded ({})",
                    crate::parser::PREVIEW_MAX_SOURCE_CELLS
                ),
            ));
        }
        for index in 0..volume {
            if index % 16_384 == 0 {
                check_cancelled(cancel)?;
                report(path, Stage::Decode, index, volume, progress);
            }
            let palette_index = decode_palette_index(region, index)?;
            let state_id = *region_palettes[region_index]
                .get(palette_index)
                .ok_or_else(|| {
                    ConversionError::new(
                        Stage::Preview,
                        "palette index out of range during preview",
                    )
                })?;
            let state = &palette[state_id as usize];
            if is_air(state) {
                continue;
            }
            source_non_air_counts[region_index] += 1;
            let xyz = linear_index_to_xyz(index, region.size);
            let world = [
                min[0] + i64::from(xyz[0]),
                min[1] + i64::from(xyz[1]),
                min[2] + i64::from(xyz[2]),
            ];
            let mut relative = [0i32; 3];
            for axis in 0..3 {
                relative[axis] =
                    i32::try_from(world[axis].checked_sub(origin[axis]).ok_or_else(|| {
                        ConversionError::new(Stage::Preview, "preview coordinate overflow")
                    })?)
                    .map_err(|_| {
                        ConversionError::new(
                            Stage::Preview,
                            "preview block coordinate exceeds i32 range",
                        )
                    })?;
            }
            if retain_region_blocks && region_blocks.len() >= MAX_PREVIEW_REGION_BLOCKS {
                return Err(ConversionError::new(
                    Stage::Preview,
                    format!("preview region block candidate limit exceeded ({MAX_PREVIEW_REGION_BLOCKS})"),
                ));
            }
            if retain_region_blocks {
                region_blocks.push(PreviewBlock {
                    position: relative,
                    state: state_id,
                    region_id: region_index as u32,
                });
            }
            if let Some(previous) = blocks.get(&relative) {
                overlap_count[region_index] += 1;
                if previous.state != state_id {
                    conflict_count[region_index] += 1;
                }
                continue;
            }
            if blocks.len() >= MAX_PREVIEW_VISIBLE_BLOCKS {
                return Err(ConversionError::new(
                    Stage::Preview,
                    format!("preview visible block limit exceeded ({MAX_PREVIEW_VISIBLE_BLOCKS})"),
                ));
            }
            blocks.insert(
                relative,
                PreviewBlock {
                    position: relative,
                    state: state_id,
                    region_id: region_index as u32,
                },
            );
        }
        let mut relative_position = [0i32; 3];
        for axis in 0..3 {
            relative_position[axis] = i32::try_from(min[axis] - origin[axis]).map_err(|_| {
                ConversionError::new(Stage::Preview, "preview region position exceeds i32 range")
            })?;
        }
        output_regions.push(PreviewRegion {
            id: region_index as u32,
            name: region.name.clone(),
            source_origin: [
                i64::from(region.position[0]),
                i64::from(region.position[1]),
                i64::from(region.position[2]),
            ],
            position: relative_position,
            size: [
                i32::try_from(dims[0]).map_err(|_| {
                    ConversionError::new(Stage::Preview, "region dimension exceeds i32")
                })?,
                i32::try_from(dims[1]).map_err(|_| {
                    ConversionError::new(Stage::Preview, "region dimension exceeds i32")
                })?,
                i32::try_from(dims[2]).map_err(|_| {
                    ConversionError::new(Stage::Preview, "region dimension exceeds i32")
                })?,
            ],
            block_count: source_non_air_counts[region_index],
            entities_count: region.entities.len(),
        });
    }
    let mut warnings = parsed.warnings;
    for (index, region) in output_regions.iter().enumerate() {
        if overlap_count[index] > 0 {
            warnings.push(
                ConversionWarning::new(format!(
                    "{} overlapping blocks; {} had conflicting states; earlier region wins",
                    overlap_count[index], conflict_count[index]
                ))
                .in_region(region.name.clone()),
            );
        }
    }
    let mut blocks: Vec<_> = blocks.into_values().collect();
    blocks.sort_by_key(|b| (b.position[1], b.position[0], b.position[2]));
    let entities_count = regions.iter().map(|region| region.entities.len()).sum();
    let document = PreviewDocument {
        format,
        origin,
        size,
        palette,
        regions: output_regions,
        blocks,
        entities_count,
        warnings,
    };
    Ok(PreviewScene {
        document,
        data_version: parsed.data_version,
        block_entities_count,
        region_blocks,
    })
}

fn region_bounds(region: &Region) -> Result<([i64; 3], [i64; 3], [i64; 3]), ConversionError> {
    let mut min = [0; 3];
    let mut end = [0; 3];
    let mut dims = [0; 3];
    for axis in 0..3 {
        let size = i64::from(region.size[axis]);
        if size == 0 {
            return Err(ConversionError::new(Stage::Preview, "region has zero size")
                .in_region(region.name.clone()));
        }
        let position = i64::from(region.position[axis]);
        let endpoint = position
            .checked_add(if size > 0 { size - 1 } else { size + 1 })
            .ok_or_else(|| ConversionError::new(Stage::Preview, "region endpoint overflow"))?;
        min[axis] = position.min(endpoint);
        end[axis] = position.max(endpoint);
        dims[axis] = size.abs();
    }
    Ok((dims, min, end))
}

fn region_volume_checked(dims: [i64; 3]) -> Result<usize, ConversionError> {
    dims.into_iter()
        .try_fold(1usize, |n, dim| n.checked_mul(usize::try_from(dim).ok()?))
        .ok_or_else(|| ConversionError::new(Stage::Preview, "region volume overflow"))
}

fn palette_id(
    state: &BlockState,
    palette: &mut Vec<BlockState>,
    ids: &mut HashMap<BlockState, u32>,
) -> Result<u32, ConversionError> {
    if let Some(id) = ids.get(state) {
        return Ok(*id);
    }
    let id = u32::try_from(palette.len())
        .map_err(|_| ConversionError::new(Stage::Preview, "preview palette exceeds u32 range"))?;
    palette.push(state.clone());
    ids.insert(state.clone(), id);
    Ok(id)
}

fn is_air(state: &BlockState) -> bool {
    matches!(
        state.name.as_str(),
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use fastnbt::{LongArray, Value};
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::collections::HashMap;
    use std::io::Write;

    fn compound(entries: impl IntoIterator<Item = (String, Value)>) -> Value {
        Value::Compound(entries.into_iter().collect::<HashMap<_, _>>())
    }

    fn write_value(path: &Path, value: &Value, gzip: bool) {
        let mut raw = Vec::new();
        fastnbt::to_writer(&mut raw, value).unwrap();
        if gzip {
            let mut encoder = GzEncoder::new(File::create(path).unwrap(), Compression::default());
            encoder.write_all(&raw).unwrap();
            encoder.finish().unwrap();
        } else {
            std::fs::write(path, raw).unwrap();
        }
    }

    fn structure_root(pos: [i32; 3]) -> Value {
        let state = compound([("Name".into(), Value::String("minecraft:stone".into()))]);
        let block = compound([
            (
                "pos".into(),
                Value::List(pos.into_iter().map(Value::Int).collect()),
            ),
            ("state".into(), Value::Int(0)),
        ]);
        compound([
            (
                "size".into(),
                Value::List(vec![Value::Int(1), Value::Int(1), Value::Int(1)]),
            ),
            ("palette".into(), Value::List(vec![state])),
            ("blocks".into(), Value::List(vec![block])),
        ])
    }

    fn litematic_region(name: &str, state_name: &str, pos: [i32; 3]) -> (String, Value) {
        let state = compound([("Name".into(), Value::String(state_name.into()))]);
        let position = compound([
            ("x".into(), Value::Int(pos[0])),
            ("y".into(), Value::Int(pos[1])),
            ("z".into(), Value::Int(pos[2])),
        ]);
        let size = compound([
            ("x".into(), Value::Int(1)),
            ("y".into(), Value::Int(1)),
            ("z".into(), Value::Int(1)),
        ]);
        (
            name.into(),
            compound([
                ("Position".into(), position),
                ("Size".into(), size),
                ("BlockStatePalette".into(), Value::List(vec![state])),
                (
                    "BlockStates".into(),
                    Value::LongArray(LongArray::new(vec![0])),
                ),
                ("TileEntities".into(), Value::List(vec![])),
                ("Entities".into(), Value::List(vec![])),
            ]),
        )
    }

    fn litematic_root() -> Value {
        compound([
            ("Version".into(), Value::Int(6)),
            ("SubVersion".into(), Value::Int(1)),
            (
                "Regions".into(),
                compound([
                    litematic_region("B", "minecraft:dirt", [0, 0, 0]),
                    litematic_region("A", "minecraft:stone", [0, 0, 0]),
                ]),
            ),
        ])
    }

    fn sponge_root(version: i32, include_data_version: bool) -> Value {
        let mut entries = vec![
            ("Version".into(), Value::Int(version)),
            ("Width".into(), Value::Short(1)),
            ("Height".into(), Value::Short(1)),
            ("Length".into(), Value::Short(1)),
            (
                "Palette".into(),
                compound([("minecraft:stone".into(), Value::Int(0))]),
            ),
            (
                "BlockData".into(),
                Value::ByteArray(fastnbt::ByteArray::new(vec![0])),
            ),
        ];
        if include_data_version {
            entries.push(("DataVersion".into(), Value::Int(3700)));
        }
        compound(entries)
    }

    fn sponge_v3_root() -> Value {
        let schematic = compound([
            ("Version".into(), Value::Int(3)),
            ("Width".into(), Value::Short(1)),
            ("Height".into(), Value::Short(1)),
            ("Length".into(), Value::Short(1)),
            ("DataVersion".into(), Value::Int(3700)),
            (
                "Blocks".into(),
                compound([
                    (
                        "Palette".into(),
                        compound([("minecraft:stone".into(), Value::Int(0))]),
                    ),
                    (
                        "Data".into(),
                        Value::ByteArray(fastnbt::ByteArray::new(vec![0])),
                    ),
                ]),
            ),
        ]);
        compound([("Schematic".into(), schematic)])
    }

    #[test]
    fn reads_raw_and_gzip_structure_lists_and_omitted_entities() {
        let dir = tempfile::tempdir().unwrap();
        for (name, gzip) in [("raw.nbt", false), ("gzip.nbt", true)] {
            let path = dir.path().join(name);
            write_value(&path, &structure_root([0, 0, 0]), gzip);
            let preview = load_preview(&path, &CancellationToken::default(), |_| {}).unwrap();
            assert_eq!(preview.format, PreviewFormat::JavaStructure);
            assert_eq!(preview.size, [1, 1, 1]);
            assert_eq!(preview.blocks.len(), 1);
            assert_eq!(
                preview.palette[preview.blocks[0].state as usize].name,
                "minecraft:stone"
            );
            assert_eq!(preview.entities_count, 0);
        }
    }

    #[test]
    fn structure_palette_variants_use_first_and_duplicate_positions_fail() {
        let dir = tempfile::tempdir().unwrap();
        let root = structure_root([0, 0, 0]);
        let Value::Compound(mut root) = root else {
            unreachable!()
        };
        root.remove("palette");
        let first = Value::List(vec![compound([(
            "Name".into(),
            Value::String("minecraft:stone".into()),
        )])]);
        let second = Value::List(vec![compound([(
            "Name".into(),
            Value::String("minecraft:dirt".into()),
        )])]);
        root.insert("palettes".into(), Value::List(vec![first, second]));
        let path = dir.path().join("variants.nbt");
        write_value(&path, &Value::Compound(root), false);
        let preview = load_preview(&path, &CancellationToken::default(), |_| {}).unwrap();
        assert_eq!(
            preview.palette[preview.blocks[0].state as usize].name,
            "minecraft:stone"
        );
        assert!(preview
            .warnings
            .iter()
            .any(|w| w.message.contains("variant")));

        let mut duplicate = structure_root([0, 0, 0]);
        if let Value::Compound(ref mut c) = duplicate {
            let Value::List(ref mut blocks) = c.get_mut("blocks").unwrap() else {
                unreachable!()
            };
            blocks.push(blocks[0].clone());
        }
        write_value(&path, &duplicate, false);
        let error = load_preview(&path, &CancellationToken::default(), |_| {}).unwrap_err();
        assert_eq!(error.stage, Stage::Preview);
        assert!(error.message.contains("duplicate"));
    }

    #[test]
    fn rejects_structure_position_outside_dimensions_and_nonfinite_entity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.nbt");
        write_value(&path, &structure_root([1, 0, 0]), false);
        assert!(load_preview(&path, &CancellationToken::default(), |_| {}).is_err());
        let mut root = structure_root([0, 0, 0]);
        if let Value::Compound(ref mut c) = root {
            c.insert(
                "entities".into(),
                Value::List(vec![compound([
                    (
                        "pos".into(),
                        Value::List(vec![
                            Value::Double(f64::NAN),
                            Value::Double(0.5),
                            Value::Double(0.5),
                        ]),
                    ),
                    (
                        "blockPos".into(),
                        Value::List(vec![Value::Int(0), Value::Int(0), Value::Int(0)]),
                    ),
                    (
                        "nbt".into(),
                        compound([("id".into(), Value::String("minecraft:pig".into()))]),
                    ),
                ])]),
            );
        }
        write_value(&path, &root, false);
        assert_eq!(
            load_preview(&path, &CancellationToken::default(), |_| {})
                .unwrap_err()
                .stage,
            Stage::Preview
        );
    }

    #[test]
    fn cancellation_and_trailing_nbt_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cancel.nbt");
        write_value(&path, &structure_root([0, 0, 0]), false);
        let token = CancellationToken::default();
        token.cancel();
        assert_eq!(
            load_preview(&path, &token, |_| {}).unwrap_err().stage,
            Stage::Cancelled
        );
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, [bytes.as_slice(), &[0]].concat()).unwrap();
        assert_eq!(
            load_preview(&path, &CancellationToken::default(), |_| {})
                .unwrap_err()
                .stage,
            Stage::Parse
        );
    }

    #[test]
    fn previews_litematic_regions_in_stable_order_and_warns_on_conflicts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("multi.litematic");
        write_value(&path, &litematic_root(), true);
        let preview = load_preview(&path, &CancellationToken::default(), |_| {}).unwrap();
        assert_eq!(preview.format, PreviewFormat::Litematica);
        assert_eq!(
            preview
                .regions
                .iter()
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>(),
            vec!["A", "B"]
        );
        assert_eq!(preview.blocks.len(), 1);
        assert_eq!(
            preview.regions[preview.blocks[0].region_id as usize].name,
            "A"
        );
        assert_eq!(
            preview.palette[preview.blocks[0].state as usize].name,
            "minecraft:stone"
        );
        assert!(preview
            .warnings
            .iter()
            .any(|warning| warning.message.contains("conflicting states")));
    }

    #[test]
    fn previews_sponge_v1_v2_and_v3_through_shared_data_model() {
        let dir = tempfile::tempdir().unwrap();
        for (name, root, expected) in [
            ("v1.schem", sponge_root(1, false), PreviewFormat::SpongeV1V2),
            ("v2.schem", sponge_root(2, true), PreviewFormat::SpongeV1V2),
            ("v3.schem", sponge_v3_root(), PreviewFormat::SpongeV3),
        ] {
            let path = dir.path().join(name);
            write_value(&path, &root, true);
            let preview = load_preview(&path, &CancellationToken::default(), |_| {}).unwrap();
            assert_eq!(preview.format, expected);
            assert_eq!(preview.blocks.len(), 1);
            assert_eq!(
                preview.palette[preview.blocks[0].state as usize].name,
                "minecraft:stone"
            );
        }
    }

    #[test]
    fn preview_cell_limit_is_checked_before_sponge_decode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oversized.schem");
        let root = compound([
            ("Version".into(), Value::Int(1)),
            ("Width".into(), Value::Short(257)),
            ("Height".into(), Value::Short(256)),
            ("Length".into(), Value::Short(256)),
            (
                "Palette".into(),
                compound([("minecraft:air".into(), Value::Int(0))]),
            ),
            (
                "BlockData".into(),
                Value::ByteArray(fastnbt::ByteArray::new(vec![])),
            ),
        ]);
        write_value(&path, &root, true);
        let error = load_preview(&path, &CancellationToken::default(), |_| {}).unwrap_err();
        assert_eq!(error.stage, Stage::Preview);
        assert!(error.message.contains("source cell limit"));
    }

    #[test]
    fn negative_litematic_extents_keep_full_bounding_box() {
        let region = Region {
            name: "negative-x".into(),
            position: [5, 0, 0],
            size: [-3, 1, 1],
            palette: vec![BlockState {
                name: "minecraft:stone".into(),
                properties: Default::default(),
            }],
            packed_states: vec![0],
            block_entities: vec![],
            entities: vec![],
        };
        let (dims, min, max) = region_bounds(&region).unwrap();
        assert_eq!(dims, [3, 1, 1]);
        assert_eq!(min, [3, 0, 0]);
        assert_eq!(max, [5, 0, 0]);
    }

    #[test]
    fn negative_extents_on_all_axes_and_region_union_normalize_consistently() {
        let state = BlockState {
            name: "minecraft:stone".into(),
            properties: Default::default(),
        };
        let parsed = ParsedSchematic {
            data_version: None,
            regions: vec![
                Region {
                    name: "Negative".into(),
                    position: [5, 7, 9],
                    size: [-3, -2, -4],
                    palette: vec![state.clone()],
                    packed_states: vec![0],
                    block_entities: vec![],
                    entities: vec![],
                },
                Region {
                    name: "Remote".into(),
                    position: [10, 4, 20],
                    size: [1, 1, 1],
                    palette: vec![state],
                    packed_states: vec![0],
                    block_entities: vec![],
                    entities: vec![],
                },
            ],
            warnings: vec![],
        };
        let preview = schematic_preview(
            parsed,
            PreviewFormat::Litematica,
            &CancellationToken::default(),
            Path::new("synthetic.litematic"),
            &mut |_| {},
            true,
        )
        .unwrap()
        .document;
        assert_eq!(preview.origin, [3, 4, 6]);
        assert_eq!(preview.size, [8, 4, 15]);
        assert_eq!(preview.regions[0].position, [0, 2, 0]);
        assert_eq!(preview.regions[0].size, [3, 2, 4]);
        assert_eq!(preview.regions[1].position, [7, 0, 14]);
        assert_eq!(preview.blocks.len(), 25);
        assert!(preview.blocks.iter().all(|block| {
            (0..3)
                .all(|axis| block.position[axis] >= 0 && block.position[axis] < preview.size[axis])
        }));
    }

    #[test]
    fn structure_coordinates_require_standard_list_of_three_ints() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("array-coords.nbt");
        let mut root = structure_root([0, 0, 0]);
        if let Value::Compound(ref mut compound) = root {
            compound.insert(
                "size".into(),
                Value::IntArray(fastnbt::IntArray::new(vec![1, 1, 1])),
            );
        }
        write_value(&path, &root, false);
        let error = load_preview(&path, &CancellationToken::default(), |_| {}).unwrap_err();
        assert_eq!(error.stage, Stage::Preview);
        assert!(error.message.contains("List<Int>"));
    }
}
