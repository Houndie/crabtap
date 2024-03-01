use clap::Parser;
use crossterm::{
    event::{Event, KeyCode, KeyEvent, KeyModifiers},
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
    CompletedFrame, Frame, Terminal,
};
use std::{
    collections::{hash_map, HashMap},
    ffi::OsStr,
    io,
    path::Path,
    process::{Child, Command, Stdio},
};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    inputs: Vec<String>,
}

enum State {
    Playing {
        input_idx: usize,
        player: std::process::Child,
        last_press_at: Option<chrono::DateTime<chrono::Utc>>,
        bpms: Bpms,
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

struct RAIITerminal {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
}

impl RAIITerminal {
    fn new() -> Result<RAIITerminal, anyhow::Error> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        Ok(RAIITerminal {
            terminal: Terminal::new(backend)?,
        })
    }

    fn draw<F>(&mut self, f: F) -> io::Result<CompletedFrame>
    where
        F: FnOnce(&mut Frame),
    {
        self.terminal.draw(f)
    }
}

impl Drop for RAIITerminal {
    fn drop(&mut self) {
        disable_raw_mode().unwrap();
        execute!(
            self.terminal.backend_mut(),
            crossterm::terminal::LeaveAlternateScreen
        )
        .unwrap();
        self.terminal.show_cursor().unwrap();
    }
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
    iter: impl IntoIterator<Item = (KeyEvent, Command)>,
) -> Result<Command, anyhow::Error> {
    let mut commands = iter
        .into_iter()
        .map(|(k, v)| (Event::Key(k), v))
        .collect::<HashMap<Event, Command>>();
    loop {
        let key = crossterm::event::read()?;

        if let hash_map::Entry::Occupied(entry) = commands.entry(key) {
            return Ok(entry.remove());
        }
    }
}

struct Bpms {
    bpms: [f64; 10],
    next: usize,
    size: usize,
}

impl Bpms {
    fn new() -> Bpms {
        Bpms {
            bpms: [0.0; 10],
            next: 0,
            size: 0,
        }
    }

    fn push(&mut self, bpm: f64) {
        self.bpms[self.next] = bpm;
        self.next = (self.next + 1) % 10;
        if self.size < 10 {
            self.size += 1;
        }
    }

    fn avg(&self) -> Option<u32> {
        if self.size == 0 {
            None
        } else {
            Some((self.bpms.iter().take(self.size).sum::<f64>() / self.size as f64) as u32)
        }
    }
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

fn draw_ui(f: &mut Frame, inputs: &[File], input_idx: usize, bpm: Option<u32>) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([Constraint::Percentage(90), Constraint::Percentage(10)].as_ref())
        .split(f.size());

