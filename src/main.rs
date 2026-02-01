use anyhow::Result;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::Position,
    style::{Color, Modifier, Style},
    Terminal,
};
use std::{
    io::{Read, Write},
    sync::mpsc,
    thread,
    time::Duration,
};

fn main() -> Result<()> {
    // 1. Setup PTY
    let pty_system = native_pty_system();
    let mut pair = pty_system.openpty(PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    // Use bash for WSL (or zsh if you prefer)
    let cmd = CommandBuilder::new("bash");
    let mut child = pair.slave.spawn_command(cmd)?;

    // 2. Setup Channel for PTY Output
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let mut reader = pair.master.try_clone_reader()?;
    
    // Separate thread to read from PTY
    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(n) if n > 0 => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                _ => break,
            }
        }
    });

    // 3. Setup TUI (Ratatui + Crossterm)
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // 4. Setup VT100 Parser
    let mut parser = vt100::Parser::new(24, 80, 0);

    // Write handle for forwarding input to PTY
    let mut writer = pair.master.take_writer()?;

    // 5. Main Loop
    let loop_result = run_app(
        &mut terminal,
        &mut parser,
        &rx,
        &mut writer,
        &mut *pair.master,
    );

    // 6. Cleanup
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    // Kill PTY process if still running
    let _ = child.kill();

    loop_result
}

fn run_app(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    parser: &mut vt100::Parser,
    rx: &mpsc::Receiver<Vec<u8>>,
    writer: &mut dyn Write,
    master: &mut dyn portable_pty::MasterPty,
) -> Result<()> {
    loop {
        // A. Process all available PTY output
        while let Ok(data) = rx.try_recv() {
            parser.process(&data);
        }

        // B. Render
        terminal.draw(|frame| {
            let screen = parser.screen();
            let area = frame.area();
            let (rows, cols) = screen.size();
            let buffer = frame.buffer_mut();
            
            // Render the VT100 screen to the buffer
            for row in 0..rows.min(area.height) {
                for col in 0..cols.min(area.width) {
                    if let Some(cell) = screen.cell(row, col) {
                        let fg = convert_color(cell.fgcolor());
                        let bg = convert_color(cell.bgcolor());
                        
                        let mut style = Style::default().fg(fg).bg(bg);
                        if cell.bold() {
                            style = style.add_modifier(Modifier::BOLD);
                        }
                        if cell.italic() {
                            style = style.add_modifier(Modifier::ITALIC);
                        }
                        if cell.underline() {
                            style = style.add_modifier(Modifier::UNDERLINED);
                        }
                        if cell.inverse() {
                            style = style.add_modifier(Modifier::REVERSED);
                        }

                        let contents = cell.contents();
                        if !contents.is_empty() {
                            buffer.set_string(col, row, contents, style);
                        } else {
                            buffer.set_string(col, row, " ", style);
                        }
                    }
                }
            }

            // Render Cursor
            if !screen.hide_cursor() {
                let (crow, ccol) = screen.cursor_position();
                frame.set_cursor_position(Position { x: ccol, y: crow });
            }
        })?;

        // C. Poll Input (non-blocking wait)
        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                // Resize Event
                Event::Resize(cols, rows) => {
                    master.resize(PtySize {
                        rows,
                        cols,
                        pixel_width: 0,
                        pixel_height: 0,
                    })?;
                    // TODO: This clears screen history. Future: preserve buffer on resize
                    *parser = vt100::Parser::new(rows, cols, 0);
                }
                
                // Key Events
                Event::Key(key) => {
                    match key.code {
                        KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            // Changed: Ctrl+Q to exit (not just 'q')
                            return Ok(());
                        }
                        
                        // Ctrl+C - forward to shell
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            writer.write_all(&[3])?;
                        }
                        
                        // Ctrl+D - forward to shell
                        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            writer.write_all(&[4])?;
                        }

                        // Regular character input
                        KeyCode::Char(c) => {
                            writer.write_all(c.to_string().as_bytes())?;
                        }
                        
                        KeyCode::Enter => {
                            writer.write_all(b"\r")?;
                        }
                        
                        KeyCode::Backspace => {
                            writer.write_all(&[127])?; // DEL for bash
                        }
                        
                        KeyCode::Tab => {
                            writer.write_all(&[9])?;
                        }
                        
                        KeyCode::Esc => {
                            writer.write_all(&[27])?;
                        }
                        
                        // Arrow keys
                        KeyCode::Up => {
                            writer.write_all(b"\x1b[A")?;
                        }
                        KeyCode::Down => {
                            writer.write_all(b"\x1b[B")?;
                        }
                        KeyCode::Right => {
                            writer.write_all(b"\x1b[C")?;
                        }
                        KeyCode::Left => {
                            writer.write_all(b"\x1b[D")?;
                        }
                        
                        _ => {}
                    }
                    writer.flush()?;
                }
                _ => {}
            }
        }
    }
}

// Convert vt100 color to ratatui color
fn convert_color(c: vt100::Color) -> Color {
    match c {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}