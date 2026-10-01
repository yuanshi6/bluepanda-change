use crate::conversion::{convert_batch_with_events, convert_file_with_events, ConversionEvent};
use crate::{BatchResult, ConversionError, ConversionOptions, ConversionResult, ProgressEvent};
use std::path::{Path, PathBuf};

/// Convert a Litematica or Sponge schematic to a verified, compressed Structure NBT file.
///
/// This compatibility entry point preserves the original API and forwards only progress events.
pub fn convert_file(
    input: &Path,
    output: &Path,
    options: &ConversionOptions,
    mut progress: impl FnMut(ProgressEvent),
) -> Result<ConversionResult, ConversionError> {
    convert_file_with_events(input, output, options, |event| {
        if let ConversionEvent::Progress(progress_event) = event {
            progress(progress_event);
        }
    })
}

/// Convert a list sequentially. Duplicate inputs are collapsed and every task targeting the same
/// output name fails before conversion, including when overwrite is enabled. This compatibility
/// entry point preserves the original API and forwards only progress events.
pub fn convert_batch(
    inputs: &[PathBuf],
    output_dir: &Path,
    options: &ConversionOptions,
    mut progress: impl FnMut(ProgressEvent),
) -> BatchResult {
    convert_batch_with_events(inputs, output_dir, options, |event| {
        if let ConversionEvent::Progress(progress_event) = event {
            progress(progress_event);
        }
    })
}
