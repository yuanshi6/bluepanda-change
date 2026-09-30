#!/usr/bin/env python3
"""Independent, typed-NBT comparison for the real 7.1 sample and Structure output."""

from __future__ import annotations

import gzip
import math
import struct
import sys
from collections import Counter
from dataclasses import dataclass
from pathlib import Path
from typing import Any


@dataclass(frozen=True)
class ListValue:
    element_type: int
    values: tuple["Tag", ...]


@dataclass(frozen=True)
class Tag:
    tag_type: int
    value: Any


class NbtReader:
    """NBT reader retaining every payload's tag type and supporting Java Modified UTF-8."""

    def __init__(self, data: bytes):
        self.data = data
        self.offset = 0

    def take(self, size: int) -> bytes:
        if size < 0 or self.offset + size > len(self.data):
            raise ValueError(f"truncated NBT at byte {self.offset}, requested {size}")
        out = self.data[self.offset : self.offset + size]
        self.offset += size
        return out

    def u8(self) -> int:
        return self.take(1)[0]

    def i8(self) -> int:
        return struct.unpack(">b", self.take(1))[0]

    def i16(self) -> int:
        return struct.unpack(">h", self.take(2))[0]

    def u16(self) -> int:
        return struct.unpack(">H", self.take(2))[0]

    def i32(self) -> int:
        return struct.unpack(">i", self.take(4))[0]

    def i64(self) -> int:
        return struct.unpack(">q", self.take(8))[0]

    @staticmethod
    def decode_modified_utf8(raw: bytes) -> str:
        units: list[int] = []
        i = 0
        while i < len(raw):
            b0 = raw[i]
            if b0 & 0x80 == 0:
                units.append(b0)
                i += 1
            elif b0 & 0xE0 == 0xC0:
                if i + 1 >= len(raw) or raw[i + 1] & 0xC0 != 0x80:
                    raise ValueError("invalid two-byte Modified UTF-8 sequence")
                units.append(((b0 & 0x1F) << 6) | (raw[i + 1] & 0x3F))
                i += 2
            elif b0 & 0xF0 == 0xE0:
                if i + 2 >= len(raw) or raw[i + 1] & 0xC0 != 0x80 or raw[i + 2] & 0xC0 != 0x80:
                    raise ValueError("invalid three-byte Modified UTF-8 sequence")
                units.append(((b0 & 0x0F) << 12) | ((raw[i + 1] & 0x3F) << 6) | (raw[i + 2] & 0x3F))
                i += 3
            elif b0 & 0xF8 == 0xF0:
                # Accept standard UTF-8 supplementary code points too; Modified UTF-8 writers
                # normally encode them as CESU-8 surrogate pairs.
                if i + 3 >= len(raw):
                    raise ValueError("truncated four-byte UTF-8 sequence")
                cp = ((b0 & 0x07) << 18) | ((raw[i + 1] & 0x3F) << 12) | ((raw[i + 2] & 0x3F) << 6) | (raw[i + 3] & 0x3F)
                units.append(cp)
                i += 4
            else:
                raise ValueError("invalid Modified UTF-8 lead byte")

        out: list[str] = []
        i = 0
        while i < len(units):
            first = units[i]
            if 0xD800 <= first <= 0xDBFF and i + 1 < len(units) and 0xDC00 <= units[i + 1] <= 0xDFFF:
                out.append(chr(0x10000 + ((first - 0xD800) << 10) + units[i + 1] - 0xDC00))
                i += 2
            else:
                out.append(chr(first))
                i += 1
        return "".join(out)

    def string(self) -> str:
        return self.decode_modified_utf8(self.take(self.u16()))

    def named_root(self) -> tuple[str, Tag]:
        tag_type = self.u8()
        if tag_type == 0:
            raise ValueError("NBT root cannot be TAG_End")
        name = self.string()
        value = self.payload(tag_type, 0)
        if self.offset != len(self.data):
            raise ValueError(f"{len(self.data) - self.offset} trailing decompressed bytes")
        return name, Tag(tag_type, value)

    def payload(self, tag_type: int, depth: int) -> Any:
        if depth > 128:
            raise ValueError("NBT nesting exceeds 128 levels")
        if tag_type == 1:
            return self.i8()
        if tag_type == 2:
            return self.i16()
        if tag_type == 3:
            return self.i32()
        if tag_type == 4:
            return self.i64()
        if tag_type == 5:
            return struct.unpack(">f", self.take(4))[0]
        if tag_type == 6:
            return struct.unpack(">d", self.take(8))[0]
        if tag_type == 7:
            return self.take(self.i32())
        if tag_type == 8:
            return self.string()
        if tag_type == 9:
            element_type = self.u8()
            count = self.i32()
            if count < 0 or count > 50_000_000:
                raise ValueError(f"invalid NBT list size {count}")
            return ListValue(element_type, tuple(Tag(element_type, self.payload(element_type, depth + 1)) for _ in range(count)))
        if tag_type == 10:
            compound: dict[str, Tag] = {}
            while True:
                child_type = self.u8()
                if child_type == 0:
                    return compound
                child_name = self.string()
                compound[child_name] = Tag(child_type, self.payload(child_type, depth + 1))
        if tag_type == 11:
            count = self.i32()
            if count < 0 or count > 50_000_000:
                raise ValueError(f"invalid NBT int-array size {count}")
            return tuple(self.i32() for _ in range(count))
        if tag_type == 12:
            count = self.i32()
            if count < 0 or count > 50_000_000:
                raise ValueError(f"invalid NBT long-array size {count}")
            return tuple(self.i64() for _ in range(count))
        raise ValueError(f"unsupported NBT tag type {tag_type} at byte {self.offset}")


