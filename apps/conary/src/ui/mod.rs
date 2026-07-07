// apps/conary/src/ui/mod.rs
//! Single source of truth for user-facing CLI output styling.

use console::style;

/// Per-item indicator used by [`row`]/[`row_line`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Ok,
    Fail,
    Warn,
    Skip,
    Info,
    Off,
    Missing,
    Pending,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Fail => "fail",
            Status::Warn => "warn",
            Status::Skip => "skip",
            Status::Info => "info",
            Status::Off => "off",
            Status::Missing => "missing",
            Status::Pending => "pending",
        }
    }
}

const TAG_COLUMN: usize = 9;

pub fn tag(status: Status) -> String {
    let inner = status.label();
    let styled = match status {
        Status::Ok => style(inner).green(),
        Status::Fail => style(inner).red(),
        Status::Warn => style(inner).yellow(),
        Status::Skip => style(inner).dim(),
        Status::Info => style(inner).cyan(),
        Status::Off => style(inner).dim(),
        Status::Missing => style(inner).red(),
        Status::Pending => style(inner).yellow(),
    };
    format!("[{styled}]")
}

pub fn row_line(status: Status, cells: &[&str]) -> String {
    let visible = status.label().len() + 2;
    let pad = TAG_COLUMN.saturating_sub(visible);
    format!("{}{}  {}", tag(status), " ".repeat(pad), cells.join("  "))
}

pub fn error_line(msg: &str) -> String {
    format!("{}: {msg}", style("error").red().bold())
}

pub fn warn_line(msg: &str) -> String {
    format!("{}: {msg}", style("warning").yellow().bold())
}

pub fn note_line(msg: &str) -> String {
    format!("{}: {msg}", style("note").cyan().bold())
}

pub fn status_line(verb: &str, msg: &str) -> String {
    format!("{} {msg}", style(verb).green().bold())
}

pub fn heading_line(text: &str) -> String {
    style(text).bold().to_string()
}

pub fn field_line(label: &str, value: &str) -> String {
    format!("  {}: {value}", style(label).bold())
}

pub fn error(msg: &str) {
    eprintln!("{}", error_line(msg));
}

pub fn warn(msg: &str) {
    eprintln!("{}", warn_line(msg));
}

pub fn note(msg: &str) {
    eprintln!("{}", note_line(msg));
}

pub fn status(verb: &str, msg: &str) {
    println!("{}", status_line(verb, msg));
}

pub fn row(status: Status, cells: &[&str]) {
    println!("{}", row_line(status, cells));
}

pub fn heading(text: &str) {
    println!("{}", heading_line(text));
}

pub fn field(label: &str, value: &str) {
    println!("{}", field_line(label, value));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain() {
        console::set_colors_enabled(false);
    }

    #[test]
    fn tags_are_lowercase_bracketed_words() {
        plain();
        assert_eq!(tag(Status::Ok), "[ok]");
        assert_eq!(tag(Status::Fail), "[fail]");
        assert_eq!(tag(Status::Warn), "[warn]");
        assert_eq!(tag(Status::Skip), "[skip]");
        assert_eq!(tag(Status::Info), "[info]");
        assert_eq!(tag(Status::Off), "[off]");
        assert_eq!(tag(Status::Missing), "[missing]");
        assert_eq!(tag(Status::Pending), "[pending]");
    }

    #[test]
    fn rows_align_regardless_of_tag_width() {
        plain();
        let short = row_line(Status::Ok, &["alpha"]);
        let long = row_line(Status::Missing, &["beta"]);
        assert_eq!(short.find("alpha"), long.find("beta"));
    }

    #[test]
    fn message_prefixes_are_lowercase() {
        plain();
        assert_eq!(error_line("boom"), "error: boom");
        assert_eq!(warn_line("stale"), "warning: stale");
        assert_eq!(note_line("hint"), "note: hint");
        assert_eq!(status_line("Installing", "nginx"), "Installing nginx");
        assert_eq!(field_line("Arch", "x86_64"), "  Arch: x86_64");
        assert_eq!(heading_line("Installed packages:"), "Installed packages:");
    }
}
