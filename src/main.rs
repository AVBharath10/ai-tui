use anyhow::Result;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, List, ListItem},
    Terminal,
};
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    collections::VecDeque,
    io::{Read, Write},
    path::PathBuf,
    sync::mpsc,
    thread,
    time::Duration,
};

// Unified event type for our application
enum AppEvent {
    PtyData(Vec<u8>),
    FileEvent(notify::Event),
}

#[derive(Clone)]
struct FileChange {
    path: String,
    kind: ChangeKind,
    timestamp: std::time::Instant,
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum ChangeKind {
    Create,
    Modify,
    Remove,
}

struct AppState {
    file_changes: VecDeque<FileChange>,
    // key: (path, kind), value: instant when last recorded
    debounce_map: std::collections::HashMap<(String, ChangeKind), std::time::Instant>,
}

impl AppState {
    fn new() -> Self {
        Self {
            file_changes: VecDeque::with_capacity(50),
            debounce_map: std::collections::HashMap::new(),
        }
    }

    fn add_change(&mut self, path: PathBuf, kind: ChangeKind) {
        let file_name = path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        // 1. Filter Noise (System folders)
        if path.components().any(|c| c.as_os_str() == ".git" || c.as_os_str() == "target") {
            return;
        }
        if file_name.starts_with('.') && file_name != ".gitignore" {
             return;
        }

        // 2. Debounce (Collapse duplicates)
        // If we saw the exact same (path, kind) recently, ignore it.
        let key = (file_name.clone(), kind.clone());
        if let Some(last_time) = self.debounce_map.get(&key) {
            if last_time.elapsed() < Duration::from_millis(500) {
                return;
            }
        }
        self.debounce_map.insert(key, std::time::Instant::now());

        // 3. Add to UI List
        if self.file_changes.len() >= 20 {
            self.file_changes.pop_back();
        }

        self.file_changes.push_front(FileChange {
            path: file_name,
            kind,
            timestamp: std::time::Instant::now(),
        });
    }
}

fn main() -> Result<()> {
    // 1. Setup PTY
    let pty_system = native_pty_system();
    let mut pair = pty_system.openpty(PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    })?;
    let cwd = std::env::current_dir()?;
    let mut cmd = CommandBuilder::new("npx");
    cmd.args(&["opencode-ai"]);
    cmd.cwd(&cwd);
    let mut child = pair.slave.spawn_command(cmd)?;

    // 2. Setup Channel for Events
    let (tx, rx) = mpsc::channel::<AppEvent>();

    // 3. PTY Reader Thread
    let mut reader = pair.master.try_clone_reader()?;
    let tx_pty = tx.clone();
    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(n) if n > 0 => {
                    if tx_pty
                        .send(AppEvent::PtyData(buf[..n].to_vec()))
                        .is_err()
                    {
                        break;
                    }
                }
                _ => break,
            }
        }
    });

    // 4. File Watcher
    let tx_watcher = tx.clone();
    let mut watcher = RecommendedWatcher::new(
        move |res| {
            if let Ok(event) = res {
                let _ = tx_watcher.send(AppEvent::FileEvent(event));
            }
        },
        Config::default(),
    )?;
    // Watch current directory recursively
    watcher.watch(".".as_ref(), RecursiveMode::Recursive)?;

    // 5. Setup TUI
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // 6. Setup VT100 Parser & App State
    let mut parser = vt100::Parser::new(24, 80, 0);
    let mut app_state = AppState::new();

    // Write handle for forwarding input to PTY
    let mut writer = pair.master.take_writer()?;

    // 7. Main Loop
    let loop_result = run_app(
        &mut terminal,
        &mut parser,
        &mut app_state,
        &rx,
        &mut writer,
        &mut *pair.master,
    );

    // 8. Cleanup
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    let _ = child.kill();

    loop_result
}

