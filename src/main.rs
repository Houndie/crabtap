use clap::Parser;
use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen},
};
use id3::TagLike;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Row, Table},
    Frame, Terminal,
};
use std::{
    collections::{hash_map, HashMap},
    ffi::OsStr,
    fs, io,
    path::Path,
    process::{Child, Command, Stdio},
};
use termion::{event::Key, input::TermRead};

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

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .split(r);

    Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .split(popup_layout[1])[1]
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

fn draw_ui<S: AsRef<str>>(f: &mut Frame, inputs: &[S], input_idx: usize, bpm: Option<u32>) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([Constraint::Percentage(90), Constraint::Percentage(10)].as_ref())
        .split(f.size());

    let input_table = inputs
        .iter()
        .enumerate()
        .map(|(idx, input)| {
            let style = if idx == input_idx {
                Style::default().fg(Color::Black).bg(Color::White)
            } else {
                Style::default()
            };

            Row::new(vec![input.as_ref().to_owned()]).style(style)
        })
        .collect::<Table>()
        .block(Block::default().borders(Borders::ALL));

    f.render_widget(input_table, chunks[0]);

    let bpm_part = Paragraph::new(vec![Line::from(match bpm {
        Some(bpm) => format!("BPM: {}", bpm),
        None => String::new(),
    })])
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title("Tap Space for BPM!")
            .title_alignment(Alignment::Center),
    );

    f.render_widget(bpm_part, chunks[1]);
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

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    loop {
        match state {
            State::Playing {
                input_idx,
                mut player,
                last_press_at,
                mut bpms,
            } => {
                terminal.draw(|f| {
                    let bpm = if bpms.len() > 0 {
                        Some(avg_bpm(&bpms))
                    } else {
                        None
                    };
                    draw_ui(f, &inputs, input_idx, bpm);
                })?;

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
                terminal.draw(|f| {
                    draw_ui(f, &inputs, input_idx, Some(bpm));
                    let popup = Paragraph::new(vec![
                        Line::from("Save BPM?"),
                        Line::from(vec![
                            Span::styled("y", Style::default().add_modifier(Modifier::BOLD)),
                            Span::raw("es/"),
                            Span::styled("n", Style::default().add_modifier(Modifier::BOLD)),
                            Span::raw("o"),
                        ]),
                    ])
                    .block(Block::default().borders(Borders::ALL))
                    .alignment(Alignment::Center);
                    let area = centered_rect(10, 10, f.size());
                    f.render_widget(Clear, area);
                    f.render_widget(popup, area);
                })?;

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

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        crossterm::terminal::LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;

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
