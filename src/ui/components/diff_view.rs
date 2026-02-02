use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use crate::types::FileChange;
use crate::ui::theme::Theme;

pub fn render(frame: &mut Frame, area: Rect, change: Option<&FileChange>, theme: &Theme) {
    let block = Block::default()
        .title(" Diff View ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.status_info)) // Highlight border to show it's active
        .style(Style::default().bg(theme.bg_primary));

    let mut lines = vec![];

    if let Some(change) = change {
        lines.push(Line::from(vec![
            Span::styled(format!("File: {}", change.path), Style::default().add_modifier(Modifier::BOLD).fg(theme.text_main))
        ]));
        lines.push(Line::from(""));

        if let Some(diff_text) = &change.diff {
            for line_str in diff_text.lines() {
                if line_str.starts_with('+') {
                    lines.push(Line::from(Span::styled(line_str, Style::default().fg(theme.status_success))));
                } else if line_str.starts_with('-') {
                    lines.push(Line::from(Span::styled(line_str, Style::default().fg(theme.status_error))));
                } else if line_str.starts_with('@') {
                     lines.push(Line::from(Span::styled(line_str, Style::default().fg(theme.status_info))));
                } else {
                    lines.push(Line::from(Span::styled(line_str, Style::default().fg(theme.text_muted))));
                }
            }
        } else {
            lines.push(Line::from(Span::styled("No diff details available.", Style::default().fg(theme.text_muted))));
        }
    } else {
        lines.push(Line::from(Span::styled("Select a file to see changes.", Style::default().fg(theme.text_muted))));
    }

    let p = Paragraph::new(lines).block(block);
    frame.render_widget(p, area);
}
