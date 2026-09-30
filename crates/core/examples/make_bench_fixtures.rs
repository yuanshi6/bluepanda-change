use fastnbt::Value;
use flate2::write::GzEncoder;
use flate2::Compression;
use std::collections::HashMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

fn compound(values: impl IntoIterator<Item = (String, Value)>) -> Value {
    Value::Compound(values.into_iter().collect::<HashMap<_, _>>())
}

fn vec3(x: i32, y: i32, z: i32) -> Value {
    compound([
        ("x".into(), Value::Int(x)),
        ("y".into(), Value::Int(y)),
        ("z".into(), Value::Int(z)),
    ])
}

fn region(name: &str, pos: [i32; 3], x_size: usize) -> (String, Value) {
    let volume = x_size;
    let long_count = (volume * 2 + 63) / 64;
    let stone = compound([("Name".into(), Value::String("minecraft:stone".into()))]);
    (
        name.to_owned(),
        compound([
            ("Position".into(), vec3(pos[0], pos[1], pos[2])),
            ("Size".into(), vec3(x_size as i32, 1, 1)),
            ("BlockStatePalette".into(), Value::List(vec![stone])),
            (
                "BlockStates".into(),
                Value::LongArray(fastnbt::LongArray::new(vec![0; long_count])),
            ),
        ]),
    )
}

fn write_sample(path: &Path, regions: Vec<(String, Value)>) {
    let root = compound([
        ("Version".into(), Value::Int(6)),
        ("SubVersion".into(), Value::Int(1)),
        ("MinecraftDataVersion".into(), Value::Int(3700)),
        ("Regions".into(), compound(regions)),
    ]);
    let mut gzip = GzEncoder::new(File::create(path).unwrap(), Compression::default());
    fastnbt::to_writer(&mut gzip, &root).unwrap();
    gzip.finish().unwrap();
}

fn main() {
    let dir = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .expect("usage: make_bench_fixtures DIR"),
    );
    fs::create_dir_all(&dir).unwrap();
    for size in [100_000usize, 1_000_000] {
        write_sample(
            &dir.join(format!("stone-{size}.litematic")),
            vec![region("Stone", [0, 0, 0], size)],
        );
    }
    write_sample(
        &dir.join("distant-regions.litematic"),
        vec![
            region("Near", [0, 0, 0], 1),
            region("Far", [1_000_000_000, 0, 0], 1),
        ],
    );
}