def read_gzip_nbt(path: Path) -> tuple[str, Tag]:
    raw = gzip.decompress(path.read_bytes())  # verifies the GZIP CRC/trailer
    return NbtReader(raw).named_root()


def compound(tag: Tag, where: str) -> dict[str, Tag]:
    if tag.tag_type != 10:
        raise ValueError(f"{where}: expected TAG_Compound, got {tag.tag_type}")
    return tag.value


def field(obj: dict[str, Tag], name: str) -> Tag:
    if name not in obj:
        raise ValueError(f"missing required field {name}")
    return obj[name]


def numeric(tag: Tag) -> int | float:
    if tag.tag_type not in (1, 2, 3, 4, 5, 6):
        raise ValueError(f"expected number tag, got {tag.tag_type}")
    return tag.value


def vec3(tag: Tag, where: str) -> tuple[int, int, int]:
    if tag.tag_type == 11 and len(tag.value) == 3:
        return tuple(tag.value)
    if tag.tag_type == 9 and len(tag.value.values) == 3:
        return tuple(int(numeric(t)) for t in tag.value.values)
    if tag.tag_type == 10:
        c = tag.value
        keys = ("x", "y", "z") if "x" in c else ("X", "Y", "Z")
        if all(k in c for k in keys):
            return tuple(int(numeric(c[k])) for k in keys)
    raise ValueError(f"{where}: expected three coordinates, got tag type {tag.tag_type}")


def block_state(tag: Tag) -> tuple[str, tuple[tuple[str, str], ...]]:
    c = compound(tag, "block state")
    name = field(c, "Name")
    if name.tag_type != 8:
        raise ValueError("block state Name must be TAG_String")
    properties = c.get("Properties")
    if properties is None:
        props: dict[str, str] = {}
    else:
        props = {}
        for key, value in compound(properties, "Properties").items():
            if value.tag_type != 8:
                raise ValueError(f"property {key} is not TAG_String")
            props[key] = value.value
    return name.value, tuple(sorted(props.items()))


def field_vec_int(tag: Tag, keys: tuple[str, str, str] = ("x", "y", "z")) -> tuple[int, int, int]:
    c = compound(tag, "position compound")
    return tuple(int(numeric(field(c, k))) for k in keys)


def bits_per_block(palette_len: int) -> int:
    return max(2, (palette_len - 1).bit_length())


def decode_indices(longs: tuple[int, ...], volume: int, bits: int) -> list[int]:
    expected = (volume * bits + 63) // 64
    if len(longs) != expected:
        raise ValueError(f"packed long count {len(longs)} != expected {expected}")
    mask = (1 << bits) - 1
    unsigned = [word & 0xFFFFFFFFFFFFFFFF for word in longs]
    out = []
    for i in range(volume):
        bit_start = i * bits
        word = bit_start // 64
        shift = bit_start % 64
        value = unsigned[word] >> shift
        if shift + bits > 64:
            value |= unsigned[word + 1] << (64 - shift)
        out.append(value & mask)
    return out


