use ratatui::style::{Color, Modifier, Style};

pub(super) fn green() -> Style {
    Style::default().fg(Color::Green)
}

pub(super) fn red() -> Style {
    Style::default().fg(Color::Red)
}

pub(super) fn yellow() -> Style {
    Style::default().fg(Color::Yellow)
}

pub(super) fn gray() -> Style {
    Style::default().fg(Color::DarkGray)
}

pub(super) fn bright() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

pub(super) fn cyan() -> Style {
    Style::default().fg(Color::Cyan)
}

pub(super) fn bid_bar() -> Style {
    Style::default().fg(Color::Rgb(38, 110, 62))
}

pub(super) fn ask_bar() -> Style {
    Style::default().fg(Color::Rgb(125, 45, 45))
}

pub(super) fn flash_up() -> Style {
    Style::default()
        .bg(Color::Rgb(30, 90, 50))
        .add_modifier(Modifier::BOLD)
}

pub(super) fn flash_down() -> Style {
    Style::default()
        .bg(Color::Rgb(110, 35, 35))
        .add_modifier(Modifier::BOLD)
}
