use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use clap::{Parser, ValueEnum};
use foxglove::McapWriter;
use mcap::{Compression, WriteOptions};
use std::time::Duration;

#[derive(Debug, Parser)]
struct Cli {
    /// Output path.
    #[arg(short, long, default_value = "output.mcap")]
    path: PathBuf,
    /// If set, overwrite an existing file.
    #[arg(long)]
    overwrite: bool,
    /// Chunk size.
    #[arg(long, default_value_t = 1024 * 768)]
    chunk_size: u64,
    /// Compression algorithm to use.
    #[arg(long, default_value = "zstd")]
    compression: CompressionArg,
    /// Frames per second.
    #[arg(long, default_value_t = 10)]
    fps: u8,
}

#[derive(Debug, Clone, ValueEnum)]
enum CompressionArg {
    Zstd,
    Lz4,
    None,
}
impl From<CompressionArg> for Option<Compression> {
    fn from(value: CompressionArg) -> Self {
        match value {
            CompressionArg::Zstd => Some(Compression::Zstd),
            CompressionArg::Lz4 => Some(Compression::Lz4),
            CompressionArg::None => None,
        }
    }
}

#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
struct Message {
    msg: String,
    count: u32,
}

foxglove::static_typed_channel!(pub MSG_CHANNEL, "/msg", Message);

fn log_until(fps: u8, stop: Arc<AtomicBool>) {
    let mut count: u32 = 0;
    let duration = Duration::from_millis(1000 / u64::from(fps));
    while !stop.load(Ordering::Relaxed) {
        MSG_CHANNEL.log(&Message {
            msg: "Hello, world!".to_string(),
            count,
        });
        std::thread::sleep(duration);
        count += 1;
    }
}

fn main() {
    let env = env_logger::Env::default().default_filter_or("debug");
    env_logger::init_from_env(env);

    let args = Cli::parse();

    let done = Arc::new(AtomicBool::default());
    ctrlc::set_handler({
        let done = done.clone();
        move || {
            done.store(true, Ordering::Relaxed);
        }
    })
    .expect("Failed to set SIGINT handler");

    if args.overwrite && args.path.exists() {
        std::fs::remove_file(&args.path).expect("Failed to remove file");
    }

    let options = WriteOptions::new()
        .chunk_size(Some(args.chunk_size))
        .compression(args.compression.into());

    let writer = McapWriter::with_options(options)
        .create_new_buffered_file(&args.path)
        .expect("Failed to start mcap writer");

    log_until(args.fps, done);
    writer.close().expect("Failed to flush mcap file");
}
