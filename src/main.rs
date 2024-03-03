use clap::Parser;
use crossterm::{
    event::{Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Row, Table, TableState},
    CompletedFrame, Frame, Terminal,
};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};
use std::{
    ffi::OsStr,
    fs::File,
    io::{self, BufReader},
    path::Path,
};

mod file;

/// A tui for generating tap BPMs in rust
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Any flac or mp3 file
    inputs: Vec<String>,

    /// Confirm the BPM before saving
    #[clap(short, long)]
    confirm: bool,
}

enum State {
    Playing,
    Finished { bpm: u32 },
    Manual { manual_bpm: u32 },
}

enum PlayCommands {
    Quit,
    Confirm,
    Restart,
    Tap,
    Up,
    Down,
    Manual,
}

fn play_keys(key: KeyEvent) -> Option<PlayCommands> {
    if key.modifiers != KeyModifiers::empty() {
        return None;
    }

    match key.code {
        KeyCode::Char(' ') => Some(PlayCommands::Tap),
        KeyCode::Esc | KeyCode::Char('q') => Some(PlayCommands::Quit),
        KeyCode::Char('r') => Some(PlayCommands::Restart),
        KeyCode::Enter => Some(PlayCommands::Confirm),
        KeyCode::Up | KeyCode::Char('k') => Some(PlayCommands::Up),
        KeyCode::Down | KeyCode::Char('j') => Some(PlayCommands::Down),
        KeyCode::Char('m') => Some(PlayCommands::Manual),
        _ => None,
    }
}

