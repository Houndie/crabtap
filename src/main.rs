use clap::Parser;
use id3::TagLike;
use std::collections::{hash_map, HashMap};
use std::fs;
use std::io::{self, Write};
use std::process::{Child, Stdio};
use std::{ffi::OsStr, path::Path, process::Command};
use termion::{event::Key, input::TermRead, raw::IntoRawMode};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(short, long)]
    output_directory: Option<String>,

    #[arg(long)]
    inplace: bool,

    #[arg(short, long)]
    filter_existing: bool,

    inputs: Vec<String>,
}

enum State {
    Playing {
        input_idx: usize,
        player: std::process::Child,
        last_press_at: Option<chrono::DateTime<chrono::Utc>>,
        bpms: Vec<f64>,
    },
    Finished {
        input_idx: usize,
        bpm: u32,
    },
}

enum PlayCommands {
    Skip,
    Quit,
    Confirm,
    Restart,
    Tap,
}

enum ConfirmCommands {
    Yes,
    No,
}

fn on_keypress<Command>(
    iter: impl IntoIterator<Item = (Key, Command)>,
) -> Result<Command, anyhow::Error> {
    let mut commands = iter.into_iter().collect::<HashMap<Key, Command>>();
    for key in io::stdin().keys() {
        let key = key?;

        if let hash_map::Entry::Occupied(entry) = commands.entry(key) {
            return Ok(entry.remove());
        }
    }

    Err(anyhow::anyhow!("No keypress found"))
}

fn avg_bpm(bpms: &[f64]) -> u32 {
    (bpms.iter().sum::<f64>() / bpms.len() as f64) as u32
}

enum Output {
    ToDirectory(String),
    InPlace,
}

