use ratatui::{
    layout::Rect,
    style::Style,
    widgets::Paragraph,
    Frame,
};
use crate::types::ChangeKind;
use crate::types::FileChange;
use crate::ui::theme::Theme;

pub fn render(frame: &mut Frame, area: Rect, changes: &[FileChange], theme: &Theme) {
    let total = changes.len();
    let created = changes.iter().filter(|c| c.kind == ChangeKind::Create).count();
    let modified = changes.iter().filter(|c| c.kind == ChangeKind::Modify).count();
    let removed = changes.iter().filter(|c| c.kind == ChangeKind::Remove).count();

    // Shadcn style: Clean, minimal status bar. No garish background.
    // Maybe just text with some colored dots.

    let status_text = format!(
        "  AI Terminal  |  Theme: {} (Ctrl+T)  |  Total: {}  |  +{}  ~{}  -{}  |  Ctrl+H: Sidebar  Ctrl+K: Diff  Ctrl+L: Clear",
        theme.variant.name(), total, created, modified, removed
    );

    let p = Paragraph::new(status_text)
        .style(Style::default().fg(theme.text_main).bg(theme.border_dim)); // Subtle bar at bottom
    
    frame.render_widget(p, area);
}
