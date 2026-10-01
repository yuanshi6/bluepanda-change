use crate::state::{CameraPreset, DisplayMode};
use eframe::egui::{self, epaint::PaintCallback};
use eframe::egui_glow::{CallbackFn, Painter};
use eframe::glow::{self, HasContext};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};

pub const MAX_PREVIEW_FACES: usize = 500_000;
pub const MAX_PREVIEW_WIRE_SEGMENTS: usize = 2_000_000;
const CHUNK_SIZE: i32 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerView {
    All,
    Layer(i32),
    Through(i32),
}

#[derive(Clone, Debug)]
pub struct BlockInput {
    pub position: [i32; 3],
    pub region_id: u32,
    pub name: String,
    pub properties: HashMap<String, String>,
}

#[derive(Clone, Debug, Default)]
pub struct CpuChunk {
    pub vertices: Vec<f32>,
    pub indices: Vec<u32>,
    /// Independent line vertices keep wireframe edges free of triangle diagonals.
    pub wire_vertices: Vec<f32>,
    pub wire_segments: usize,
}

#[derive(Clone, Debug, Default)]
pub struct CpuMesh {
    pub chunks: Vec<CpuChunk>,
    pub block_count: usize,
    pub face_count: usize,
    pub wire_segment_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    Cube,
    SlabBottom,
    SlabTop,
    Stair { facing: Facing, upside_down: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Facing {
    North,
    South,
    East,
    West,
}

#[derive(Clone, Copy, Debug)]
struct Cell {
    color: [u8; 3],
    shape: Shape,
}

/// Build an opaque, flat-colour mesh. Coordinates are document-relative.
#[cfg(test)]
pub fn build_mesh(
    blocks: &[BlockInput],
    layer: LayerView,
    cancel: impl Fn() -> bool,
) -> Result<CpuMesh, String> {
    let visible_regions: BTreeSet<_> = blocks.iter().map(|block| block.region_id).collect();
    build_mesh_with_progress(blocks, layer, &visible_regions, cancel, |_, _| {})
}

pub fn build_mesh_with_progress(
    blocks: &[BlockInput],
    layer: LayerView,
    visible_regions: &BTreeSet<u32>,
    cancel: impl Fn() -> bool,
    mut progress: impl FnMut(usize, usize),
) -> Result<CpuMesh, String> {
    let mut visible = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        if index % 4096 == 0 && cancel() {
            return Err("预览已取消".into());
        }
        let in_layer = match layer {
            LayerView::All => true,
            LayerView::Layer(y) => block.position[1] == y,
            LayerView::Through(y) => block.position[1] <= y,
        };
        if in_layer && visible_regions.contains(&block.region_id) {
            visible.push(block);
        }
    }
    if visible.len() > 1_000_000 {
        return Err("预览方块超过 1,000,000 个上限".into());
    }

    // Candidates arrive in stable source-region order. Keeping the first visible candidate
    // preserves the historical earlier-region-wins behavior while allowing hidden overlaps
    // to reveal the next visible region.
    let mut cells: HashMap<[i32; 3], Cell> = HashMap::with_capacity(visible.len());
    for (index, block) in visible.iter().enumerate() {
        if index % 4096 == 0 && cancel() {
            return Err("预览已取消".into());
        }
        cells.entry(block.position).or_insert_with(|| Cell {
            color: block_color(&block.name),
            shape: block_shape(block),
        });
    }
    let mut by_chunk: HashMap<[i32; 3], Vec<([i32; 3], Cell)>> = HashMap::new();
    for (index, (position, cell)) in cells.iter().enumerate() {
        if index % 4096 == 0 && cancel() {
            return Err("预览已取消".into());
        }
        by_chunk
            .entry(chunk_key(*position))
            .or_default()
            .push((*position, *cell));
    }

    let mut mesh = CpuMesh {
        chunks: Vec::with_capacity(by_chunk.len()),
        block_count: cells.len(),
        face_count: 0,
        wire_segment_count: 0,
    };
    let total = cells.len();
    let mut processed = 0usize;
    let mut keys: Vec<_> = by_chunk.keys().copied().collect();
    keys.sort_unstable();
    for key in keys {
        if cancel() {
            return Err("预览已取消".into());
        }
        let chunk_cells = &by_chunk[&key];
        let mut chunk = CpuChunk::default();
        let mut full_cubes = Vec::new();
        for (index, (position, cell)) in chunk_cells.iter().enumerate() {
            if index % 4096 == 0 && cancel() {
                return Err("预览已取消".into());
            }
            match cell.shape {
                Shape::Cube => full_cubes.push((*position, cell.color)),
                shape => add_partial_shape(&mut chunk, *position, cell.color, shape),
            }
            if chunk.indices.len() / 6 > MAX_PREVIEW_FACES {
                return Err(format!("预览暴露面超过 {MAX_PREVIEW_FACES} 个上限"));
            }
            if chunk.wire_segments > MAX_PREVIEW_WIRE_SEGMENTS {
                return Err(format!("预览格线超过 {MAX_PREVIEW_WIRE_SEGMENTS} 段上限"));
            }
            processed += 1;
            if index % 4096 == 0 || processed == total {
                progress(processed, total.max(1));
            }
        }
        add_greedy_cube_faces(&mut chunk, &cells, &full_cubes, key);
        let chunk_faces = chunk.indices.len() / 6;
        if mesh.face_count + chunk_faces > MAX_PREVIEW_FACES {
            return Err(format!("预览暴露面超过 {MAX_PREVIEW_FACES} 个上限"));
        }
        mesh.face_count += chunk_faces;
        mesh.wire_segment_count += chunk.wire_segments;
        if mesh.wire_segment_count > MAX_PREVIEW_WIRE_SEGMENTS {
            return Err(format!("预览格线超过 {MAX_PREVIEW_WIRE_SEGMENTS} 段上限"));
        }
        mesh.chunks.push(chunk);
    }
    if mesh.face_count > MAX_PREVIEW_FACES {
        return Err(format!("预览暴露面超过 {MAX_PREVIEW_FACES} 个上限"));
    }
    Ok(mesh)
}

fn chunk_key(position: [i32; 3]) -> [i32; 3] {
    position.map(|coordinate| coordinate.div_euclid(CHUNK_SIZE))
}

fn block_shape(block: &BlockInput) -> Shape {
    let name = block.name.as_str();
    if name.ends_with("_slab") {
        match block.properties.get("type").map(String::as_str) {
            Some("top") => Shape::SlabTop,
            Some("double") => Shape::Cube,
            _ => Shape::SlabBottom,
        }
    } else if name.ends_with("_stairs") {
        let facing = match block.properties.get("facing").map(String::as_str) {
            Some("north") => Facing::North,
            Some("south") => Facing::South,
            Some("east") => Facing::East,
            _ => Facing::West,
        };
        Shape::Stair {
            facing,
            upside_down: block
                .properties
                .get("half")
                .is_some_and(|half| half == "top"),
        }
    } else {
        Shape::Cube
    }
}

fn block_color(name: &str) -> [u8; 3] {
    let _ = name;
    [205, 215, 224]
}

fn add_greedy_cube_faces(
    chunk: &mut CpuChunk,
    cells: &HashMap<[i32; 3], Cell>,
    cubes: &[([i32; 3], [u8; 3])],
    chunk_key: [i32; 3],
) {
    let origin = chunk_key.map(|coordinate| coordinate * CHUNK_SIZE);
    for axis in 0..3 {
        let other = match axis {
            0 => [2, 1],
            1 => [0, 2],
            _ => [0, 1],
        };
        for positive in [false, true] {
            let step = if positive { 1 } else { -1 };
            let normal = match (axis, positive) {
                (0, false) => [-1.0, 0.0, 0.0],
                (0, true) => [1.0, 0.0, 0.0],
                (1, false) => [0.0, -1.0, 0.0],
                (1, true) => [0.0, 1.0, 0.0],
                (2, false) => [0.0, 0.0, -1.0],
                _ => [0.0, 0.0, 1.0],
            };
            let mut planes: BTreeMap<(i32, [u8; 3]), BTreeSet<(i32, i32)>> = BTreeMap::new();
            for (position, color) in cubes {
                let neighbor = [
                    position[0] + if axis == 0 { step } else { 0 },
                    position[1] + if axis == 1 { step } else { 0 },
                    position[2] + if axis == 2 { step } else { 0 },
                ];
                if cells
                    .get(&neighbor)
                    .is_some_and(|cell| cell.shape == Shape::Cube)
                {
                    continue;
                }
                let local_plane = position[axis] - origin[axis] + if positive { 1 } else { 0 };
                let u = position[other[0]] - origin[other[0]];
                let v = position[other[1]] - origin[other[1]];
                if !(0..CHUNK_SIZE).contains(&u) || !(0..CHUNK_SIZE).contains(&v) {
                    continue;
                }
                planes
                    .entry((local_plane, *color))
                    .or_default()
                    .insert((v, u));
            }
            for ((plane, color), mut coordinates) in planes {
                while let Some(&(v, u)) = coordinates.iter().next() {
                    let mut width = 1;
                    while u + width < CHUNK_SIZE && coordinates.contains(&(v, u + width)) {
                        width += 1;
                    }
                    let mut height = 1;
                    'height: while v + height < CHUNK_SIZE {
                        for offset in 0..width {
                            if !coordinates.contains(&(v + height, u + offset)) {
                                break 'height;
                            }
                        }
                        height += 1;
                    }
                    for dv in 0..height {
                        for du in 0..width {
                            coordinates.remove(&(v + dv, u + du));
                        }
                    }
                    let corners =
                        face_rect_corners(axis, positive, origin, plane, u, v, width, height);
                    push_quad(chunk, corners, normal, color);
                    push_quad_grid(chunk, corners);
                }
            }
        }
    }
}

