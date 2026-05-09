use std::{
    fs, io::{self, Read, Write}, str::FromStr
};

use clap::Parser;

#[derive(clap::Parser)]
struct Args {
    #[arg(short, long)]
    resolution: Resolution,

    #[arg(short, long)]
    out: String,
}

#[derive(Clone)]
struct Resolution {
    width: u32,
    height: u32,
}

#[derive(thiserror::Error, Debug)]
#[error("Error parsing resolution from {0}")]
struct ParseResolutionError(String);

impl FromStr for Resolution {
    type Err = ParseResolutionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (width, height) = s
            .split_once(":")
            .ok_or_else(|| ParseResolutionError(s.to_string()))?;
        let width = width
            .parse::<u32>()
            .map_err(|_| ParseResolutionError(s.to_string()))?;
        let height = height
            .parse::<u32>()
            .map_err(|_| ParseResolutionError(s.to_string()))?;

        Ok(Self { width, height })
    }
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let Resolution { width, height } = args.resolution;
    let frame_size = (width * height / 8) as usize;

    println!("resolution: {width}:{height}");
    let reader = io::BufReader::new(io::stdin().lock());

    let mut out_bytes: Vec<u8> = vec![];
    let mut counter = 0;
    let mut current_packed_byte = 0u8;
    for byte in reader.bytes() {
        let byte = byte?;

        let bit = (byte < 128) as u8;
        current_packed_byte |= bit << counter % 8;

        counter += 1;
        if counter % 8 == 0 {
            out_bytes.push(current_packed_byte);
            current_packed_byte = 0u8;
        }
    }

    let frame_count = out_bytes.len() / frame_size;
    println!("frame count: {frame_count}");
    println!("out bytes (uncompressed): {}", out_bytes.len());

    let buf = io::Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(buf);

    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .compression_level(Some(9));

    for frame in 0..frame_count {
        zip.start_file(format!("{frame:08}.frame"), options)?;
        zip.write_all(&out_bytes[frame..frame+frame_size])?;
    }

    let buf = zip.finish()?;

    fs::write(&args.out, buf.into_inner())?;

    Ok(())
}
