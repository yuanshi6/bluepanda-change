use crate::transform::Structure;
use crate::types::{ConversionError, Stage};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::io::{Read, Write};

/// Serialize a Vanilla Structure root compound as GZIP-compressed NBT.
pub fn write_structure<W: Write>(writer: W, structure: &Structure) -> Result<(), ConversionError> {
    let mut gzip = GzEncoder::new(writer, Compression::default());
    fastnbt::to_writer(&mut gzip, structure).map_err(|e| {
        ConversionError::new(Stage::Write, format!("could not encode Structure NBT: {e}"))
    })?;
    gzip.finish().map_err(|e| {
        ConversionError::new(Stage::Write, format!("could not finish GZIP stream: {e}"))
    })?;
    Ok(())
}

/// Read the just-written GZIP NBT and check its Structure shape and semantic payload.
pub fn verify_structure<R: Read>(reader: R, expected: &Structure) -> Result<(), ConversionError> {
    let decoded: Structure = read_gzip_nbt(reader)?;
    validate_structure(&decoded)?;
    validate_structure(expected)?;
    if decoded.size != expected.size {
        return Err(ConversionError::new(
            Stage::Verify,
            "Structure size did not survive NBT round-trip",
        ));
    }
    if decoded.palette != expected.palette {
        return Err(ConversionError::new(
            Stage::Verify,
            "Structure palette did not survive NBT round-trip",
        ));
    }
    if decoded.blocks.len() != expected.blocks.len()
        || decoded.entities.len() != expected.entities.len()
    {
        return Err(ConversionError::new(
            Stage::Verify,
            "Structure record counts did not survive NBT round-trip",
        ));
    }
    if decoded.data_version != expected.data_version {
        return Err(ConversionError::new(
            Stage::Verify,
            "DataVersion did not survive NBT round-trip",
        ));
    }
    if decoded
        .blocks
        .iter()
        .zip(&expected.blocks)
        .any(|(a, b)| a.pos != b.pos || a.state != b.state || a.nbt != b.nbt)
    {
        return Err(ConversionError::new(
            Stage::Verify,
            "Structure block records did not survive NBT round-trip",
        ));
    }
    if decoded
        .entities
        .iter()
        .zip(&expected.entities)
        .any(|(a, b)| a.pos != b.pos || a.block_pos != b.block_pos || a.nbt != b.nbt)
    {
        return Err(ConversionError::new(
            Stage::Verify,
            "Structure entity records did not survive NBT round-trip",
        ));
    }
    Ok(())
}

fn validate_structure(structure: &Structure) -> Result<(), ConversionError> {
    if structure.palette.is_empty() {
        return Err(ConversionError::new(
            Stage::Verify,
            "Structure palette is empty",
        ));
    }
    if structure.size.iter().any(|v| *v <= 0) {
        return Err(ConversionError::new(
            Stage::Verify,
            "Structure dimensions must be positive",
        ));
    }
    for block in &structure.blocks {
        if block.state < 0 || block.state as usize >= structure.palette.len() {
            return Err(ConversionError::new(
                Stage::Verify,
                "block references an invalid palette index",
            ));
        }
        if (0..3).any(|axis| block.pos[axis] < 0 || block.pos[axis] >= structure.size[axis]) {
            return Err(ConversionError::new(
                Stage::Verify,
                "block position is outside Structure dimensions",
            )
            .at(block.pos));
        }
    }
    Ok(())
}

pub fn read_gzip_nbt<R: Read, T: DeserializeOwned>(reader: R) -> Result<T, ConversionError> {
    let mut decoder = GzDecoder::new(reader);
    let value = fastnbt::from_reader(&mut decoder).map_err(|e| {
        ConversionError::new(Stage::Verify, format!("could not read GZIP NBT: {e}"))
    })?;
    // NBT deserialization may stop after the root compound. Drain the decoder so its GZIP
    // checksum and trailer are checked, while keeping memory bounded for large schematics.
    let mut scratch = [0u8; 8192];
    loop {
        let n = decoder.read(&mut scratch).map_err(|e| {
            ConversionError::new(Stage::Verify, format!("invalid GZIP trailer: {e}"))
        })?;
        if n == 0 {
            break;
        }
        return Err(ConversionError::new(
            Stage::Verify,
            "trailing bytes after Structure NBT root",
        ));
    }
    Ok(value)
}

#[allow(dead_code)]
fn _assert_serialize<T: Serialize>() {}
