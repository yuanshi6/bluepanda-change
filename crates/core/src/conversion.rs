//! Conversion entry points that report stage progress and per-file diagnostics.

use crate::transform::{build_structure_with_progress, Structure};
use crate::writer::{verify_structure_with_progress, write_structure};
use crate::{
    BatchFileResult, BatchResult, ConversionError, ConversionOptions, ConversionResult,
    ConversionWarning, ProgressEvent, Stage,
};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tempfile::NamedTempFile;

/// Events emitted while converting one or more files.
#[derive(Debug, Clone)]
pub enum ConversionEvent {
    /// A stage update. `completed / total` is meaningful when `total > 1`.
    Progress(ProgressEvent),
    /// A conversion warning, emitted as soon as it is known, including before later failure.
    Warning {
        input: PathBuf,
        warning: ConversionWarning,
    },
    /// Final outcome for an input. A batch emits exactly one for every unique input.
    FileFinished(FileFinished),
    /// Emitted once after all per-file results in a batch have been reported.
    BatchFinished { completed: usize, failed: usize },
}

/// Terminal conversion information for one input.
#[derive(Debug, Clone)]
pub struct FileFinished {
    pub input: PathBuf,
    pub output: PathBuf,
    pub result: Result<ConversionResult, ConversionError>,
    /// Warnings collected before completion or failure.
    pub warnings: Vec<ConversionWarning>,
    pub elapsed: Duration,
}

/// Convert one schematic and emit progress, warning, and final-result events.
pub fn convert_file_with_events(
    input: &Path,
    output: &Path,
    options: &ConversionOptions,
    mut event: impl FnMut(ConversionEvent),
) -> Result<ConversionResult, ConversionError> {
    let started = Instant::now();
    let mut warnings = Vec::new();
    let result = convert_one(input, output, options, &mut warnings, &mut event);
    event(ConversionEvent::FileFinished(FileFinished {
        input: input.to_path_buf(),
        output: output.to_path_buf(),
        result: result.clone(),
        warnings,
        elapsed: started.elapsed(),
    }));
    result
}

/// Convert a list sequentially, emitting terminal events even when individual files fail.
pub fn convert_batch_with_events(
    inputs: &[PathBuf],
    output_dir: &Path,
    options: &ConversionOptions,
    mut event: impl FnMut(ConversionEvent),
) -> BatchResult {
    let mut unique = Vec::new();
    let mut seen_inputs = HashSet::new();
    for input in inputs {
        if seen_inputs.insert(input_identity(input)) {
            unique.push(input.clone());
        }
    }

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
        let started = Instant::now();
        let mut warnings = Vec::new();
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
            .is_some_and(|(source, destination)| source == destination)
        {
            Err(ConversionError::new(
                Stage::Validate,
                "input and output refer to the same file",
            ))
        } else {
            convert_one(&input, &output, options, &mut warnings, &mut event)
        };

        event(ConversionEvent::FileFinished(FileFinished {
            input: input.clone(),
            output: output.clone(),
            result: result.clone(),
            warnings,
            elapsed: started.elapsed(),
        }));
        results.push(BatchFileResult {
            input,
            output,
            result,
        });
    }
    let completed = results.iter().filter(|file| file.result.is_ok()).count();
    let failed = results.len().saturating_sub(completed);
    event(ConversionEvent::BatchFinished { completed, failed });
    BatchResult { results }
}