fn face_rect_corners(
    axis: usize,
    positive: bool,
    origin: [i32; 3],
    plane: i32,
    u: i32,
    v: i32,
    width: i32,
    height: i32,
) -> [[f32; 3]; 4] {
    let other = match axis {
        0 => [2, 1],
        1 => [0, 2],
        _ => [0, 1],
    };
    let mut p = [0.0f32; 3];
    p[axis] = (origin[axis] + plane) as f32;
    p[other[0]] = (origin[other[0]] + u) as f32;
    p[other[1]] = (origin[other[1]] + v) as f32;
    let mut pu = p;
    pu[other[0]] += width as f32;
    let mut puv = pu;
    puv[other[1]] += height as f32;
    let mut pv = p;
    pv[other[1]] += height as f32;
    let corners = if (axis, positive) == (0, false)
        || (axis, positive) == (1, false)
        || (axis, positive) == (2, true)
    {
        [p, pu, puv, pv]
    } else {
        [p, pv, puv, pu]
    };
    corners
}

fn add_partial_shape(chunk: &mut CpuChunk, position: [i32; 3], color: [u8; 3], shape: Shape) {
    let [x, y, z] = position.map(|n| n as f32);
    let boxes: Vec<([f32; 3], [f32; 3])> = match shape {
        Shape::SlabBottom => vec![([x, y, z], [x + 1.0, y + 0.5, z + 1.0])],
        Shape::SlabTop => vec![([x, y + 0.5, z], [x + 1.0, y + 1.0, z + 1.0])],
        Shape::Stair {
            facing,
            upside_down,
        } => {
            let middle = if upside_down { y + 0.5 } else { y + 0.5 };
            let (lo, hi) = match facing {
                Facing::North => ([x, z], [x + 1.0, z + 0.5]),
                Facing::South => ([x, z + 0.5], [x + 1.0, z + 1.0]),
                Facing::West => ([x, z], [x + 0.5, z + 1.0]),
                Facing::East => ([x + 0.5, z], [x + 1.0, z + 1.0]),
            };
            if upside_down {
                vec![
                    ([x, y + 0.5, z], [x + 1.0, y + 1.0, z + 1.0]),
                    ([lo[0], y, lo[1]], [hi[0], middle, hi[1]]),
                ]
            } else {
                vec![
                    ([x, y, z], [x + 1.0, y + 0.5, z + 1.0]),
                    ([lo[0], middle, lo[1]], [hi[0], y + 1.0, hi[1]]),
                ]
            }
        }
        Shape::Cube => return,
    };
    for (min, max) in boxes {
        add_box(chunk, min, max, color);
    }
}

fn add_box(chunk: &mut CpuChunk, min: [f32; 3], max: [f32; 3], color: [u8; 3]) {
    let [x, y, z] = min;
    let [x1, y1, z1] = max;
    let faces = [
        (
            [[x, y, z], [x, y, z1], [x, y1, z1], [x, y1, z]],
            [-1.0, 0.0, 0.0],
        ),
        (
            [[x1, y, z], [x1, y1, z], [x1, y1, z1], [x1, y, z1]],
            [1.0, 0.0, 0.0],
        ),
        (
            [[x, y, z], [x1, y, z], [x1, y, z1], [x, y, z1]],
            [0.0, -1.0, 0.0],
        ),
        (
            [[x, y1, z], [x, y1, z1], [x1, y1, z1], [x1, y1, z]],
            [0.0, 1.0, 0.0],
        ),
        (
            [[x, y, z], [x, y1, z], [x1, y1, z], [x1, y, z]],
            [0.0, 0.0, -1.0],
        ),
        (
            [[x, y, z1], [x1, y, z1], [x1, y1, z1], [x, y1, z1]],
            [0.0, 0.0, 1.0],
        ),
    ];
    for (corners, normal) in faces {
        push_quad(chunk, corners, normal, color);
    }
}

fn push_quad(chunk: &mut CpuChunk, corners: [[f32; 3]; 4], normal: [f32; 3], color: [u8; 3]) {
    let base = (chunk.vertices.len() / 10) as u32;
    let shades = match normal {
        [0.0, 1.0, 0.0] => 1.0,
        [0.0, -1.0, 0.0] => 0.52,
        [0.0, 0.0, _] => 0.72,
        _ => 0.84,
    };
    for position in corners {
        chunk.vertices.extend([
            position[0],
            position[1],
            position[2],
            normal[0],
            normal[1],
            normal[2],
            color[0] as f32 / 255.0 * shades,
            color[1] as f32 / 255.0 * shades,
            color[2] as f32 / 255.0 * shades,
            1.0,
        ]);
    }
    chunk
        .indices
        .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    for edge in 0..4 {
        let start = corners[edge];
        let end = corners[(edge + 1) % 4];
        push_wire_segment(chunk, start, end, normal);
    }
}