def set_compound_field(compound_tag: Tag, name: str, new_tag: Tag) -> Tag:
    copied = dict(compound_tag.value)
    copied[name] = new_tag
    return Tag(10, copied)


def add_to_numeric_tag(tag: Tag, delta: int) -> Tag:
    value = numeric(tag) + delta
    if tag.tag_type in (1, 2, 3, 4):
        return Tag(tag.tag_type, int(value))
    if tag.tag_type == 5:
        return Tag(5, float(value))
    return Tag(6, float(value))


def shift_coordinate_tag(tag: Tag, delta: tuple[int, int, int]) -> Tag:
    if tag.tag_type == 9 and len(tag.value.values) == 3:
        shifted = tuple(add_to_numeric_tag(v, delta[i]) for i, v in enumerate(tag.value.values))
        return Tag(9, ListValue(tag.value.element_type, shifted))
    if tag.tag_type == 11 and len(tag.value) == 3:
        return Tag(11, tuple(tag.value[i] + delta[i] for i in range(3)))
    if tag.tag_type == 10:
        c = dict(tag.value)
        keys = ("x", "y", "z") if "x" in c else ("X", "Y", "Z")
        if all(k in c for k in keys):
            for i, key in enumerate(keys):
                c[key] = add_to_numeric_tag(c[key], delta[i])
            return Tag(10, c)
    raise ValueError(f"unsupported shape for recognized coordinate field, tag type {tag.tag_type}")


def shift_entity_nbt(tag: Tag, delta: tuple[int, int, int]) -> Tag:
    c = dict(compound(tag, "entity NBT"))
    if "Pos" in c:
        c["Pos"] = shift_coordinate_tag(c["Pos"], delta)
    for key in ("block_pos", "home_pos", "leash"):
        if key in c:
            c[key] = shift_coordinate_tag(c[key], delta)
    for prefix in ("Tile", "Sleeping"):
        for axis, suffix in enumerate(("X", "Y", "Z")):
            key = prefix + suffix
            if key in c:
                c[key] = add_to_numeric_tag(c[key], delta[axis])
    return Tag(10, c)