fn convert_one(
    input: &Path,
    output: &Path,
    options: &ConversionOptions,
    warnings: &mut Vec<ConversionWarning>,
    event: &mut impl FnMut(ConversionEvent),
) -> Result<ConversionResult, ConversionError> {
    emit(input, Stage::Read, 0, 1, event);
    let source = File::open(input).map_err(|error| {
        ConversionError::new(Stage::Read, format!("cannot open input: {error}"))
    })?;
    emit(input, Stage::Decompress, 0, 1, event);
    emit(input, Stage::Parse, 0, 1, event);
    let parser_event = std::cell::RefCell::new(&mut *event);
    let parsed = crate::parser::parse_schematic_with_events(
        source,
        |completed, total| {
            emit(
                input,
                Stage::Decode,
                completed,
                total,
                &mut **parser_event.borrow_mut(),
            )
        },
        |warning| {
            warnings.push(warning.clone());
            (**parser_event.borrow_mut())(ConversionEvent::Warning {
                input: input.to_path_buf(),
                warning,
            });
        },
    )?;
    drop(parser_event);
    emit(input, Stage::Parse, 1, 1, event);

    emit(input, Stage::Merge, 0, 1, event);
    let parsed_warning_count = parsed.warnings.len();
    let transformed = build_structure_with_progress(&parsed, |completed, total| {
        emit(input, Stage::Merge, completed, total, event)
    })?;
    for warning in transformed.warnings.iter().skip(parsed_warning_count) {
        warnings.push(warning.clone());
        event(ConversionEvent::Warning {
            input: input.to_path_buf(),
            warning: warning.clone(),
        });
    }
    let structure = transformed.structure;

    let output_parent = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(output_parent).map_err(|error| {
        ConversionError::new(
            Stage::Write,
            format!("cannot create output directory: {error}"),
        )
    })?;
    if !options.overwrite && output.exists() {
        return Err(ConversionError::new(
            Stage::Commit,
            "output already exists; enable overwrite to replace it",
        ));
    }
    if let (Ok(source), Ok(destination)) = (fs::canonicalize(input), fs::canonicalize(output)) {
        if source == destination {
            return Err(ConversionError::new(
                Stage::Validate,
                "input and output refer to the same file",
            ));
        }
    }

    emit(input, Stage::Write, 0, 1, event);
    let mut temporary = NamedTempFile::new_in(output_parent).map_err(|error| {
        ConversionError::new(
            Stage::Write,
            format!("cannot create temporary output: {error}"),
        )
    })?;
    write_structure(&mut temporary, &structure)?;
    temporary.flush().map_err(|error| {
        ConversionError::new(
            Stage::Write,
            format!("cannot flush temporary output: {error}"),
        )
    })?;
    temporary.as_file().sync_all().map_err(|error| {
        ConversionError::new(
            Stage::Write,
            format!("cannot sync temporary output: {error}"),
        )
    })?;
    emit(input, Stage::Write, 1, 1, event);

    emit(input, Stage::Verify, 0, 1, event);
    let readback = temporary.reopen().map_err(|error| {
        ConversionError::new(
            Stage::Verify,
            format!("cannot reopen temporary output: {error}"),
        )
    })?;
    verify_structure_with_progress(readback, &structure, |completed, total| {
        emit(input, Stage::Verify, completed, total, event)
    })?;
    emit(input, Stage::Verify, 1, 1, event);

    emit(input, Stage::Commit, 0, 1, event);
    if options.overwrite {
        temporary.persist(output).map_err(|error| {
            ConversionError::new(
                Stage::Commit,
                format!("cannot atomically replace output: {}", error.error),
            )
        })?;
    } else {
        temporary.persist_noclobber(output).map_err(|error| {
            let message = if error.error.kind() == std::io::ErrorKind::AlreadyExists {
                "output appeared during conversion; existing file was preserved".to_string()
            } else {
                format!("cannot commit output: {}", error.error)
            };
            ConversionError::new(Stage::Commit, message)
        })?;
    }
    emit(input, Stage::Commit, 1, 1, event);

    Ok(ConversionResult {
        input: input.to_path_buf(),
        output: output.to_path_buf(),
        blocks: structure.blocks.len(),
        entities: structure.entities.len(),
        warnings: transformed.warnings,
    })
}

fn emit(
    input: &Path,
    stage: Stage,
    completed: usize,
    total: usize,
    event: &mut impl FnMut(ConversionEvent),
) {
    event(ConversionEvent::Progress(ProgressEvent {
        input: input.to_path_buf(),
        stage,
        completed,
        total: total.max(1),
    }));
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
        .and_then(|parent| fs::canonicalize(parent).ok())
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

#[allow(dead_code)]
fn _assert_structure_is_send_sync(_: &Structure) {}