fn play(input: &str) -> Result<Child, anyhow::Error> {
    Command::new("mpv")
        .arg(input)
        .arg("--no-video")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(Into::into)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let output = match (args.output_directory, args.inplace) {
        (Some(dir), false) => Output::ToDirectory(dir),
        (None, true) => Output::InPlace,
        (Some(_), true) => {
            return Err(anyhow::anyhow!("Cannot use both --output-directory and --inplace").into())
        }
        (None, false) => {
            return Err(anyhow::anyhow!("Must use either --output-directory or --inplace").into())
        }
    };

    let inputs = if args.filter_existing {
        let args_and_filters = args
            .inputs
            .into_iter()
            .map(
                |input| match Path::new(&input).extension().and_then(OsStr::to_str) {
                    Some("mp3") => {
                        let tag = match id3::Tag::read_from_path(&input) {
                            Ok(tag) => tag,
                            Err(id3::Error {
                                kind: id3::ErrorKind::NoTag,
                                ..
                            }) => return Ok((input, true)),
                            Err(e) => return Err(e.into()),
                        };

                        let bpm = tag
                            .frames()
                            .find(|frame| frame.id() == "TBPM")
                            .and_then(|frame| frame.content().text());

                        Ok((input, bpm.is_none()))
                    }
                    Some("flac") => {
                        let tag = match metaflac::Tag::read_from_path(&input) {
                            Ok(tag) => tag,
                            Err(e) => return Err(e.into()),
                        };

                        let bpms = tag.vorbis_comments().and_then(|comment| comment.get("BPM"));

                        Ok((input, bpms.is_none()))
                    }
                    _ => Err(anyhow::anyhow!("Unsupported file type for file: {}", input)),
                },
            )
            .collect::<Result<Vec<(String, bool)>, anyhow::Error>>()?;

        args_and_filters
            .into_iter()
            .filter(|(_, f)| *f)
            .map(|(i, _)| i)
            .collect()
    } else {
        args.inputs
    };

    if inputs.len() == 0 {
        return Ok(());
    }

    let mut state = State::Playing {
        input_idx: 0,
        player: play(&inputs[0])?,
        last_press_at: None,
        bpms: Vec::new(),
    };

    loop {
        let mut stdout = io::stdout().into_raw_mode()?;
        match state {
            State::Playing {
                input_idx,
                mut player,
                last_press_at,
                mut bpms,
            } => {
                let bpm_part = if bpms.len() > 0 {
                    format!("{}BPM: {}", termion::cursor::Goto(1, 7), avg_bpm(&bpms))
                } else {
                    String::new()
                };
                write!(
                    stdout,
                    "{}{}Playing: {}{}Space to tap for BPM{}Enter to confirm{}s to skip this song{}r to restart the song{}Esc to quit{}",
                    termion::clear::All,
                    termion::cursor::Goto(1, 1),
                    inputs[input_idx],
                    termion::cursor::Goto(1, 2),
                    termion::cursor::Goto(1, 3),
                    termion::cursor::Goto(1, 4),
                    termion::cursor::Goto(1, 5),
                    termion::cursor::Goto(1, 6),
                    bpm_part,
                )?;
                stdout.flush()?;

                let command = on_keypress([
                    (Key::Char(' '), PlayCommands::Tap),
                    (Key::Esc, PlayCommands::Quit),
                    (Key::Char('s'), PlayCommands::Skip),
                    (Key::Char('r'), PlayCommands::Restart),
                    (Key::Char('\n'), PlayCommands::Confirm),
                ])?;

                match command {
                    PlayCommands::Skip => {
                        let input_idx = (input_idx + 1) % inputs.len();
                        player.kill()?;
                        state = State::Playing {
                            input_idx,
                            player: play(&inputs[input_idx])?,
                            last_press_at: None,
                            bpms: Vec::new(),
                        };
                    }
                    PlayCommands::Quit => {
                        player.kill()?;
                        break;
                    }
                    PlayCommands::Confirm => {
                        player.kill()?;
                        state = State::Finished {
                            input_idx,
                            bpm: avg_bpm(&bpms),
                        };
                    }
                    PlayCommands::Restart => {
                        player.kill()?;
                        state = State::Playing {
                            input_idx,
                            player: play(&inputs[input_idx])?,
                            last_press_at: None,
                            bpms: Vec::new(),
                        };
                    }
                    PlayCommands::Tap => {
                        let now = chrono::Utc::now();
                        if let Some(last_press_at) = last_press_at {
                            let diff: chrono::TimeDelta = now - last_press_at;
                            let bpm = 60000.0 / (diff.num_milliseconds() as f64);
                            bpms.push(bpm);
                            if bpms.len() > 10 {
                                bpms.remove(0);
                            }
                        }
                        state = State::Playing {
                            input_idx,
                            player,
                            last_press_at: Some(now),
                            bpms,
                        }
                    }
                }
            }
            State::Finished { input_idx, bpm } => {
                write!(
                    stdout,
                    "{}{}Playing: {}{}Write BPM: {}? (y/n)",
                    termion::clear::All,
                    termion::cursor::Goto(1, 1),
                    inputs[input_idx],
                    termion::cursor::Goto(1, 2),
                    bpm,
                )?;
                stdout.flush()?;

                let command = on_keypress([
                    (Key::Char('y'), ConfirmCommands::Yes),
                    (Key::Char('n'), ConfirmCommands::No),
                ])?;

                match command {
                    ConfirmCommands::Yes => {
                        match output {
                            Output::ToDirectory(ref dir) => {
                                let new_path = Path::new(&dir)
                                    .join(Path::new(&inputs[input_idx]).file_name().unwrap());
                                fs::copy(&inputs[input_idx], &new_path)?;
                                save_tag(new_path.to_str().unwrap(), bpm)?;
                            }
                            Output::InPlace => {
                                save_tag(&inputs[input_idx], bpm)?;
                            }
                        }
                        let input_idx = (input_idx + 1) % inputs.len();
                        state = State::Playing {
                            input_idx,
                            player: play(&inputs[input_idx])?,
                            last_press_at: None,
                            bpms: Vec::new(),
                        };
                    }
                    ConfirmCommands::No => {
                        state = State::Playing {
                            input_idx,
                            player: play(&inputs[input_idx])?,
                            last_press_at: None,
                            bpms: Vec::new(),
                        };
                    }
                }
            }
        }
    }

    Ok(())
}

fn save_tag(filename: &str, bpm: u32) -> Result<(), Box<dyn std::error::Error>> {
    match Path::new(&filename).extension().and_then(OsStr::to_str) {
        Some("mp3") => {
            let mut tag = id3::Tag::read_from_path(&filename)?;
            tag.set_text("TBPM", bpm.to_string());
            tag.write_to_path(&filename, id3::Version::Id3v24)?;
        }
        Some("flac") => {
            let mut tag = metaflac::Tag::read_from_path(&filename)?;
            tag.set_vorbis("BPM", vec![bpm.to_string()]);
            tag.save()?;
        }
        _ => return Err(anyhow::anyhow!("Unsupported file type").into()),
    }
    Ok(())
}
