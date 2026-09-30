use bluepanda_core::{convert_batch, ConversionOptions};
use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "bluepanda-change",
    version,
    about = "将 Litematica 或 Sponge 蓝图转换为 Minecraft Structure NBT",
    long_about = "批量将 Litematica（.litematic）或 Sponge（.schem）蓝图转换为压缩的 Minecraft Structure NBT 文件。输出文件名为输入文件名后追加 .nbt；单个文件失败不会中断后续转换。"
)]
struct Args {
    /// 一个或多个 .litematic 或 .schem 蓝图文件
    #[arg(required = true, value_name = "INPUT")]
    inputs: Vec<PathBuf>,
    /// 保存转换结果的目录；输出名为原文件名后追加 .nbt
    #[arg(long, value_name = "DIR")]
    output_dir: PathBuf,
    /// 允许覆盖已存在的输出文件（默认保留原文件并报告失败）
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