/// Add each unit's face boundaries across a greedy rectangle. The filled faces stay merged for
/// low triangle counts, while independent GL_LINES produce visible per-block seams without ever
/// drawing the triangulation diagonal.
fn push_quad_grid(chunk: &mut CpuChunk, corners: [[f32; 3]; 4]) {
    let u_len = axis_length(corners[0], corners[1]).round() as i32;
    let v_len = axis_length(corners[0], corners[3]).round() as i32;
    if u_len <= 0 || v_len <= 0 {
        return;
    }
    let normal = normalize3(cross3(
        sub3(corners[1], corners[0]),
        sub3(corners[3], corners[0]),
    ));
    let u_step = scale3(sub3(corners[1], corners[0]), 1.0 / u_len as f32);
    let v_step = scale3(sub3(corners[3], corners[0]), 1.0 / v_len as f32);
    for v in 1..v_len {
        let offset = scale3(v_step, v as f32);
        push_wire_segment(
            chunk,
            add3(corners[0], offset),
            add3(corners[1], offset),
            normal,
        );
    }
    for u in 1..u_len {
        let offset = scale3(u_step, u as f32);
        push_wire_segment(
            chunk,
            add3(corners[0], offset),
            add3(corners[3], offset),
            normal,
        );
    }
}

fn push_wire_segment(chunk: &mut CpuChunk, start: [f32; 3], end: [f32; 3], normal: [f32; 3]) {
    const OFFSET: f32 = 0.0025;
    const COLOR: [f32; 4] = [0.38, 0.45, 0.52, 0.58];
    let start = add3(start, scale3(normal, OFFSET));
    let end = add3(end, scale3(normal, OFFSET));
    for position in [start, end] {
        chunk.wire_vertices.extend([
            position[0],
            position[1],
            position[2],
            normal[0],
            normal[1],
            normal[2],
            COLOR[0],
            COLOR[1],
            COLOR[2],
            COLOR[3],
        ]);
    }
    chunk.wire_segments += 1;
}

fn axis_length(start: [f32; 3], end: [f32; 3]) -> f32 {
    let delta = sub3(end, start);
    (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt()
}

fn add3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale3(a: [f32; 3], scale: f32) -> [f32; 3] {
    [a[0] * scale, a[1] * scale, a[2] * scale]
}

fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize3(value: [f32; 3]) -> [f32; 3] {
    let length = axis_length([0.0; 3], value).max(1e-6);
    scale3(value, 1.0 / length)
}

#[derive(Clone, Debug)]
pub struct Camera {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub target: [f32; 3],
    pub bounds: [f32; 3],
    pub orthographic: bool,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            yaw: 0.75,
            pitch: 0.55,
            distance: 16.0,
            target: [0.5, 0.5, 0.5],
            bounds: [1.0; 3],
            orthographic: false,
        }
    }
}

impl Camera {
    pub fn fit(&mut self, size: [i32; 3]) {
        self.bounds = size.map(|n| n.max(1) as f32);
        self.target = self.bounds.map(|n| n * 0.5);
        self.distance = self.bounds[0].max(self.bounds[1]).max(self.bounds[2]) * 1.8 + 2.0;
    }

    pub fn reset(&mut self) {
        self.yaw = 0.75;
        self.pitch = 0.55;
        self.target = self.bounds.map(|n| n * 0.5);
        self.distance = self.bounds[0].max(self.bounds[1]).max(self.bounds[2]) * 1.8 + 2.0;
        self.orthographic = false;
    }

    pub fn orbit(&mut self, delta: egui::Vec2) {
        self.yaw -= delta.x * 0.008;
        self.pitch = (self.pitch + delta.y * 0.008).clamp(-1.48, 1.48);
    }

    pub fn zoom(&mut self, scroll: f32) {
        self.distance = (self.distance * (-scroll * 0.001).exp()).clamp(2.0, 100_000.0);
    }

    pub fn pan(&mut self, delta: egui::Vec2, viewport_height: f32) {
        let scale = self.distance / viewport_height.max(1.0);
        let right = [self.yaw.cos(), 0.0, self.yaw.sin()];
        let up = [
            -self.yaw.sin() * self.pitch.sin(),
            self.pitch.cos(),
            self.yaw.cos() * self.pitch.sin(),
        ];
        for axis in 0..3 {
            self.target[axis] -= right[axis] * delta.x * scale;
            self.target[axis] += up[axis] * delta.y * scale;
        }
    }

    fn view_projection(&self, aspect: f32) -> [f32; 16] {
        let cp = self.pitch.cos();
        let eye = [
            self.target[0] + self.distance * cp * self.yaw.sin(),
            self.target[1] + self.distance * self.pitch.sin(),
            self.target[2] + self.distance * cp * self.yaw.cos(),
        ];
        let view = look_at(eye, self.target, [0.0, 1.0, 0.0]);
        let radius = self.bounds[0]
            .max(self.bounds[1])
            .max(self.bounds[2])
            .max(1.0);
        let projection = if self.orthographic {
            let half_height = self.distance.max(2.0) * 0.55;
            orthographic(
                -half_height * aspect,
                half_height * aspect,
                -half_height,
                half_height,
                -radius * 100.0,
                radius * 100.0,
            )
        } else {
            perspective(
                45f32.to_radians(),
                aspect,
                0.05,
                radius * 100.0 + self.distance * 2.0,
            )
        };
        multiply(projection, view)
    }

    fn eye(&self) -> [f32; 3] {
        let cp = self.pitch.cos();
        [
            self.target[0] + self.distance * cp * self.yaw.sin(),
            self.target[1] + self.distance * self.pitch.sin(),
            self.target[2] + self.distance * cp * self.yaw.cos(),
        ]
    }

    fn forward(&self) -> [f32; 3] {
        normalize3(sub3(self.target, self.eye()))
    }
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-6);
    [v[0] / length, v[1] / length, v[2] / length]
}

fn look_at(eye: [f32; 3], center: [f32; 3], up: [f32; 3]) -> [f32; 16] {
    let f = normalize([center[0] - eye[0], center[1] - eye[1], center[2] - eye[2]]);
    let s = normalize([
        f[1] * up[2] - f[2] * up[1],
        f[2] * up[0] - f[0] * up[2],
        f[0] * up[1] - f[1] * up[0],
    ]);
    let u = [
        s[1] * f[2] - s[2] * f[1],
        s[2] * f[0] - s[0] * f[2],
        s[0] * f[1] - s[1] * f[0],
    ];
    [
        s[0],
        u[0],
        -f[0],
        0.0,
        s[1],
        u[1],
        -f[1],
        0.0,
        s[2],
        u[2],
        -f[2],
        0.0,
        -(s[0] * eye[0] + s[1] * eye[1] + s[2] * eye[2]),
        -(u[0] * eye[0] + u[1] * eye[1] + u[2] * eye[2]),
        f[0] * eye[0] + f[1] * eye[1] + f[2] * eye[2],
        1.0,
    ]
}

fn perspective(fov: f32, aspect: f32, near: f32, far: f32) -> [f32; 16] {
    let f = 1.0 / (fov * 0.5).tan();
    [
        f / aspect.max(0.01),
        0.0,
        0.0,
        0.0,
        0.0,
        f,
        0.0,
        0.0,
        0.0,
        0.0,
        (far + near) / (near - far),
        -1.0,
        0.0,
        0.0,
        (2.0 * far * near) / (near - far),
        0.0,
    ]
}

fn orthographic(left: f32, right: f32, bottom: f32, top: f32, near: f32, far: f32) -> [f32; 16] {
    let width = (right - left).max(1e-6);
    let height = (top - bottom).max(1e-6);
    let depth = (far - near).max(1e-6);
    [
        2.0 / width,
        0.0,
        0.0,
        0.0,
        0.0,
        2.0 / height,
        0.0,
        0.0,
        0.0,
        0.0,
        -2.0 / depth,
        0.0,
        -(right + left) / width,
        -(top + bottom) / height,
        -(far + near) / depth,
        1.0,
    ]
}