    let input_table = inputs
        .into_iter()
        .enumerate()
        .map(|(idx, input)| {
            let style = if idx == input_idx {
                Style::default().fg(Color::Black).bg(Color::White)
            } else {
                Style::default()
            };

            let bpm_str = match input.bpm {
                Some(bpm) => format!("{}", bpm),
                None => "None".to_owned(),
            };

            Row::new(vec![input.path.clone(), bpm_str]).style(style)
        })
        .collect::<Table>()
        .widths(&[Constraint::Percentage(90), Constraint::Percentage(10)])
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

enum FileType {
    Mp3,
    Flac,
}

struct File {
    path: String,
    bpm: Option<u32>,
    typ: FileType,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let mut inputs = args
        .inputs
        .into_iter()
        .map(|input| {
            let typ = match Path::new(&input).extension().and_then(OsStr::to_str) {
                Some("mp3") => FileType::Mp3,
                Some("flac") => FileType::Flac,
                _ => return Err(anyhow::anyhow!("Unsupported file type")),
            };

            let bpm = match typ {
                FileType::Mp3 => {
                    let tag = id3::Tag::read_from_path(&input)?;
                    tag.get("TBPM")
                        .and_then(|bpm| bpm.content().text())
                        .and_then(|bpm| bpm.parse().ok())
                }
                FileType::Flac => {
                    let tag = metaflac::Tag::read_from_path(&input)?;
                    tag.get_vorbis("BPM")
                        .and_then(|mut bpm| bpm.next())
                        .and_then(|bpm| bpm.parse().ok())
                }
            };

            Ok(File {
                path: input,
                bpm,
                typ,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    if inputs.len() == 0 {
        return Ok(());
    }

    let mut state = State::Playing {
        input_idx: 0,
        player: play(&inputs[0].path)?,
        last_press_at: None,
        bpms: Bpms::new(),
    };

    let mut terminal = RAIITerminal::new()?;

    loop {
        match state {
            State::Playing {
                input_idx,
                mut player,
                last_press_at,
                mut bpms,
            } => {
                terminal.draw(|f| {
                    draw_ui(f, &inputs, input_idx, bpms.avg());
                })?;

                let command = on_keypress([
                    (
                        KeyEvent::new(KeyCode::Char(' '), KeyModifiers::empty()),
                        PlayCommands::Tap,
                    ),
                    (
                        KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()),
                        PlayCommands::Quit,
                    ),
                    (
                        KeyEvent::new(KeyCode::Char('s'), KeyModifiers::empty()),
                        PlayCommands::Skip,
                    ),
                    (
                        KeyEvent::new(KeyCode::Char('r'), KeyModifiers::empty()),
                        PlayCommands::Restart,
                    ),
                    (
                        KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()),
                        PlayCommands::Confirm,
                    ),
                ])?;

                match command {
                    PlayCommands::Skip => {
                        let input_idx = (input_idx + 1) % inputs.len();
                        player.kill()?;
                        state = State::Playing {
                            input_idx,
                            player: play(&inputs[input_idx].path)?,
                            last_press_at: None,
                            bpms: Bpms::new(),
                        };
                    }
                    PlayCommands::Quit => {
                        player.kill()?;
                        break;
                    }
                    PlayCommands::Confirm => match bpms.avg() {
                        Some(bpm) => {
                            player.kill()?;
                            state = State::Finished { input_idx, bpm };
                        }
                        None => {
                            state = State::Playing {
                                input_idx,
                                player,
                                last_press_at,
                                bpms,
                            }
                        }
                    },
                    PlayCommands::Restart => {
                        player.kill()?;
                        state = State::Playing {
                            input_idx,
                            player: play(&inputs[input_idx].path)?,
                            last_press_at: None,
                            bpms: Bpms::new(),
                        };
                    }
                    PlayCommands::Tap => {
                        let now = chrono::Utc::now();
                        if let Some(last_press_at) = last_press_at {
                            let diff: chrono::TimeDelta = now - last_press_at;
                            let bpm = 60000.0 / (diff.num_milliseconds() as f64);
                            bpms.push(bpm);
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
                    (
                        KeyEvent::new(KeyCode::Char('y'), KeyModifiers::empty()),
                        ConfirmCommands::Yes,
                    ),
                    (
                        KeyEvent::new(KeyCode::Char('n'), KeyModifiers::empty()),
                        ConfirmCommands::No,
                    ),
                ])?;

                match command {
                    ConfirmCommands::Yes => {
                        inputs[input_idx].bpm = Some(bpm);
                        save_tag(&inputs[input_idx], bpm)?;
                        let input_idx = (input_idx + 1) % inputs.len();
                        state = State::Playing {
                            input_idx,
                            player: play(&inputs[input_idx].path)?,
                            last_press_at: None,
                            bpms: Bpms::new(),
                        };
                    }
                    ConfirmCommands::No => {
                        state = State::Playing {
                            input_idx,
                            player: play(&inputs[input_idx].path)?,
                            last_press_at: None,
                            bpms: Bpms::new(),
                        };
                    }
                }
            }
        }
    }

    Ok(())
}

fn save_tag(file: &File, bpm: u32) -> Result<(), anyhow::Error> {
    match file.typ {
        FileType::Mp3 => {
            let mut tag =
                id3::Tag::read_from_path(&file.path).map_err(Into::<anyhow::Error>::into)?;
            tag.set_text("TBPM", bpm.to_string());
            tag.write_to_path(&file.path, id3::Version::Id3v24)
                .map_err(Into::<anyhow::Error>::into)?;
        }
        FileType::Flac => {
            let mut tag =
                metaflac::Tag::read_from_path(&file.path).map_err(Into::<anyhow::Error>::into)?;
            tag.set_vorbis("BPM", vec![bpm.to_string()]);
            tag.save().map_err(Into::<anyhow::Error>::into)?;
        }
    }
    Ok(())
}
