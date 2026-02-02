use anyhow::Result;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Position},
    text::{Line, Span},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Clear},
    Terminal,
};
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use chrono::{DateTime, Local};
use similar::{ChangeTag, TextDiff};
use walkdir::WalkDir;
use std::{
    collections::VecDeque,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

// Unified event type for our application
enum AppEvent {
    PtyData(Vec<u8>),
    FileChange(PathBuf, ChangeKind),
    Tick,
    Input(Event),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum ChangeKind {
    Create,
    Modify,
    Remove,
}

#[derive(Clone)]
struct FileChange {
    path: String,
    kind: ChangeKind,
    timestamp: DateTime<Local>,
    diff: Option<String>, // Stores colored ANSI string or plain text representation
}

#[derive(Clone)]
struct PendingChange {
    path: String,
    old_content: String,
    new_content: String,
    diff_text: String,
}

struct AppState {
    file_changes: VecDeque<FileChange>,
    // key: (path, kind), value: instant when last recorded
    debounce_map: std::collections::HashMap<(String, ChangeKind), Instant>,
    list_state: ListState,
    show_sidebar: bool,
    
    // key: path, value: content
    file_cache: std::collections::HashMap<String, String>,
    
    // Approval System
    approval_queue: VecDeque<PendingChange>,
    ignore_next_write: std::collections::HashSet<String>,
    modal_active: bool,
    
    show_diff_view: bool,
    parser: vt100::Parser,
}

impl AppState {
    fn new() -> Self {
        let mut cache = std::collections::HashMap::new();
        
        // Initial Scan to populate cache
        for entry in WalkDir::new(".").into_iter().filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_file() {
                // Filter noise
                 if path.components().any(|c| c.as_os_str() == ".git" || c.as_os_str() == "target") {
                    continue;
                }
                
                // Store normalized absolute path
                let key = normalize_path(path);
                if let Ok(content) = std::fs::read_to_string(path) {
                     cache.insert(key, content);
                }
            }
        }

        Self {
            file_changes: VecDeque::with_capacity(50),
            debounce_map: std::collections::HashMap::new(),
            list_state: ListState::default(),
            show_sidebar: true,
            file_cache: cache,
            
            approval_queue: VecDeque::new(),
            ignore_next_write: std::collections::HashSet::new(),
            modal_active: false,
            
            show_diff_view: false,
            parser: vt100::Parser::new(24, 80, 0), // Initial size, will be updated
        }
    }

    fn add_change(&mut self, path: PathBuf, kind: ChangeKind) {
        let file_name = path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        let cache_key = normalize_path(&path);

        // 1. Check Ignore List (Revert Loop Prevention)
        if self.ignore_next_write.contains(&cache_key) {
            self.ignore_next_write.remove(&cache_key);
            // We ignore this event because it was our own revert
            return;
        }

        // 2. Filter Noise
        if path.components().any(|c| c.as_os_str() == ".git" || c.as_os_str() == "target" || c.as_os_str() == "node_modules") {
            return;
        }
        if file_name.starts_with('.') && file_name != ".gitignore" {
             return;
        }

        // 3. Debounce
        let key = (file_name.clone(), kind.clone());
        if let Some(last_time) = self.debounce_map.get(&key) {
            if last_time.elapsed() < Duration::from_millis(500) {
                return;
            }
        }
        self.debounce_map.insert(key, Instant::now());

        // 4. Content Logic & Approval
        let old_content = self.file_cache.get(&cache_key).cloned().unwrap_or_default();
        let mut new_content = String::new();
        let mut diff_output = None;
        let mut needs_approval = false;

        if kind == ChangeKind::Modify || kind == ChangeKind::Create {
            if let Ok(content) = std::fs::read_to_string(&path) {
                new_content = content;
                
                // If content hasn't effectively changed from our cache, ignore it
                if new_content == old_content {
                    return; 
                }

                // Generate Diff
                let diff = TextDiff::from_lines(&old_content, &new_content);
                let mut output = String::new();
                for (idx, group) in diff.grouped_ops(3).iter().enumerate() {
                    if idx > 0 { output.push_str("...\n"); }
                    for op in group {
                        for change in diff.iter_changes(op) {
                            let (sign, _) = match change.tag() {
                                ChangeTag::Delete => ("-", Color::Red),
                                ChangeTag::Insert => ("+", Color::Green),
                                ChangeTag::Equal => (" ", Color::Reset),
                            };
                            output.push_str(&format!("{}{}", sign, change));
                        }
                    }
                }
                
                if output.is_empty() && !new_content.is_empty() {
                     output = format!("+{}", new_content.replace('\n', "\n+"));
                } else if output.is_empty() {
                    output = "No Content Changes".to_string();
                }

                diff_output = Some(output.clone());
                
                // QUEUE FOR APPROVAL
                self.approval_queue.push_back(PendingChange {
                    path: cache_key.clone(), // Store full path for revert
                    old_content: old_content,
                    new_content: new_content, // Don't update cache yet
                    diff_text: output,
                });
                self.modal_active = true;
                needs_approval = true;
            }
        } else if kind == ChangeKind::Remove {
             // Handle Deletion Approval
             // logic: new_content is empty
             if !old_content.is_empty() {
                let diff = format!("File Deleted: {}", file_name);
                diff_output = Some(diff.clone());
                
                self.approval_queue.push_back(PendingChange {
                    path: cache_key.clone(),
                    old_content: old_content,
                    new_content: String::new(), // Empty means deleted logic?
                    // Actually, if we reject deletion, we need to write old_content back.
                    // If we accept, we remove from cache.
                    diff_text: diff,
                });
                self.modal_active = true;
                needs_approval = true;
             }
        }

        // Add to Sidebar (Visual Log)
        if self.file_changes.len() >= 50 {
            self.file_changes.pop_back();
        }
        self.file_changes.push_front(FileChange {
            path: file_name,
            kind,
            timestamp: Local::now(),
            diff: diff_output,
        });
        self.list_state.select(Some(0));
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
        move |res: notify::Result<notify::Event>| {
            if let Ok(event) = res {
                use notify::event::{EventKind, ModifyKind};
                match event.kind {
                    EventKind::Create(_) => {
                        for path in event.paths {
                            let _ = tx_watcher.send(AppEvent::FileChange(path, ChangeKind::Create));
                        }
                    }
                    EventKind::Modify(ModifyKind::Data(_)) => {
                        for path in event.paths {
                            let _ = tx_watcher.send(AppEvent::FileChange(path, ChangeKind::Modify));
                        }
                    }
                    EventKind::Modify(ModifyKind::Name(_)) => {
                        for path in event.paths {
                            let _ = tx_watcher.send(AppEvent::FileChange(path, ChangeKind::Modify));
                        }
                    }
                    EventKind::Remove(_) => {
                        for path in event.paths {
                            let _ = tx_watcher.send(AppEvent::FileChange(path, ChangeKind::Remove));
                        }
                    }
                    _ => {}
                }
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

    // 6. Setup App State and Logger
    let app_state = Arc::new(Mutex::new(AppState::new()));

    // Write handle for forwarding input to PTY
    let mut writer = pair.master.take_writer()?;

    // 7. Main Loop
    let loop_result = run_app(
        &mut terminal,
        app_state.clone(),
        rx,
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
    app_state: Arc<Mutex<AppState>>,
    rx: mpsc::Receiver<AppEvent>,
    writer: &mut dyn Write,
    master: &mut dyn portable_pty::MasterPty,
) -> Result<()> {
    loop {
        // A. Process all available events (non-blocking)
        while let Ok(event) = rx.try_recv() {
            match event {
                AppEvent::PtyData(data) => {
                     // Only process PTY data if modal is NOT active? 
                     // No, background PTY should still run/update, just input blocked.
                    let mut state = app_state.lock().unwrap();
                    state.parser.process(&data);
                }
                AppEvent::FileChange(path, kind) => {
                    let mut state = app_state.lock().unwrap();
                    state.add_change(path.clone(), kind.clone());
                }
                AppEvent::Tick => {}
                AppEvent::Input(_) => {}
            }
        }

        // B. Render
        terminal.draw(|frame| {
             // Lock state for rendering
            let mut state = app_state.lock().unwrap();
            
            let area = frame.area();
            
            // 1. Vertical Split
            let v_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(1), Constraint::Length(1)])
                .split(area);
                
            let main_area = v_chunks[0];
            let status_area = v_chunks[1];

            // 2. Horizontal Split
            let (term_area, side_area) = if state.show_sidebar {
                let h_chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
                    .split(main_area);
                (h_chunks[0], Some(h_chunks[1]))
            } else {
                (main_area, None)
            };

            // --- Render Terminal OR Diff View ---
            if state.show_diff_view {
                 // Reuse existing diff view render logic...
                 // (Simplified for brevity block)
                 let block = Block::default()
                    .title(" Diff View (Ctrl+K to Close) ")
                    .borders(Borders::ALL)
                    .style(Style::default().fg(Color::Cyan));
                
                let mut lines = vec![];
                if let Some(idx) = state.list_state.selected() {
                    if let Some(change) = state.file_changes.get(idx) {
                        lines.push(Line::from(vec![Span::styled(format!("File: {}", change.path), Style::default().add_modifier(Modifier::BOLD))]));
                        lines.push(Line::from(""));
                        if let Some(diff_text) = &change.diff {
                            for line_str in diff_text.lines() {
                                if line_str.starts_with('+') { lines.push(Line::from(Span::styled(line_str, Style::default().fg(Color::Green)))); }
                                else if line_str.starts_with('-') { lines.push(Line::from(Span::styled(line_str, Style::default().fg(Color::Red)))); }
                                else { lines.push(Line::from(Span::styled(line_str, Style::default().fg(Color::DarkGray)))); }
                            }
                        }
                    }
                }
                frame.render_widget(Paragraph::new(lines).block(block), term_area);
            } else {
                // Render VT100
                let screen = state.parser.screen();
                let (rows, cols) = screen.size();
                let buffer = frame.buffer_mut();
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
                             if !contents.is_empty() { buffer.set_string(term_area.x + col, term_area.y + row, contents, style); }
                             else { buffer.set_string(term_area.x + col, term_area.y + row, " ", style); }
                        }
                    }
                }
                if !screen.hide_cursor() && !state.modal_active {
                     let (crow, ccol) = screen.cursor_position();
                     if ccol < term_area.width && crow < term_area.height {
                          frame.set_cursor_position(Position { x: term_area.x + ccol, y: term_area.y + crow });
                     }
                }
            }
            
            // --- Render Sidebar ---
            if let Some(area) = side_area {
                // (Existing Sidebar Logic...)
                 let block = Block::default().title(" Active Monitoring ").borders(Borders::ALL).style(Style::default().fg(Color::DarkGray));
                 let now = Local::now();
                 let items: Vec<ListItem> = state.file_changes.iter().map(|c| {
                     let (sym, col) = match c.kind { ChangeKind::Create => ("+", Color::Green), ChangeKind::Modify => ("~", Color::Yellow), ChangeKind::Remove => ("-", Color::Red) };
                     let td = now.signed_duration_since(c.timestamp).num_seconds();
                     ListItem::new(format!("{}s {} {}", td, sym, c.path)).style(Style::default().fg(col))
                 }).collect();
                 frame.render_stateful_widget(List::new(items).block(block).highlight_style(Style::default().add_modifier(Modifier::REVERSED)), area, &mut state.list_state);
            }

            // --- Render Status Bar ---
            let status_text = format!(" Total: {} | Modal: {} (Queue: {}) ", state.file_changes.len(), state.modal_active, state.approval_queue.len());
            frame.render_widget(Paragraph::new(status_text).style(Style::default().fg(Color::Black).bg(Color::White)), status_area);

            // --- RENDER MODAL ---
            if state.modal_active {
                if let Some(pending) = state.approval_queue.front() {
                    let modal_area = centered_rect(60, 60, area);
                    frame.render_widget(Clear, modal_area); // Clear background

                    let block = Block::default()
                        .title(format!(" REVIEW CHANGE: {} ", pending.path))
                        .borders(Borders::ALL)
                        .style(Style::default().bg(Color::Black)) // Ensure opaque
                        .border_style(Style::default().fg(Color::LightRed).add_modifier(Modifier::BOLD));
                    
                    let mut lines = vec![];
                    lines.push(Line::from(Span::styled("Press [y] to Accept, [n] to Reject (Revert)", Style::default().fg(Color::Yellow))));
                    lines.push(Line::from(""));
                    
                    // Show Diff Snippet
                    for line in pending.diff_text.lines().take(20) {
                        if line.starts_with('+') { lines.push(Line::from(Span::styled(line, Style::default().fg(Color::Green)))); }
                        else if line.starts_with('-') { lines.push(Line::from(Span::styled(line, Style::default().fg(Color::Red)))); }
                        else { lines.push(Line::from(Span::styled(line, Style::default().fg(Color::Gray)))); }
                    }
                    if pending.diff_text.lines().count() > 20 {
                        lines.push(Line::from("... (more lines) ..."));
                    }
                    
                    frame.render_widget(Paragraph::new(lines).block(block), modal_area);
                }
            }

        })?;

        // C. Poll Input
        if event::poll(Duration::from_millis(50))? {
             let mut state = app_state.lock().unwrap();
             
             match event::read()? {
                Event::Resize(cols, rows) => { 
                    master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })?; 
                    state.parser = vt100::Parser::new(rows, cols, 0); 
                }
                Event::Key(key) => {
                    // *** MODAL INTERCEPTION ***
                    if state.modal_active {
                        match key.code {
                            KeyCode::Char('y') => {
                                if let Some(pending) = state.approval_queue.pop_front() {
                                    // Accept: Update Cache
                                    if pending.new_content.is_empty() {
                                        state.file_cache.remove(&pending.path);
                                    } else {
                                        state.file_cache.insert(pending.path, pending.new_content);
                                    }
                                }
                                state.modal_active = !state.approval_queue.is_empty();
                            }
                            KeyCode::Char('n') => {
                                if let Some(pending) = state.approval_queue.pop_front() {
                                    // Reject: Revert to Old Content
                                    state.ignore_next_write.insert(pending.path.clone());
                                    
                                    if pending.old_content.is_empty() {
                                        // It was a new file, so delete it
                                        let _ = std::fs::remove_file(&pending.path);
                                    } else {
                                        // Revert content
                                        let _ = std::fs::write(&pending.path, &pending.old_content);
                                    }
                                }
                                state.modal_active = !state.approval_queue.is_empty();
                            }
                            _ => {} // Consume other keys
                        }
                        return Ok(()); // SKIP NORMAL PROCESSING
                    }

                    // *** NORMAL PROCESSING ***
                    match key.code {
                        KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(()),
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => writer.write_all(&[3])?,
                        KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => state.show_diff_view = !state.show_diff_view,
                        KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) => state.show_sidebar = !state.show_sidebar,
                        KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::CONTROL) => { state.file_changes.clear(); state.list_state.select(None); }
                        KeyCode::Up if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            let i = state.list_state.selected().map_or(0, |i| i.saturating_sub(1));
                            state.list_state.select(Some(i));
                        }
                        KeyCode::Down if key.modifiers.contains(KeyModifiers::CONTROL) => {
                             let i = state.list_state.selected().map_or(0, |i| (i + 1).min(state.file_changes.len().saturating_sub(1)));
                             state.list_state.select(Some(i));
                        }
                        // Pass through to PTY
                        KeyCode::Char(c) => writer.write_all(c.to_string().as_bytes())?,
                        KeyCode::Enter => writer.write_all(b"\r")?,
                        KeyCode::Backspace => writer.write_all(&[127])?,
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

// Helper for centering modal
fn centered_rect(percent_x: u16, percent_y: u16, r: ratatui::layout::Rect) -> ratatui::layout::Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn convert_color(c: vt100::Color) -> Color {
    match c {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

fn normalize_path(path: &std::path::Path) -> String {
    // Attempt canonicalization to resolve symlinks/relativity
    if let Ok(abs) = std::fs::canonicalize(path) {
        return abs.to_string_lossy()
            .trim_start_matches(r"\\?\")
            .to_string();
    }
    // Fallback if file missing (e.g. deleted)
    // Assume path is already absolute (from notify) or close to it
    path.to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_string()
}