fn run_app(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    parser: &mut vt100::Parser,
    state: &mut AppState,
    rx: &mpsc::Receiver<AppEvent>,
    writer: &mut dyn Write,
    master: &mut dyn portable_pty::MasterPty,
) -> Result<()> {
    loop {
        // A. Process all available events (non-blocking)
        while let Ok(event) = rx.try_recv() {
            match event {
                AppEvent::PtyData(data) => {
                    parser.process(&data);
                }
                AppEvent::FileEvent(event) => {
                    use notify::event::{EventKind, ModifyKind};
                    match event.kind {
                        EventKind::Create(_) => {
                            for path in event.paths {
                                state.add_change(path, ChangeKind::Create);
                            }
                        }
                        EventKind::Modify(ModifyKind::Data(_)) => {
                            // Only capture Content changes, ignore Metadata/Access
                            for path in event.paths {
                                state.add_change(path, ChangeKind::Modify);
                            }
                        }
                        EventKind::Modify(ModifyKind::Name(_)) => {
                            // Rename often comes with Create/Remove, 
                            // but sometimes is standalone. Treat as Modify for now or Create?
                            // 'Name' often means move. 
                            // Let's just track it as Modify to ensure visibility.
                            for path in event.paths {
                                state.add_change(path, ChangeKind::Modify);
                            }
                        }
                        EventKind::Remove(_) => {
                            for path in event.paths {
                                state.add_change(path, ChangeKind::Remove);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        // B. Render
        terminal.draw(|frame| {
            let area = frame.area();
            
            // Split screen
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(70), // Terminal
                    Constraint::Percentage(30), // Sidebar
                ])
                .split(area);
                
            let term_area = chunks[0];
            let side_area = chunks[1];

            // --- Render Terminal ---
            // Render the VT100 screen to the buffer
            let screen = parser.screen();
            let (rows, cols) = screen.size();
            let buffer = frame.buffer_mut();

            // We need to handle the offset of the term_area
            // term_area has x, y. 
            // We map loop(0..rows) -> term_area.y + row
            
            for row in 0..rows.min(term_area.height) {
                for col in 0..cols.min(term_area.width) {
                    if let Some(cell) = screen.cell(row, col) {
                        let fg = convert_color(cell.fgcolor());
                        let bg = convert_color(cell.bgcolor());
                        
                        let mut style = Style::default().fg(fg).bg(bg);
                        if cell.bold() { style = style.add_modifier(Modifier::BOLD); }
                        if cell.italic() { style = style.add_modifier(Modifier::ITALIC); }
                        if cell.underline() { style = style.add_modifier(Modifier::UNDERLINED); }
                        if cell.inverse() { style = style.add_modifier(Modifier::REVERSED); }

                        let contents = cell.contents();
                        let grid_x = term_area.x + col;
                        let grid_y = term_area.y + row;
                        
                        if !contents.is_empty() {
                            buffer.set_string(grid_x, grid_y, contents, style);
                        } else {
                            // Clear background
                             buffer.set_string(grid_x, grid_y, " ", style);
                        }
                    }
                }
            }

            // Render Cursor (adjusted for term_area)
            if !screen.hide_cursor() {
                let (crow, ccol) = screen.cursor_position();
                if ccol < term_area.width && crow < term_area.height {
                     frame.set_cursor_position(Position { 
                         x: term_area.x + ccol, 
                         y: term_area.y + crow 
                     });
                }
            }
            
            // --- Render Sidebar ---
            let block = Block::default()
                .title(" Active Monitoring ")
                .borders(Borders::ALL)
                .style(Style::default().fg(Color::DarkGray));
            
            let items: Vec<ListItem> = state.file_changes
                .iter()
                .map(|change| {
                    let (symbol, color) = match change.kind {
                        ChangeKind::Create => ("+", Color::Green),
                        ChangeKind::Modify => ("~", Color::Yellow),
                        ChangeKind::Remove => ("-", Color::Red),
                    };
                    let content = format!("{} {}", symbol, change.path);
                    ListItem::new(content).style(Style::default().fg(color))
                })
                .collect();
                
            let list = List::new(items).block(block);
            frame.render_widget(list, side_area);
            
        })?;

        // C. Poll Input
        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                 Event::Resize(cols, rows) => {
                     // We need to handle resize carefully with split panes.
                     // The PTY size should match the *Terminal Pane* size, not the full window.
                     // But we only know the *term_area* during render. 
                     // Simple approximation: calc what 70% is.
                     
                     let term_cols = (cols as f32 * 0.7) as u16;
                     let term_rows = rows; // Full height
                     
                     master.resize(PtySize {
                        rows: term_rows,
                        cols: term_cols,
                        pixel_width: 0,
                        pixel_height: 0,
                    })?;
                    *parser = vt100::Parser::new(term_rows, term_cols, 0);
                }
                Event::Key(key) => {
                    match key.code {
                        KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(()),
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => writer.write_all(&[3])?,
                        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => writer.write_all(&[4])?,
                        
                        KeyCode::Char(c) => writer.write_all(c.to_string().as_bytes())?,
                        KeyCode::Enter => writer.write_all(b"\r")?,
                        KeyCode::Backspace => writer.write_all(&[127])?, // DEL
                        KeyCode::Tab => writer.write_all(&[9])?,
                        KeyCode::Esc => writer.write_all(&[27])?,
                        
                        KeyCode::Up => writer.write_all(b"\x1b[A")?,
                        KeyCode::Down => writer.write_all(b"\x1b[B")?,
                        KeyCode::Right => writer.write_all(b"\x1b[C")?,
                        KeyCode::Left => writer.write_all(b"\x1b[D")?,
                        _ => {}
                    }
                    writer.flush()?;
                }
                _ => {}
            }
        }
    }
}

fn convert_color(c: vt100::Color) -> Color {
    match c {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}