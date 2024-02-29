use clap::Parser;
use id3::TagLike;
use std::fs;
use std::io::{self, Write};
use std::process::Stdio;
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

enum FinishState {
    Skip,
    Quit,
    Confirm,
    Restart,
}

enum Output {
    ToDirectory(String),
    InPlace,
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

    for input in inputs {
        let mut do_another = true;
        loop {
            let mut last_press_at = None;
            let mut times = Vec::new();
            let mut child = Command::new("mpv")
                .arg(input.clone())
                .arg("--no-video")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?;
            let mut stdout = io::stdout().into_raw_mode()?;
            let instructions = format!(
                "{}Playing: {}{}Space to tap for BPM{}Enter to confirm{}s to skip this song{}r to restart the song{}Esc to quit{}",
                termion::cursor::Goto(1, 1),
                input,
                termion::cursor::Goto(1, 2),
                termion::cursor::Goto(1, 3),
                termion::cursor::Goto(1, 4),
                termion::cursor::Goto(1, 5),
                termion::cursor::Goto(1, 6),
                termion::cursor::Goto(1, 7)
            );
            write!(stdout, "{}{}", termion::clear::All, instructions)?;
            stdout.flush()?;
            let mut state = FinishState::Quit;
            for key in io::stdin().keys() {
                match key? {
                    Key::Char(' ') => {
                        let now = chrono::Utc::now();
                        if let Some(last_press_at) = last_press_at {
                            let diff: chrono::TimeDelta = now - last_press_at;
                            let bpm = 60000.0 / (diff.num_milliseconds() as f64);
                            times.push(bpm);
                            if times.len() > 10 {
                                times.remove(0);
                            }
                            let avg_bpm = (times.iter().sum::<f64>() / times.len() as f64) as u32;
                            write!(
                                stdout,
                                "{}{}BPM: {}",
                                termion::clear::CurrentLine,
                                termion::cursor::Goto(1, 7),
                                avg_bpm
                            )?;
                            stdout.flush()?;
                        }
                        last_press_at = Some(now);
                    }
                    Key::Esc => {
                        state = FinishState::Quit;
                        break;
                    }
                    Key::Char('s') => {
                        state = FinishState::Skip;
                        break;
                    }
                    Key::Char('r') => {
                        state = FinishState::Restart;
                        break;
                    }
                    Key::Char('\n') => {
                        state = FinishState::Confirm;
                        break;
                    }
                    _ => {}
                }
            }
            child.kill()?;
            match state {
                FinishState::Skip => {
                    break;
                }
                FinishState::Quit => {
                    do_another = false;
                    break;
                }
                FinishState::Restart => (),
                FinishState::Confirm => {
                    let avg_bpm = (times.iter().sum::<f64>() / times.len() as f64) as u32;
                    write!(
                        stdout,
                        "{}Write BPM: {}? (y/n)",
                        termion::cursor::Goto(1, 8),
                        avg_bpm,
                    )?;
                    stdout.flush()?;

                    let mut write_file = false;
                    for key in io::stdin().keys() {
                        match key? {
                            Key::Char('y') => {
                                write_file = true;
                                break;
                            }
                            Key::Char('n') => {
                                write_file = false;
                                break;
                            }
                            _ => {}
                        }
                    }
                    if write_file {
                        match output {
                            Output::ToDirectory(ref dir) => {
                                let new_path =
                                    Path::new(&dir).join(Path::new(&input).file_name().unwrap());
                                fs::copy(&input, &new_path)?;
                                save_tag(new_path.to_str().unwrap(), avg_bpm)?;
                            }
                            Output::InPlace => {
                                save_tag(&input, avg_bpm)?;
                            }
                        }
                        break;
                    };
                }
            }
        }
        if !do_another {
            break;
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
