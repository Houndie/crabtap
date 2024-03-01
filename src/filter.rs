use std::{ffi::OsStr, path::Path};

use clap::Parser;

mod file;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    inputs: Vec<String>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let inputs = args
        .inputs
        .into_iter()
        .map(|input| -> Result<Box<dyn file::Music>, anyhow::Error> {
            let f = match Path::new(&input).extension().and_then(OsStr::to_str) {
                Some("mp3") => Box::new(file::Mp3::new(input)?) as Box<dyn file::Music>,
                Some("flac") => Box::new(file::Flac::new(input)?) as Box<dyn file::Music>,
                _ => return Err(anyhow::anyhow!("Unsupported file type")),
            };

            Ok(f)
        })
        .collect::<Result<Vec<_>, _>>()?;

    inputs
        .into_iter()
        .filter(|f| f.bpm().is_none())
        .for_each(|f| {
            println!("{}", f.path());
        });

    Ok(())
}
