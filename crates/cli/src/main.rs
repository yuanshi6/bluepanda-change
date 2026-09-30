use bluepanda_core::{convert_batch, ConversionOptions};
use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "bluepanda-change",
    version,
    about = "将 Litematica 蓝图转换为 Minecraft Structure NBT"
)]
struct Args {
    /// 一个或多个 .litematic 文件
    #[arg(required = true, value_name = "INPUT")]
    inputs: Vec<PathBuf>,
    /// 输出目录
    #[arg(long, value_name = "DIR")]
    output_dir: PathBuf,
    /// 覆盖已存在的输出文件
    #[arg(long)]
    overwrite: bool,
}

fn main() {
    let args = Args::parse();
    let options = ConversionOptions {
        overwrite: args.overwrite,
    };
    let batch = convert_batch(&args.inputs, &args.output_dir, &options, |event| {
        eprintln!("{}: {:?}", event.input.display(), event.stage);
    });
    let mut failed = false;
    for file in batch.results {
        match file.result {
            Ok(result) => {
                println!(
                    "已转换: {} -> {} ({} 方块, {} 实体)",
                    file.input.display(),
                    file.output.display(),
                    result.blocks,
                    result.entities
                );
                for warning in result.warnings {
                    eprintln!(
                        "提示{}: {}",
                        warning
                            .region
                            .map(|r| format!(" [{r}]"))
                            .unwrap_or_default(),
                        warning.message
                    );
                }
            }
            Err(error) => {
                failed = true;
                eprintln!(
                    "转换失败 [{}]: {}{}{} -> {}",
                    format!("{:?}", error.stage),
                    error.message,
                    error
                        .region
                        .map(|r| format!(" (Region {r})"))
                        .unwrap_or_default(),
                    error
                        .position
                        .map(|p| format!(" at {},{},{}", p[0], p[1], p[2]))
                        .unwrap_or_default(),
                    file.input.display()
                );
            }
        }
    }
    if failed {
        std::process::exit(1);
    }
}