fn view_depth(point: [f32; 3], eye: [f32; 3], forward: [f32; 3]) -> f32 {
    let offset = sub3(point, eye);
    offset[0] * forward[0] + offset[1] * forward[1] + offset[2] * forward[2]
}

fn project_world(camera: &Camera, rect: egui::Rect, point: [f32; 3]) -> Option<egui::Pos2> {
    let matrix = camera.view_projection(rect.width() / rect.height().max(1.0));
    let x = matrix[0] * point[0] + matrix[4] * point[1] + matrix[8] * point[2] + matrix[12];
    let y = matrix[1] * point[0] + matrix[5] * point[1] + matrix[9] * point[2] + matrix[13];
    let z = matrix[2] * point[0] + matrix[6] * point[1] + matrix[10] * point[2] + matrix[14];
    let w = matrix[3] * point[0] + matrix[7] * point[1] + matrix[11] * point[2] + matrix[15];
    if w <= 0.001 {
        return None;
    }
    let ndc = [x / w, y / w, z / w];
    if ndc[2] < -1.0 || ndc[2] > 1.0 {
        return None;
    }
    Some(egui::pos2(
        rect.left() + (ndc[0] + 1.0) * rect.width() * 0.5,
        rect.top() + (1.0 - ndc[1]) * rect.height() * 0.5,
    ))
}

fn draw_reference_grid(ui: &mut egui::Ui, rect: egui::Rect, camera: &Camera) {
    let visible_radius = if camera.orthographic {
        camera.distance * 0.55
    } else {
        camera.distance * 0.75
    };
    let max_x = (camera.bounds[0] * 0.5 + visible_radius).ceil() as i32;
    let max_z = (camera.bounds[2] * 0.5 + visible_radius).ceil() as i32;
    let span = max_x.max(max_z).max(1);
    let target_step = (span / 18).max(1);
    let scale = 10_i32.pow((target_step as f32).log10().floor().max(0.0) as u32);
    let scaled = target_step as f32 / scale as f32;
    let step = if scaled <= 1.0 {
        scale
    } else if scaled <= 2.0 {
        2 * scale
    } else if scaled <= 5.0 {
        5 * scale
    } else {
        10 * scale
    };
    let x_start = (camera.target[0] - max_x as f32).floor() as i32;
    let x_end = (camera.target[0] + max_x as f32).ceil() as i32;
    let z_start = (camera.target[2] - max_z as f32).floor() as i32;
    let z_end = (camera.target[2] + max_z as f32).ceil() as i32;
    let painter = ui.painter().with_clip_rect(rect);
    let grid_stroke = egui::Stroke::new(0.7, egui::Color32::from_rgba_unmultiplied(65, 75, 88, 38));
    let axis_stroke = egui::Stroke::new(
        1.1,
        egui::Color32::from_rgba_unmultiplied(105, 124, 145, 72),
    );
    let y = -0.015;
    let mut x = x_start;
    while x <= x_end {
        let color = if x == 0 { axis_stroke } else { grid_stroke };
        if let (Some(a), Some(b)) = (
            project_world(camera, rect, [x as f32, y, z_start as f32]),
            project_world(camera, rect, [x as f32, y, z_end as f32]),
        ) {
            painter.line_segment([a, b], color);
        }
        x += step;
    }
    let mut z = z_start;
    while z <= z_end {
        let color = if z == 0 { axis_stroke } else { grid_stroke };
        if let (Some(a), Some(b)) = (
            project_world(camera, rect, [x_start as f32, y, z as f32]),
            project_world(camera, rect, [x_end as f32, y, z as f32]),
        ) {
            painter.line_segment([a, b], color);
        }
        z += step;
    }
}

fn draw_axis_gizmo(ui: &mut egui::Ui, rect: egui::Rect, camera: &Camera) {
    let origin = egui::pos2(rect.left() + 34.0, rect.bottom() - 34.0);
    let yaw = camera.yaw;
    let pitch = camera.pitch;
    let basis = [
        (
            [yaw.cos(), yaw.sin() * pitch.sin()],
            egui::Color32::from_rgb(238, 88, 80),
            "X",
        ),
        (
            [0.0, pitch.cos()],
            egui::Color32::from_rgb(87, 204, 125),
            "Y",
        ),
        (
            [-yaw.sin(), yaw.cos() * pitch.sin()],
            egui::Color32::from_rgb(79, 147, 255),
            "Z",
        ),
    ];
    for (screen, color, label) in basis {
        let end = origin + egui::vec2(screen[0] * 25.0, -screen[1] * 25.0);
        ui.painter()
            .line_segment([origin, end], egui::Stroke::new(2.0, color));
        ui.painter().text(
            end,
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(12.0),
            color,
        );
    }
}

fn draw_scale_bar(ui: &mut egui::Ui, rect: egui::Rect, camera: &Camera) {
    let center = camera.target;
    let center_screen = project_world(camera, rect, center);
    let x_screen = project_world(camera, rect, [center[0] + 1.0, center[1], center[2]]);
    let Some((center_screen, x_screen)) = center_screen.zip(x_screen) else {
        return;
    };
    let pixels_per_block = (x_screen - center_screen).length().max(0.001);
    let target_blocks = 150.0 / pixels_per_block;
    let power = 10.0f32.powf(target_blocks.log10().floor());
    let normalized = target_blocks / power;
    let nice = if normalized < 1.5 {
        1.0
    } else if normalized < 3.5 {
        2.0
    } else if normalized < 7.5 {
        5.0
    } else {
        10.0
    };
    let blocks = (nice * power).max(1.0);
    let pixels = (blocks * pixels_per_block).clamp(36.0, 200.0);
    let end = egui::pos2(rect.right() - 24.0, rect.bottom() - 26.0);
    let start = end - egui::vec2(pixels, 0.0);
    let painter = ui.painter();
    let color = egui::Color32::from_rgb(226, 235, 243);
    painter.line_segment([start, end], egui::Stroke::new(1.5, color));
    for tick in 0..=4 {
        let x = start.x + pixels * tick as f32 / 4.0;
        painter.line_segment(
            [egui::pos2(x, end.y - 4.0), egui::pos2(x, end.y + 3.0)],
            egui::Stroke::new(1.0, color),
        );
        painter.text(
            egui::pos2(x, end.y + 6.0),
            egui::Align2::CENTER_TOP,
            format!("{:.0}", blocks * tick as f32 / 4.0),
            egui::FontId::proportional(9.0),
            color,
        );
    }
    painter.text(
        egui::pos2(start.x, start.y - 8.0),
        egui::Align2::CENTER_BOTTOM,
        "blocks",
        egui::FontId::proportional(10.0),
        color,
    );
}

fn multiply(a: [f32; 16], b: [f32; 16]) -> [f32; 16] {
    let mut result = [0.0; 16];
    for column in 0..4 {
        for row in 0..4 {
            result[column * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[column * 4 + k]).sum();
        }
    }
    result
}

#[derive(Clone)]
pub struct GlowRenderer {
    state: Arc<Mutex<GlowState>>,
    callback: Arc<CallbackFn>,
}

