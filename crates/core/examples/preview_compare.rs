use bluepanda_core::{
    convert_file, load_preview, read_gzip_nbt, CancellationToken, ConversionOptions, Structure,
};
use std::collections::BTreeMap;
use std::env;
use std::fs::File;
use std::path::PathBuf;
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args_os().skip(1);
    let input = args
        .next()
        .map(PathBuf::from)
        .ok_or("usage: preview_compare INPUT OUTPUT.nbt")?;
    let output = args
        .next()
        .map(PathBuf::from)
        .ok_or("usage: preview_compare INPUT OUTPUT.nbt")?;
    if args.next().is_some() {
        return Err("usage: preview_compare INPUT OUTPUT.nbt".into());
    }

    let preview_start = Instant::now();
    let preview = load_preview(&input, &CancellationToken::default(), |_| {})?;
    let preview_time = preview_start.elapsed();
    let source_cells: usize = preview
        .regions
        .iter()
        .map(|region| {
            region
                .size
                .iter()
                .map(|side| side.unsigned_abs() as usize)
                .product::<usize>()
        })
        .sum();

    let convert_start = Instant::now();
    convert_file(&input, &output, &ConversionOptions::default(), |_| {})?;
    let convert_time = convert_start.elapsed();
    let structure: Structure = read_gzip_nbt(File::open(&output)?)?;
    let converted = load_preview(&output, &CancellationToken::default(), |_| {})?;

    let source_blocks: BTreeMap<_, _> = preview
        .blocks
        .iter()
        .map(|block| {
            (
                block.position,
                preview.palette[block.state as usize].clone(),
            )
        })
        .collect();
    let converted_blocks: BTreeMap<_, _> = converted
        .blocks
        .iter()
        .map(|block| {
            (
                block.position,
                converted.palette[block.state as usize].clone(),
            )
        })
        .collect();

    println!(
        "input={} format={:?} dimensions={:?} regions={} source_cells={} visible_non_air_blocks={} entities={} warnings={} preview_ms={}",
        input.display(),
        preview.format,
        preview.size,
        preview.regions.len(),
        source_cells,
        preview.blocks.len(),
        preview.entities_count,
        preview.warnings.len(),
        preview_time.as_millis(),
    );
    println!(
        "output={} dimensions={:?} visible_non_air_blocks={} entities={} convert_ms={} state_position_maps_equal={}",
        output.display(),
        structure.size,
        converted.blocks.len(),
        converted.entities_count,
        convert_time.as_millis(),
        source_blocks == converted_blocks,
    );
    for warning in &preview.warnings {
        println!("warning={}", warning.message);
    }
    if preview.size != structure.size
        || preview.blocks.len() != converted.blocks.len()
        || preview.entities_count != converted.entities_count
        || source_blocks != converted_blocks
    {
        return Err("source preview and converted Structure preview differ".into());
    }
    Ok(())
}
