use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    widgets::{Block, Borders, List, ListItem, ListState},
    Frame,
};
use chrono::Local;
use crate::types::{ChangeKind, FileChange};
use crate::ui::theme::Theme;

pub fn render(
    frame: &mut Frame,
    area: Rect,
    changes: &[FileChange],
    state: &mut ListState,
    theme: &Theme,
) {
    let block = Block::default()
        .title(" Active Monitoring ")
        .borders(Borders::ALL)
        .style(Style::default().fg(theme.border_dim))
        .border_style(Style::default().fg(theme.border_dim)); // Subtle border
    
    let now = Local::now();
    
    let styled_items: Vec<ListItem> = changes.iter().map(|change| {
         let color = match change.kind {
            ChangeKind::Create => theme.status_success,
            ChangeKind::Modify => theme.status_warning,
            ChangeKind::Remove => theme.status_error,
        };
        
        let time_diff = now.signed_duration_since(change.timestamp);
        let time_str = if time_diff.num_seconds() < 60 {
            format!("{}s", time_diff.num_seconds())
        } else {
            change.timestamp.format("%H:%M").to_string()
        };
        
        let symbol = match change.kind {
            ChangeKind::Create => "A", // Added
            ChangeKind::Modify => "M", // Modified
            ChangeKind::Remove => "D", // Deleted
        };

        ListItem::new(format!("{:>3} {} {}", time_str, symbol, change.path))
            .style(Style::default().fg(color))
    }).collect();

    let list = List::new(styled_items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(theme.bg_secondary)
                .add_modifier(Modifier::BOLD)
        )
        .highlight_symbol("▎"); // A nice solid bar instead of ">"

    frame.render_stateful_widget(list, area, state);
}