struct GlowState {
    revision: u64,
    gpu_revision: u64,
    pending: Option<(u64, CpuMesh)>,
    clear_revision: u64,
    program: Option<glow::Program>,
    position_attrib: u32,
    normal_attrib: u32,
    color_attrib: u32,
    mvp_uniform: Option<glow::UniformLocation>,
    alpha_uniform: Option<glow::UniformLocation>,
    chunks: Vec<GpuChunk>,
    transparent: Option<GpuTransparent>,
    camera: Camera,
    display_mode: DisplayMode,
    axes_visible: bool,
    grid_visible: bool,
    camera_revision: u64,
}

#[derive(Clone)]
struct GpuChunk {
    vao: glow::VertexArray,
    vertex_buffer: glow::Buffer,
    index_buffer: glow::Buffer,
    index_count: i32,
    wire_vao: glow::VertexArray,
    wire_vertex_buffer: glow::Buffer,
    wire_count: i32,
}

struct GpuTransparent {
    vao: glow::VertexArray,
    vertex_buffer: glow::Buffer,
    index_buffer: glow::Buffer,
    index_count: i32,
    faces: Vec<TransparentFace>,
    sorted_camera_revision: u64,
}

#[derive(Clone, Copy)]
struct TransparentFace {
    center: [f32; 3],
    indices: [u32; 6],
}

impl Default for GlowRenderer {
    fn default() -> Self {
        let state = Arc::new(Mutex::new(GlowState {
            revision: 0,
            gpu_revision: 0,
            pending: None,
            clear_revision: 0,
            program: None,
            position_attrib: 0,
            normal_attrib: 1,
            color_attrib: 2,
            mvp_uniform: None,
            alpha_uniform: None,
            chunks: Vec::new(),
            transparent: None,
            camera: Camera::default(),
            display_mode: DisplayMode::Solid,
            axes_visible: true,
            grid_visible: true,
            camera_revision: 0,
        }));
        let callback_state = state.clone();
        let callback = Arc::new(CallbackFn::new(move |info, painter: &Painter| {
            let gl = painter.gl().clone();
            let Ok(mut state) = callback_state.lock() else {
                return;
            };
            unsafe { paint_scene(&gl, &mut state, info) };
        }));
        Self { state, callback }
    }
}

impl GlowRenderer {
    pub fn set_mesh(&self, mesh: CpuMesh) {
        let mut state = self.state.lock().expect("renderer mutex poisoned");
        state.revision = state.revision.wrapping_add(1);
        let revision = state.revision;
        state.pending = Some((revision, mesh));
        state.clear_revision = revision;
    }

    pub fn fit(&self, size: [i32; 3], _origin: [i64; 3]) {
        let mut state = self.state.lock().expect("renderer mutex poisoned");
        state.camera.fit(size);
        state.camera_revision = state.camera_revision.wrapping_add(1);
    }

    pub fn set_camera_preset(&self, preset: CameraPreset) {
        let mut state = self.state.lock().expect("renderer mutex poisoned");
        let camera = &mut state.camera;
        match preset {
            CameraPreset::Fit => camera.fit(camera.bounds.map(|bound| bound.round() as i32)),
            CameraPreset::Reset => camera.reset(),
            CameraPreset::Front => {
                camera.yaw = std::f32::consts::PI;
                camera.pitch = 0.0;
                camera.orthographic = false;
            }
            CameraPreset::BirdEye => {
                camera.yaw = 0.65;
                camera.pitch = 0.82;
                camera.orthographic = false;
            }
            CameraPreset::Top => {
                camera.yaw = 0.0;
                camera.pitch = 1.54;
                camera.orthographic = true;
            }
            CameraPreset::Perspective => {
                camera.yaw = 0.75;
                camera.pitch = 0.55;
                camera.orthographic = false;
            }
            CameraPreset::Orthographic => camera.orthographic = true,
        }
        state.camera_revision = state.camera_revision.wrapping_add(1);
    }

    pub fn set_display_mode(&self, mode: DisplayMode) {
        self.state
            .lock()
            .expect("renderer mutex poisoned")
            .display_mode = mode;
    }

    pub fn set_axes_visible(&self, visible: bool) {
        self.state
            .lock()
            .expect("renderer mutex poisoned")
            .axes_visible = visible;
    }

    pub fn set_grid_visible(&self, visible: bool) {
        self.state
            .lock()
            .expect("renderer mutex poisoned")
            .grid_visible = visible;
    }

    pub fn clear(&self) {
        let mut state = self.state.lock().expect("renderer mutex poisoned");
        state.revision = state.revision.wrapping_add(1);
        state.pending = None;
        state.clear_revision = state.revision;
    }

    pub fn with_camera(&self, update: impl FnOnce(&mut Camera)) {
        let mut state = self.state.lock().expect("renderer mutex poisoned");
        update(&mut state.camera);
        state.camera_revision = state.camera_revision.wrapping_add(1);
    }

    pub fn is_orthographic(&self) -> bool {
        self.state
            .lock()
            .expect("renderer mutex poisoned")
            .camera
            .orthographic
    }

    pub fn draw(&self, ui: &mut egui::Ui, rect: egui::Rect) {
        let (camera, axes_visible, grid_visible) = {
            let state = self.state.lock().expect("renderer mutex poisoned");
            (state.camera.clone(), state.axes_visible, state.grid_visible)
        };
        if grid_visible {
            draw_reference_grid(ui, rect, &camera);
        }
        ui.painter().add(PaintCallback {
            rect,
            callback: self.callback.clone(),
        });
        if axes_visible {
            draw_axis_gizmo(ui, rect, &camera);
        }
        draw_scale_bar(ui, rect, &camera);
    }

    pub fn release_gl(&self, gl: Option<&glow::Context>) {
        let Some(gl) = gl else { return };
        if let Ok(mut state) = self.state.lock() {
            unsafe { release_state(gl, &mut state) };
        }
    }
}