def main(source_path: Path, output_path: Path) -> None:
    source_root_name, source_tag = read_gzip_nbt(source_path)
    output_root_name, output_tag = read_gzip_nbt(output_path)
    source = compound(source_tag, "source root")
    output = compound(output_tag, "output root")

    version = field(source, "Version").value
    subversion = field(source, "SubVersion").value
    data_version = source.get("MinecraftDataVersion")
    if version != 7 or subversion != 1:
        raise AssertionError(f"expected real source header 7.1, got {version}.{subversion}")
    if data_version is None or data_version.tag_type != 3 or data_version.value != 3955:
        raise AssertionError(f"unexpected source MinecraftDataVersion: {data_version}")
    regions = compound(field(source, "Regions"), "Regions")
    if len(regions) != 1:
        raise AssertionError(f"expected one source region, got {len(regions)}")
    region_name, region_tag = next(iter(regions.items()))
    region = compound(region_tag, f"Region {region_name}")
    source_position = field_vec_int(field(region, "Position"))
    signed_size = field_vec_int(field(region, "Size"))
    dims = tuple(abs(v) for v in signed_size)
    region_min = tuple(source_position[i] + min(signed_size[i], 0) + (1 if signed_size[i] < 0 else 0) for i in range(3))
    global_min = region_min
    source_size = dims  # one source Region; its inclusive bounds span abs(signed Size)
    volume = math.prod(dims)

    source_palette_tag = field(region, "BlockStatePalette")
    if source_palette_tag.tag_type != 9:
        raise ValueError("source BlockStatePalette is not a TAG_List")
    source_palette = [block_state(item) for item in source_palette_tag.value.values]
    longs_tag = field(region, "BlockStates")
    if longs_tag.tag_type != 12:
        raise ValueError("source BlockStates is not a TAG_Long_Array")
    bits = bits_per_block(len(source_palette))
    source_indices = decode_indices(longs_tag.value, volume, bits)
    if any(index >= len(source_palette) for index in source_indices):
        raise AssertionError("source packed stream contains an invalid palette reference")

    # A single region is present, so the packed index directly determines the source world cell.
    source_states = [source_palette[index] for index in source_indices]
    air_names = {"minecraft:air", "minecraft:cave_air", "minecraft:void_air"}
    expected_blocks: dict[tuple[int, int, int], tuple[str, tuple[tuple[str, str], ...]]] = {}
    for index, state in enumerate(source_states):
        if state[0] in air_names:
            continue
        x = index % dims[0]
        z = (index // dims[0]) % dims[2]
        y = index // (dims[0] * dims[2])
        world = (region_min[0] + x, region_min[1] + y, region_min[2] + z)
        expected_blocks[tuple(world[i] - global_min[i] for i in range(3))] = state

    output_size = vec3(field(output, "size"), "output size")
    output_data_version = output.get("DataVersion")
    if output_data_version is None or output_data_version.tag_type != 3 or output_data_version.value != 3955:
        raise AssertionError(f"output DataVersion mismatch: {output_data_version}")
    if output_size != source_size:
        raise AssertionError(f"output size {output_size} != expected normalized size {source_size}")

    target_palette_tag = field(output, "palette")
    if target_palette_tag.tag_type != 9:
        raise ValueError("output palette is not a TAG_List")
    target_palette = [block_state(item) for item in target_palette_tag.value.values]
    target_blocks_tag = field(output, "blocks")
    if target_blocks_tag.tag_type != 9:
        raise ValueError("output blocks is not a TAG_List")
    target_blocks: dict[tuple[int, int, int], tuple[tuple[str, tuple[tuple[str, str], ...]], Tag | None]] = {}
    for record_tag in target_blocks_tag.value.values:
        record = compound(record_tag, "output block record")
        pos = vec3(field(record, "pos"), "block pos")
        if any(pos[i] < 0 or pos[i] >= output_size[i] for i in range(3)):
            raise AssertionError(f"output block outside size: {pos}")
        if pos in target_blocks:
            raise AssertionError(f"duplicate output block coordinate {pos}")
        state_index = field(record, "state")
        if state_index.tag_type != 3 or not 0 <= state_index.value < len(target_palette):
            raise AssertionError(f"invalid output palette reference at {pos}: {state_index.value}")
        target_blocks[pos] = (target_palette[state_index.value], record.get("nbt"))

    if set(target_blocks) != set(expected_blocks):
        missing = set(expected_blocks) - set(target_blocks)
        extra = set(target_blocks) - set(expected_blocks)
        raise AssertionError(f"block coordinate mismatch: missing={len(missing)}, extra={len(extra)}")
    for pos, expected_state in expected_blocks.items():
        actual_state, _ = target_blocks[pos]
        if actual_state != expected_state:
            raise AssertionError(f"block state mismatch at {pos}: {actual_state} != {expected_state}")

    # Compare typed block-entity payloads after removing only source absolute-location fields.
    source_bes: dict[tuple[int, int, int], Tag] = {}
    bes_tag = region.get("TileEntities") or region.get("BlockEntities")
    if bes_tag is not None:
        if bes_tag.tag_type != 9:
            raise ValueError("source block entity field is not a TAG_List")
        for raw_tag in bes_tag.value.values:
            raw = compound(raw_tag, "source block entity")
            local = tuple(int(numeric(field(raw, key))) for key in ("x", "y", "z"))
            world = tuple(region_min[i] + local[i] for i in range(3))
            rel = tuple(world[i] - global_min[i] for i in range(3))
            payload = Tag(10, {k: v for k, v in raw.items() if k not in ("x", "y", "z")})
            if rel in source_bes:
                raise AssertionError(f"duplicate source block entity coordinate {rel}")
            source_bes[rel] = payload
    target_bes = {pos: nbt for pos, (_, nbt) in target_blocks.items() if nbt is not None}
    if set(source_bes) != set(target_bes):
        raise AssertionError(f"block entity coordinate mismatch: source={len(source_bes)} output={len(target_bes)}")
    for pos, expected_payload in source_bes.items():
        if target_bes[pos] != expected_payload:
            raise AssertionError(f"typed block entity payload mismatch at {pos}")

    source_entities_tag = region.get("Entities")
    source_entities = [] if source_entities_tag is None else list(source_entities_tag.value.values)
    target_entities_tag = field(output, "entities")
    if target_entities_tag.tag_type != 9:
        raise ValueError("output entities is not a TAG_List")
    target_entities = list(target_entities_tag.value.values)
    if len(source_entities) != len(target_entities):
        raise AssertionError(f"entity count mismatch: source={len(source_entities)} output={len(target_entities)}")
    delta = tuple(source_position[i] - global_min[i] for i in range(3))
    id_counts: Counter[str] = Counter()
    first_source_pos: tuple[float, float, float] | None = None
    first_output_pos: tuple[float, float, float] | None = None
    for entity_index, (source_entity_tag, target_entity_tag) in enumerate(zip(source_entities, target_entities)):
        source_entity = compound(source_entity_tag, "source entity")
        target_entity = compound(target_entity_tag, "output entity")
        source_id = field(source_entity, "id")
        id_counts[source_id.value] += 1
        source_pos = field(source_entity, "Pos")
        source_pos_values = tuple(float(numeric(v)) for v in source_pos.value.values)
        target_pos = field(target_entity, "pos")
        target_pos_values = tuple(float(numeric(v)) for v in target_pos.value.values)
        expected_pos = tuple(source_pos_values[i] + delta[i] for i in range(3))
        if any(abs(target_pos_values[i] - expected_pos[i]) > 1e-9 for i in range(3)):
            raise AssertionError(f"entity {entity_index} Structure pos mismatch: {target_pos_values} != {expected_pos}")
        if entity_index == 0:
            first_source_pos = source_pos_values
            first_output_pos = target_pos_values

        normalized = shift_entity_nbt(source_entity_tag, delta)
        output_nbt = field(target_entity, "nbt")
        if normalized != output_nbt:
            src_fields = set(source_entity)
            coord_fields = {"Pos", "block_pos", "home_pos", "leash", "TileX", "TileY", "TileZ", "SleepingX", "SleepingY", "SleepingZ"}
            differences = sorted(k for k in src_fields | set(output_nbt.value) if normalized.value.get(k) != output_nbt.value.get(k))
            raise AssertionError(f"entity {entity_index} typed NBT differs after known-coordinate shift: {differences}")

        block_pos = vec3(field(target_entity, "blockPos"), "entity blockPos")
        expected_block_pos = tuple(math.floor(v) for v in expected_pos)
        if block_pos != expected_block_pos:
            raise AssertionError(f"entity {entity_index} blockPos mismatch: {block_pos} != {expected_block_pos}")

    if len(expected_blocks) != len(target_blocks):
        raise AssertionError("non-air source/output block counts differ")
    if len(source_bes) != 418 or len(source_entities) != 18:
        raise AssertionError(f"unexpected source BE/entity counts: {len(source_bes)}/{len(source_entities)}")
    if id_counts != Counter({"minecraft:bat": 17, "minecraft:chicken": 1}):
        raise AssertionError(f"unexpected entity IDs: {id_counts}")

    print("Independent typed NBT comparison: PASS")
    print(f"source root={source_root_name!r} output root={output_root_name!r}")
    print(f"header={version}.{subversion} DataVersion={data_version.value}; target DataVersion={output_data_version.value}")
    print(f"region={region_name!r} position={source_position} signed_size={signed_size} bounds_min={region_min} output_size={output_size}")
    print(f"palette={len(source_palette)} bits_per_block={bits} longs={len(longs_tag.value)} raw_cells={volume} nonair={len(expected_blocks)}")
    print(f"output palette={len(target_palette)} blocks={len(target_blocks)} block_entities={len(target_bes)} entities={len(target_entities)} ids={dict(id_counts)}")
    print(f"block states, coordinates, and typed block-entity payloads matched; all palette references and bounds valid")
    print(f"all entity coordinates and typed NBT matched after known-field translation {delta}")
    print(f"first entity Pos: {first_source_pos} -> {first_output_pos}")


if __name__ == "__main__":
    if len(sys.argv) != 3:
        raise SystemExit("usage: validate_real_sample.py SOURCE.litematic OUTPUT.nbt")
    main(Path(sys.argv[1]), Path(sys.argv[2]))
