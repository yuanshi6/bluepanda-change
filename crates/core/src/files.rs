use crate::transform::{build_structure_with_warnings, Structure};
use crate::writer::{verify_structure, write_structure};
use crate::{
    BatchFileResult, BatchResult, ConversionError, ConversionOptions, ConversionResult,
    ProgressEvent, Stage,
};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

fn emit(input: &Path, stage: Stage, progress: &mut impl FnMut(ProgressEvent)) {
    progress(ProgressEvent {
        input: input.to_path_buf(),
        stage,
        completed: 0,
        total: 1,
    });
}

/// Convert a single Litematica file to a verified, compressed Structure NBT file.
pub fn convert_file(
    input: &Path,
    output: &Path,
    options: &ConversionOptions,
    mut progress: impl FnMut(ProgressEvent),
) -> Result<ConversionResult, ConversionError> {
    emit(input, Stage::Read, &mut progress);
    let source = File::open(input)
        .map_err(|e| ConversionError::new(Stage::Read, format!("cannot open input: {e}")))?;
    emit(input, Stage::Decompress, &mut progress);
    emit(input, Stage::Parse, &mut progress);
    let parsed = crate::parse_litematic(source)?;

    emit(input, Stage::Decode, &mut progress);
    let transformed = build_structure_with_warnings(&parsed)?;
    emit(input, Stage::Normalize, &mut progress);
    emit(input, Stage::Merge, &mut progress);
    let structure = transformed.structure;

    let output_parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(output_parent).map_err(|e| {
        ConversionError::new(Stage::Write, format!("cannot create output directory: {e}"))
    })?;
    if !options.overwrite && output.exists() {
        return Err(ConversionError::new(
            Stage::Commit,
            "output already exists; enable overwrite to replace it",
        ));
    }
    if let (Ok(src), Ok(dst)) = (fs::canonicalize(input), fs::canonicalize(output)) {
        if src == dst {
            return Err(ConversionError::new(
                Stage::Validate,
                "input and output refer to the same file",
            ));
        }
    }

    emit(input, Stage::Write, &mut progress);
    let mut temporary = NamedTempFile::new_in(output_parent).map_err(|e| {
        ConversionError::new(Stage::Write, format!("cannot create temporary output: {e}"))
    })?;
    write_structure(&mut temporary, &structure)?;
    temporary.flush().map_err(|e| {
        ConversionError::new(Stage::Write, format!("cannot flush temporary output: {e}"))
    })?;
    temporary.as_file().sync_all().map_err(|e| {
        ConversionError::new(Stage::Write, format!("cannot sync temporary output: {e}"))
    })?;

    emit(input, Stage::Verify, &mut progress);
    let readback = temporary.reopen().map_err(|e| {
        ConversionError::new(
            Stage::Verify,
            format!("cannot reopen temporary output: {e}"),
        )
    })?;
    verify_structure(readback, &structure)?;

    emit(input, Stage::Commit, &mut progress);
    if options.overwrite {
        temporary.persist(output).map_err(|e| {
            ConversionError::new(
                Stage::Commit,
                format!("cannot atomically replace output: {}", e.error),
            )
        })?;
    } else {
        temporary.persist_noclobber(output).map_err(|e| {
            let message = if e.error.kind() == std::io::ErrorKind::AlreadyExists {
                "output appeared during conversion; existing file was preserved".to_string()
            } else {
                format!("cannot commit output: {}", e.error)
            };
            ConversionError::new(Stage::Commit, message)
        })?;
    }
    Ok(ConversionResult {
        input: input.to_path_buf(),
        output: output.to_path_buf(),
        blocks: structure.blocks.len(),
        entities: structure.entities.len(),
        warnings: transformed.warnings,
    })
}

fn output_path(input: &Path, dir: &Path) -> PathBuf {
    let mut name = input.file_name().unwrap_or_default().to_os_string();
    name.push(".nbt");
    dir.join(name)
}

fn input_identity(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir().unwrap_or_default().join(path)
        }
    })
}

fn collision_key(path: &Path) -> String {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    let parent = absolute
        .parent()
        .and_then(|p| fs::canonicalize(p).ok())
        .unwrap_or_else(|| absolute.parent().unwrap_or(Path::new(".")).to_path_buf());
    let filename = absolute
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    format!(
        "{}{}{}",
        parent.to_string_lossy().to_lowercase(),
        std::path::MAIN_SEPARATOR,
        filename
    )
}

/// Convert a list sequentially. Duplicate inputs are collapsed and every task targeting the same
/// output name fails before conversion, including when overwrite is enabled.
pub fn convert_batch(
    inputs: &[PathBuf],
    output_dir: &Path,
    options: &ConversionOptions,
    mut progress: impl FnMut(ProgressEvent),
) -> BatchResult {
    let mut unique = Vec::new();
    let mut seen_inputs = HashSet::new();
    for input in inputs {
        if seen_inputs.insert(input_identity(input)) {
            unique.push(input.clone());
        }
    }
    // Canonicalize the destination parent before collision detection; otherwise a not-yet-created
    // directory is compared lexically first and by its resolved path later.
    let output_dir_error = fs::create_dir_all(output_dir).err();
    let outputs: Vec<_> = unique
        .iter()
        .map(|input| output_path(input, output_dir))
        .collect();
    let mut counts = HashMap::new();
    for output in &outputs {
        *counts.entry(collision_key(output)).or_insert(0usize) += 1;
    }
    let colliding: HashSet<_> = counts
        .into_iter()
        .filter_map(|(key, count)| (count > 1).then_some(key))
        .collect();
    let mut results = Vec::with_capacity(unique.len());
    for (input, output) in unique.into_iter().zip(outputs) {
        let result = if let Some(error) = &output_dir_error {
            Err(ConversionError::new(
                Stage::Write,
                format!("cannot create output directory: {error}"),
            ))
        } else if colliding.contains(&collision_key(&output)) {
            Err(ConversionError::new(
                Stage::Validate,
                "multiple inputs map to the same output filename; all colliding tasks were skipped",
            ))
        } else if fs::canonicalize(&input)
            .ok()
            .zip(fs::canonicalize(&output).ok())
            .is_some_and(|(a, b)| a == b)
        {
            Err(ConversionError::new(
                Stage::Validate,
                "input and output refer to the same file",
            ))
        } else {
            convert_file(&input, &output, options, |event| progress(event))
        };
        results.push(BatchFileResult {
            input,
            output,
            result,
        });
    }
    BatchResult { results }
}

#[allow(dead_code)]
fn _assert_structure_is_send_sync(_: &Structure) {}