unsafe fn paint_scene(
    gl: &glow::Context,
    state: &mut GlowState,
    info: egui::epaint::PaintCallbackInfo,
) {
    let viewport = info.viewport_in_pixels();
    let clip = info.clip_rect_in_pixels();
    if viewport.width_px == 0
        || viewport.height_px == 0
        || clip.width_px == 0
        || clip.height_px == 0
    {
        return;
    }
    let mut old_viewport = [0; 4];
    let mut old_scissor = [0; 4];
    gl.get_parameter_i32_slice(glow::VIEWPORT, &mut old_viewport);
    gl.get_parameter_i32_slice(glow::SCISSOR_BOX, &mut old_scissor);
    let old_program = gl.get_parameter_i32(glow::CURRENT_PROGRAM);
    let old_vao = gl.get_parameter_i32(glow::VERTEX_ARRAY_BINDING);
    let old_array = gl.get_parameter_i32(glow::ARRAY_BUFFER_BINDING);
    let old_element = gl.get_parameter_i32(glow::ELEMENT_ARRAY_BUFFER_BINDING);
    let old_depth_func = gl.get_parameter_i32(glow::DEPTH_FUNC) as u32;
    let old_depth_mask = gl.get_parameter_bool(glow::DEPTH_WRITEMASK);
    let old_line_width = gl.get_parameter_f32(glow::LINE_WIDTH);
    let old_scissor_enabled = gl.is_enabled(glow::SCISSOR_TEST);
    let old_depth_enabled = gl.is_enabled(glow::DEPTH_TEST);
    let old_blend_enabled = gl.is_enabled(glow::BLEND);
    let old_cull_enabled = gl.is_enabled(glow::CULL_FACE);
    let old_blend_src_rgb = gl.get_parameter_i32(glow::BLEND_SRC_RGB) as u32;
    let old_blend_dst_rgb = gl.get_parameter_i32(glow::BLEND_DST_RGB) as u32;
    let old_blend_src_alpha = gl.get_parameter_i32(glow::BLEND_SRC_ALPHA) as u32;
    let old_blend_dst_alpha = gl.get_parameter_i32(glow::BLEND_DST_ALPHA) as u32;

    if state.pending.is_some() {
        if let Some((revision, mesh)) = state.pending.take() {
            release_chunks(gl, &mut state.chunks);
            release_transparent(gl, &mut state.transparent);
            state.pending = None;
            state.revision = revision;
            state.gpu_revision = revision;
            if state.program.is_none() {
                if let Err(error) = init_program(gl, state) {
                    eprintln!("preview shader setup failed: {error}");
                }
            }
            if state.program.is_some() {
                for chunk in &mesh.chunks {
                    if let Some(gpu) = upload_chunk(gl, state, chunk) {
                        state.chunks.push(gpu);
                    }
                }
                if let Some(transparent) = upload_transparent(&gl, state, &mesh) {
                    state.transparent = Some(transparent);
                }
            }
        }
    } else if state.gpu_revision != state.clear_revision && state.pending.is_none() {
        release_chunks(gl, &mut state.chunks);
        release_transparent(gl, &mut state.transparent);
        state.gpu_revision = state.clear_revision;
    }

    if !state.chunks.is_empty() {
        let clip_y = clip.from_bottom_px;
        let viewport_y = viewport.from_bottom_px;
        gl.enable(glow::SCISSOR_TEST);
        gl.scissor(
            clip.left_px as i32,
            clip_y,
            clip.width_px as i32,
            clip.height_px as i32,
        );
        gl.viewport(
            viewport.left_px as i32,
            viewport_y,
            viewport.width_px as i32,
            viewport.height_px as i32,
        );
        gl.depth_mask(true);
        gl.clear(glow::DEPTH_BUFFER_BIT);
        gl.enable(glow::DEPTH_TEST);
        gl.depth_func(glow::LEQUAL);
        gl.enable(glow::BLEND);
        gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);
        gl.disable(glow::CULL_FACE);
        if let Some(program) = state.program {
            gl.use_program(Some(program));
            let aspect = viewport.width_px as f32 / viewport.height_px.max(1) as f32;
            let matrix = state.camera.view_projection(aspect);
            if let Some(uniform) = &state.mvp_uniform {
                gl.uniform_matrix_4_f32_slice(Some(uniform), false, &matrix);
            }
            if let Some(uniform) = &state.alpha_uniform {
                let alpha = if state.display_mode == DisplayMode::Translucent {
                    0.34
                } else {
                    1.0
                };
                gl.uniform_1_f32(Some(uniform), alpha);
            }
            if state.display_mode == DisplayMode::Translucent {
                if let Some(transparent) = &mut state.transparent {
                    if transparent.sorted_camera_revision != state.camera_revision {
                        sort_transparent_faces(
                            gl,
                            transparent,
                            state.camera.eye(),
                            state.camera.forward(),
                            state.camera_revision,
                        );
                    }
                }
            }
            if state.display_mode == DisplayMode::Translucent {
                gl.depth_mask(false);
                if let Some(transparent) = &state.transparent {
                    gl.bind_vertex_array(Some(transparent.vao));
                    gl.draw_elements(
                        glow::TRIANGLES,
                        transparent.index_count,
                        glow::UNSIGNED_INT,
                        0,
                    );
                } else {
                    gl.depth_mask(true);
                    for chunk in &state.chunks {
                        gl.bind_vertex_array(Some(chunk.vao));
                        gl.draw_elements(glow::TRIANGLES, chunk.index_count, glow::UNSIGNED_INT, 0);
                    }
                    gl.depth_mask(false);
                }
            } else if state.display_mode == DisplayMode::Solid {
                for chunk in &state.chunks {
                    gl.bind_vertex_array(Some(chunk.vao));
                    gl.draw_elements(glow::TRIANGLES, chunk.index_count, glow::UNSIGNED_INT, 0);
                }
            }
            gl.depth_mask(true);
            gl.line_width(1.0);
            for chunk in &state.chunks {
                if chunk.wire_count > 0 {
                    gl.bind_vertex_array(Some(chunk.wire_vao));
                    gl.draw_arrays(glow::LINES, 0, chunk.wire_count);
                }
            }
        }
    }

    gl.bind_vertex_array(native_vao(old_vao));
    gl.bind_buffer(glow::ARRAY_BUFFER, native_buffer(old_array));
    gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, native_buffer(old_element));
    gl.use_program(native_program(old_program));
    if old_depth_enabled {
        gl.enable(glow::DEPTH_TEST)
    } else {
        gl.disable(glow::DEPTH_TEST)
    }
    if old_blend_enabled {
        gl.enable(glow::BLEND)
    } else {
        gl.disable(glow::BLEND)
    }
    gl.blend_func_separate(
        old_blend_src_rgb,
        old_blend_dst_rgb,
        old_blend_src_alpha,
        old_blend_dst_alpha,
    );
    if old_cull_enabled {
        gl.enable(glow::CULL_FACE)
    } else {
        gl.disable(glow::CULL_FACE)
    }
    if old_scissor_enabled {
        gl.enable(glow::SCISSOR_TEST)
    } else {
        gl.disable(glow::SCISSOR_TEST)
    }
    gl.depth_func(old_depth_func);
    gl.depth_mask(old_depth_mask);
    gl.line_width(old_line_width);
    gl.viewport(
        old_viewport[0],
        old_viewport[1],
        old_viewport[2],
        old_viewport[3],
    );
    gl.scissor(
        old_scissor[0],
        old_scissor[1],
        old_scissor[2],
        old_scissor[3],
    );
}

fn native_program(raw: i32) -> Option<glow::Program> {
    NonZeroU32::new(raw as u32).map(glow::NativeProgram)
}

fn native_buffer(raw: i32) -> Option<glow::Buffer> {
    NonZeroU32::new(raw as u32).map(glow::NativeBuffer)
}

fn native_vao(raw: i32) -> Option<glow::VertexArray> {
    NonZeroU32::new(raw as u32).map(glow::NativeVertexArray)
}

unsafe fn init_program(gl: &glow::Context, state: &mut GlowState) -> Result<(), String> {
    let vertex = gl.create_shader(glow::VERTEX_SHADER)?;
    gl.shader_source(vertex, "#version 150 core\nin vec3 a_pos;\nin vec3 a_normal;\nin vec4 a_color;\nuniform mat4 u_mvp;\nuniform float u_alpha;\nout vec4 v_color;\nvoid main(){ gl_Position=u_mvp*vec4(a_pos,1.0); float light=dot(normalize(a_normal),normalize(vec3(0.45,0.8,0.35)))*0.18+0.82; v_color=vec4(a_color.rgb*light,a_color.a*u_alpha); }");
    gl.compile_shader(vertex);
    if !gl.get_shader_compile_status(vertex) {
        let error = gl.get_shader_info_log(vertex);
        gl.delete_shader(vertex);
        return Err(error);
    }
    let fragment = match gl.create_shader(glow::FRAGMENT_SHADER) {
        Ok(fragment) => fragment,
        Err(error) => {
            gl.delete_shader(vertex);
            return Err(error);
        }
    };
    gl.shader_source(fragment, "#version 150 core\nin vec4 v_color;\nout vec4 out_color;\nvoid main(){ out_color=v_color; }");
    gl.compile_shader(fragment);
    if !gl.get_shader_compile_status(fragment) {
        let error = gl.get_shader_info_log(fragment);
        gl.delete_shader(vertex);
        gl.delete_shader(fragment);
        return Err(error);
    }
    let program = match gl.create_program() {
        Ok(program) => program,
        Err(error) => {
            gl.delete_shader(vertex);
            gl.delete_shader(fragment);
            return Err(error);
        }
    };
    gl.attach_shader(program, vertex);
    gl.attach_shader(program, fragment);
    gl.bind_attrib_location(program, 0, "a_pos");
    gl.bind_attrib_location(program, 1, "a_normal");
    gl.bind_attrib_location(program, 2, "a_color");
    gl.link_program(program);
    gl.delete_shader(vertex);
    gl.delete_shader(fragment);
    if !gl.get_program_link_status(program) {
        let error = gl.get_program_info_log(program);
        gl.delete_program(program);
        return Err(error);
    }
    state.position_attrib = 0;
    state.normal_attrib = 1;
    state.color_attrib = 2;
    state.mvp_uniform = gl.get_uniform_location(program, "u_mvp");
    state.alpha_uniform = gl.get_uniform_location(program, "u_alpha");
    state.program = Some(program);
    Ok(())
}