fn confirm_keys(key: KeyEvent) -> Option<ConfirmCommands> {
    if key.modifiers != KeyModifiers::empty() {
        return None;
    }

    match key.code {
        KeyCode::Char('y') => Some(ConfirmCommands::Yes),
        KeyCode::Char('n') => Some(ConfirmCommands::No),
        _ => None,
    }
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

fn on_keypress<Command, F: Fn(KeyEvent) -> Option<Command>>(
    keys: F,
) -> Result<Command, anyhow::Error> {
    loop {
        let key = crossterm::event::read()?;
        if let Event::Key(key) = key {
            if let Some(command) = keys(key) {
                return Ok(command);
            }
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

struct AudioStream<'a> {
    handle: &'a OutputStreamHandle,
}

impl<'a> AudioStream<'a> {
    fn new(handle: &'a OutputStreamHandle) -> AudioStream<'a> {
        AudioStream { handle }
    }

    fn play(&'a self, input: &str) -> Result<Sink, anyhow::Error> {
        let sink = Sink::try_new(self.handle)?;
        let source = Decoder::new_looped(BufReader::new(File::open(input)?))?;
        sink.append(source);
        sink.play();
        Ok(sink)
    }
}

fn draw_ui(
    f: &mut Frame,
    inputs: &[Box<dyn file::Music>],
    table_state: &mut TableState,
    bpm: Option<u32>,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([Constraint::Percentage(90), Constraint::Percentage(10)].as_ref())
        .split(f.size());

    let input_table = inputs
        .into_iter()
        .map(|input| {
            let bpm_str = match input.bpm() {
                Some(bpm) => format!("{}", bpm),
                None => "None".to_owned(),
            };

            Row::new(vec![input.path().to_owned(), bpm_str])
        })
        .collect::<Table>()
        .widths(&[Constraint::Percentage(90), Constraint::Percentage(10)])
        .block(Block::default().borders(Borders::ALL))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    f.render_stateful_widget(input_table, chunks[0], table_state);

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
    let (_stream, stream_handle) = OutputStream::try_default()?;
    let audio_stream = AudioStream::new(&stream_handle);

    let args = Args::parse();
    let mut inputs = args
        .inputs
        .into_iter()
        .map(|input| -> Result<Box<dyn file::Music>, anyhow::Error> {
            let f = match Path::new(&input).extension().and_then(OsStr::to_str) {
                Some("mp3") => Box::new(file::Mp3::new(input)?) as Box<dyn file::Music>,
                Some("flac") => Box::new(file::Flac::new(input)?) as Box<dyn file::Music>,
                _ => return Err(anyhow::anyhow!("{}: Unsupported file type", input)),
            };

            Ok(f)
        })
        .collect::<Result<Vec<_>, _>>()?;

    if inputs.len() == 0 {
        return Ok(());
    }
    let mut table_state = TableState::default();
    table_state.select(Some(0));
    let mut _player = audio_stream.play(&inputs[0].path())?;
    let mut last_press_at = None;
    let mut bpms = Bpms::new();

    let mut state = State::Playing;

    let mut terminal = RAIITerminal::new()?;

    loop {
        match state {
            State::Playing => {
                terminal.draw(|f| {
                    draw_ui(f, &inputs, &mut table_state, bpms.avg());
                })?;

                let command = on_keypress(play_keys)?;

                match command {
                    PlayCommands::Quit => {
                        break;
                    }
                    PlayCommands::Confirm => match bpms.avg() {
                        Some(bpm) => {
                            if args.confirm {
                                state = State::Finished { bpm };
                            } else {
                                inputs[table_state.selected().unwrap()].set_bpm(bpm)?;
                                let input_idx =
                                    (table_state.selected().unwrap() + 1) % inputs.len();
                                table_state.select(Some(input_idx));
                                _player = audio_stream.play(&inputs[input_idx].path())?;
                                last_press_at = None;
                                bpms = Bpms::new();
                            }
                        }
                        None => {}
                    },
                    PlayCommands::Restart => {
                        _player =
                            audio_stream.play(&inputs[table_state.selected().unwrap()].path())?;
                        last_press_at = None;
                        bpms = Bpms::new();
                    }
                    PlayCommands::Tap => {
                        let now = chrono::Utc::now();
                        if let Some(last_press_at) = last_press_at {
                            let diff: chrono::TimeDelta = now - last_press_at;
                            let bpm = 60000.0 / (diff.num_milliseconds() as f64);
                            bpms.push(bpm);
                        }
                        last_press_at = Some(now);
                    }
                    PlayCommands::Up => {
                        if inputs.len() == 1 {
                            continue;
                        }

                        let input_idx =
                            (table_state.selected().unwrap() + inputs.len() - 1) % inputs.len();
                        table_state.select(Some(input_idx));
                        _player = audio_stream.play(&inputs[input_idx].path())?;
                        last_press_at = None;
                        bpms = Bpms::new();
                    }
                    PlayCommands::Down => {
                        if inputs.len() == 1 {
                            continue;
                        }

                        let input_idx = (table_state.selected().unwrap() + 1) % inputs.len();
                        table_state.select(Some(input_idx));
                        _player = audio_stream.play(&inputs[input_idx].path())?;
                        last_press_at = None;
                        bpms = Bpms::new();
                    }

                    PlayCommands::Manual => {
                        state = State::Manual { manual_bpm: 0 };
                    }
                }
            }
            State::Finished { bpm } => {
                terminal.draw(|f| {
                    draw_ui(f, &inputs, &mut table_state, Some(bpm));
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

                let command = on_keypress(confirm_keys)?;

                match command {
                    ConfirmCommands::Yes => {
                        inputs[table_state.selected().unwrap()].set_bpm(bpm)?;
                        let input_idx = (table_state.selected().unwrap() + 1) % inputs.len();
                        state = State::Playing;
                        table_state.select(Some(input_idx));
                        _player = audio_stream.play(&inputs[input_idx].path())?;
                        last_press_at = None;
                        bpms = Bpms::new();
                    }
                    ConfirmCommands::No => {
                        state = State::Playing;
                    }
                }
            }
            State::Manual { manual_bpm } => {
                terminal.draw(|f| {
                    draw_ui(f, &inputs, &mut table_state, bpms.avg());
                    let manual_bpm_str = if manual_bpm > 0 {
                        manual_bpm.to_string()
                    } else {
                        String::new()
                    };
                    let popup = Paragraph::new(manual_bpm_str).block(
                        Block::default()
                            .title("Manually enter bpm")
                            .borders(Borders::ALL),
                    );
                    let area = centered_rect(30, 5, f.size());
                    f.render_widget(Clear, area);
                    f.render_widget(popup, area);
                })?;

                loop {
                    let key = crossterm::event::read()?;

                    let key_event = match key {
                        Event::Key(key) => key,
                        _ => continue,
                    };

                    if key_event.modifiers != KeyModifiers::empty() {
                        continue;
                    }

                    let new_bpm = match key_event.code {
                        KeyCode::Esc | KeyCode::Char('m') => {
                            state = State::Playing;
                            break;
                        }
                        KeyCode::Enter => {
                            inputs[table_state.selected().unwrap()].set_bpm(manual_bpm)?;
                            let input_idx = (table_state.selected().unwrap() + 1) % inputs.len();
                            state = State::Playing;
                            table_state.select(Some(input_idx));
                            _player = audio_stream.play(&inputs[input_idx].path())?;
                            last_press_at = None;
                            bpms = Bpms::new();
                            break;
                        }
                        KeyCode::Backspace => manual_bpm / 10,
                        KeyCode::Char(c) => {
                            if let Some(digit) = c.to_digit(10) {
                                if manual_bpm > u32::MAX / 10 {
                                    manual_bpm
                                } else {
                                    manual_bpm * 10 + digit
                                }
                            } else {
                                manual_bpm
                            }
                        }
                        _ => manual_bpm,
                    };

                    state = State::Manual {
                        manual_bpm: new_bpm,
                    };

                    break;
                }
            }
        }
    }

    Ok(())
}