unsafe fn upload_chunk(
    gl: &glow::Context,
    state: &GlowState,
    chunk: &CpuChunk,
) -> Option<GpuChunk> {
    if chunk.indices.is_empty() {
        return None;
    }
    let vao = gl.create_vertex_array().ok()?;
    let vertex_buffer = match gl.create_buffer() {
        Ok(buffer) => buffer,
        Err(_) => {
            gl.delete_vertex_array(vao);
            return None;
        }
    };
    let index_buffer = match gl.create_buffer() {
        Ok(buffer) => buffer,
        Err(_) => {
            gl.delete_vertex_array(vao);
            gl.delete_buffer(vertex_buffer);
            return None;
        }
    };
    let wire_vao = match gl.create_vertex_array() {
        Ok(vao) => vao,
        Err(_) => {
            gl.delete_vertex_array(vao);
            gl.delete_buffer(vertex_buffer);
            gl.delete_buffer(index_buffer);
            return None;
        }
    };
    let wire_vertex_buffer = match gl.create_buffer() {
        Ok(buffer) => buffer,
        Err(_) => {
            gl.delete_vertex_array(vao);
            gl.delete_buffer(vertex_buffer);
            gl.delete_buffer(index_buffer);
            gl.delete_vertex_array(wire_vao);
            return None;
        }
    };
    gl.bind_vertex_array(Some(vao));
    gl.bind_buffer(glow::ARRAY_BUFFER, Some(vertex_buffer));
    let vertices = std::slice::from_raw_parts(
        chunk.vertices.as_ptr() as *const u8,
        chunk.vertices.len() * std::mem::size_of::<f32>(),
    );
    gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, vertices, glow::STATIC_DRAW);
    gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(index_buffer));
    let indices = std::slice::from_raw_parts(
        chunk.indices.as_ptr() as *const u8,
        chunk.indices.len() * std::mem::size_of::<u32>(),
    );
    gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, indices, glow::DYNAMIC_DRAW);
    let stride = (10 * std::mem::size_of::<f32>()) as i32;
    for (index, size, offset) in [
        (state.position_attrib, 3, 0),
        (state.normal_attrib, 3, 3),
        (state.color_attrib, 4, 6),
    ] {
        gl.enable_vertex_attrib_array(index);
        gl.vertex_attrib_pointer_f32(index, size, glow::FLOAT, false, stride, offset * 4);
    }
    gl.bind_vertex_array(Some(wire_vao));
    gl.bind_buffer(glow::ARRAY_BUFFER, Some(wire_vertex_buffer));
    let wire_vertices = std::slice::from_raw_parts(
        chunk.wire_vertices.as_ptr() as *const u8,
        chunk.wire_vertices.len() * std::mem::size_of::<f32>(),
    );
    gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, wire_vertices, glow::STATIC_DRAW);
    let stride = (10 * std::mem::size_of::<f32>()) as i32;
    for (index, size, offset) in [
        (state.position_attrib, 3, 0),
        (state.normal_attrib, 3, 3),
        (state.color_attrib, 4, 6),
    ] {
        gl.enable_vertex_attrib_array(index);
        gl.vertex_attrib_pointer_f32(index, size, glow::FLOAT, false, stride, offset * 4);
    }
    gl.bind_vertex_array(None);
    Some(GpuChunk {
        vao,
        vertex_buffer,
        index_buffer,
        index_count: chunk.indices.len() as i32,
        wire_vao,
        wire_vertex_buffer,
        wire_count: (chunk.wire_vertices.len() / 10) as i32,
    })
}

unsafe fn upload_transparent(
    gl: &glow::Context,
    state: &GlowState,
    mesh: &CpuMesh,
) -> Option<GpuTransparent> {
    if mesh.face_count == 0 {
        return None;
    }
    let mut vertices = Vec::new();
    let mut faces = Vec::with_capacity(mesh.face_count);
    for chunk in &mesh.chunks {
        let vertex_base = u32::try_from(vertices.len() / 10).ok()?;
        vertices.extend_from_slice(&chunk.vertices);
        for indices in chunk.indices.chunks_exact(6) {
            let base = *indices.first()? as usize;
            let points = [0usize, 1, 2, 3].map(|offset| {
                let start = (base + offset) * 10;
                [
                    chunk.vertices[start],
                    chunk.vertices[start + 1],
                    chunk.vertices[start + 2],
                ]
            });
            let mut global_indices = [0u32; 6];
            for (slot, index) in global_indices.iter_mut().zip(indices) {
                *slot = vertex_base.checked_add(*index)?;
            }
            faces.push(TransparentFace {
                center: [
                    points.iter().map(|point| point[0]).sum::<f32>() * 0.25,
                    points.iter().map(|point| point[1]).sum::<f32>() * 0.25,
                    points.iter().map(|point| point[2]).sum::<f32>() * 0.25,
                ],
                indices: global_indices,
            });
        }
    }
    if faces.len() > MAX_PREVIEW_FACES {
        return None;
    }
    let indices: Vec<u32> = faces.iter().flat_map(|face| face.indices).collect();
    let vao = gl.create_vertex_array().ok()?;
    let vertex_buffer = match gl.create_buffer() {
        Ok(buffer) => buffer,
        Err(_) => {
            gl.delete_vertex_array(vao);
            return None;
        }
    };
    let index_buffer = match gl.create_buffer() {
        Ok(buffer) => buffer,
        Err(_) => {
            gl.delete_vertex_array(vao);
            gl.delete_buffer(vertex_buffer);
            return None;
        }
    };
    gl.bind_vertex_array(Some(vao));
    gl.bind_buffer(glow::ARRAY_BUFFER, Some(vertex_buffer));
    let vertex_bytes = std::slice::from_raw_parts(
        vertices.as_ptr() as *const u8,
        vertices.len() * std::mem::size_of::<f32>(),
    );
    gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, vertex_bytes, glow::STATIC_DRAW);
    gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(index_buffer));
    let index_bytes = std::slice::from_raw_parts(
        indices.as_ptr() as *const u8,
        indices.len() * std::mem::size_of::<u32>(),
    );
    gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, index_bytes, glow::DYNAMIC_DRAW);
    let stride = (10 * std::mem::size_of::<f32>()) as i32;
    for (index, size, offset) in [
        (state.position_attrib, 3, 0),
        (state.normal_attrib, 3, 3),
        (state.color_attrib, 4, 6),
    ] {
        gl.enable_vertex_attrib_array(index);
        gl.vertex_attrib_pointer_f32(index, size, glow::FLOAT, false, stride, offset * 4);
    }
    gl.bind_vertex_array(None);
    Some(GpuTransparent {
        vao,
        vertex_buffer,
        index_buffer,
        index_count: indices.len() as i32,
        faces,
        sorted_camera_revision: u64::MAX,
    })
}

unsafe fn sort_transparent_faces(
    gl: &glow::Context,
    mesh: &mut GpuTransparent,
    eye: [f32; 3],
    forward: [f32; 3],
    camera_revision: u64,
) {
    mesh.faces.sort_by(|left, right| {
        view_depth(right.center, eye, forward).total_cmp(&view_depth(left.center, eye, forward))
    });
    let sorted: Vec<u32> = mesh.faces.iter().flat_map(|face| face.indices).collect();
    gl.bind_vertex_array(Some(mesh.vao));
    gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(mesh.index_buffer));
    let bytes = std::slice::from_raw_parts(
        sorted.as_ptr() as *const u8,
        sorted.len() * std::mem::size_of::<u32>(),
    );
    gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, bytes, glow::DYNAMIC_DRAW);
    gl.bind_vertex_array(None);
    mesh.sorted_camera_revision = camera_revision;
}

unsafe fn release_chunks(gl: &glow::Context, chunks: &mut Vec<GpuChunk>) {
    for chunk in chunks.drain(..) {
        gl.delete_vertex_array(chunk.vao);
        gl.delete_buffer(chunk.vertex_buffer);
        gl.delete_buffer(chunk.index_buffer);
        gl.delete_vertex_array(chunk.wire_vao);
        gl.delete_buffer(chunk.wire_vertex_buffer);
    }
}

unsafe fn release_transparent(gl: &glow::Context, mesh: &mut Option<GpuTransparent>) {
    if let Some(mesh) = mesh.take() {
        gl.delete_vertex_array(mesh.vao);
        gl.delete_buffer(mesh.vertex_buffer);
        gl.delete_buffer(mesh.index_buffer);
    }
}

unsafe fn release_state(gl: &glow::Context, state: &mut GlowState) {
    release_chunks(gl, &mut state.chunks);
    release_transparent(gl, &mut state.transparent);
    if let Some(program) = state.program.take() {
        gl.delete_program(program);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(position: [i32; 3], name: &str) -> BlockInput {
        BlockInput {
            position,
            region_id: 0,
            name: name.into(),
            properties: HashMap::new(),
        }
    }

    #[test]
    fn adjacent_cubes_hide_internal_faces() {
        let mesh = build_mesh(
            &[
                block([0, 0, 0], "minecraft:stone"),
                block([1, 0, 0], "minecraft:stone"),
            ],
            LayerView::All,
            || false,
        )
        .unwrap();
        assert_eq!(mesh.block_count, 2);
        assert_eq!(mesh.face_count, 6);
    }

    #[test]
    fn a_single_layer_including_zero_is_distinct_from_all() {
        let blocks = [
            block([0, 0, 0], "minecraft:stone"),
            block([0, 1, 0], "minecraft:dirt"),
        ];
        let all = build_mesh(&blocks, LayerView::All, || false).unwrap();
        let layer = build_mesh(&blocks, LayerView::Layer(0), || false).unwrap();
        assert_eq!(all.block_count, 2);
        assert_eq!(layer.block_count, 1);
        assert_eq!(layer.face_count, 6);
    }

    #[test]
    fn chunks_are_spatially_bounded_and_color_hash_is_stable() {
        let blocks = [block([0, 0, 0], "mod:a"), block([16, 0, 0], "mod:b")];
        let mesh = build_mesh(&blocks, LayerView::All, || false).unwrap();
        assert_eq!(mesh.chunks.len(), 2);
        assert_eq!(block_color("mod:test"), block_color("mod:test"));
    }

    #[test]
    fn partial_blocks_use_simplified_geometry() {
        let cube = build_mesh(
            &[block([0, 0, 0], "minecraft:stone")],
            LayerView::All,
            || false,
        )
        .unwrap();
        let slab = build_mesh(
            &[block([0, 0, 0], "minecraft:stone_slab")],
            LayerView::All,
            || false,
        )
        .unwrap();
        assert_eq!(cube.face_count, 6);
        assert_eq!(slab.face_count, 6);
        assert_ne!(cube.chunks[0].vertices, slab.chunks[0].vertices);
    }

    #[test]
    fn cube_face_winding_points_outward_for_every_axis() {
        let position = [3, 5, 7];
        let normals = [
            [-1.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, -1.0],
            [0.0, 0.0, 1.0],
        ];
        for axis in 0..3 {
            for positive in [false, true] {
                let normal_index = axis * 2 + usize::from(positive);
                let corners = face_rect_corners(
                    axis,
                    positive,
                    [0; 3],
                    if positive {
                        position[axis] + 1
                    } else {
                        position[axis]
                    },
                    match axis {
                        0 => position[2],
                        1 => position[0],
                        _ => position[0],
                    },
                    match axis {
                        0 => position[1],
                        1 => position[2],
                        _ => position[1],
                    },
                    1,
                    1,
                );
                let a = corners[0];
                let b = corners[1];
                let c = corners[2];
                let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                let cross = [
                    ab[1] * ac[2] - ab[2] * ac[1],
                    ab[2] * ac[0] - ab[0] * ac[2],
                    ab[0] * ac[1] - ab[1] * ac[0],
                ];
                let dot = cross[0] * normals[normal_index][0]
                    + cross[1] * normals[normal_index][1]
                    + cross[2] * normals[normal_index][2];
                assert!(
                    dot > 0.0,
                    "axis={axis} positive={positive} has inward winding"
                );
            }
        }
    }

    #[test]
    fn camera_frames_model_center_and_targets_inside_clip_depth() {
        let mut camera = Camera::default();
        camera.fit([20, 10, 30]);
        let center = camera.target;
        let mvp = camera.view_projection(16.0 / 9.0);
        let clip = transform_point(mvp, center);
        assert!(clip[0].abs() < 1e-5 && clip[1].abs() < 1e-5);
        assert!(
            clip[2] >= -1.0 && clip[2] <= 1.0,
            "center outside clip depth: {}",
            clip[2]
        );
    }

    fn transform_point(matrix: [f32; 16], point: [f32; 3]) -> [f32; 3] {
        let x = matrix[0] * point[0] + matrix[4] * point[1] + matrix[8] * point[2] + matrix[12];
        let y = matrix[1] * point[0] + matrix[5] * point[1] + matrix[9] * point[2] + matrix[13];
        let z = matrix[2] * point[0] + matrix[6] * point[1] + matrix[10] * point[2] + matrix[14];
        let w = matrix[3] * point[0] + matrix[7] * point[1] + matrix[11] * point[2] + matrix[15];
        [x / w, y / w, z / w]
    }

    #[test]
    fn viewport_and_scissor_use_physical_pixels_at_high_dpi() {
        let points = egui::Rect::from_min_max(egui::pos2(10.0, 20.0), egui::pos2(110.0, 70.0));
        let pixels = egui::epaint::ViewportInPixels::from_points(&points, 2.0, [400, 300]);
        assert_eq!(pixels.left_px, 20);
        assert_eq!(pixels.top_px, 40);
        assert_eq!(pixels.width_px, 200);
        assert_eq!(pixels.height_px, 100);
        assert_eq!(pixels.from_bottom_px, 160);
    }
}
