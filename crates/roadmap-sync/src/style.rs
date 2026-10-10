//! Minimal terminal styling. Disabled when stdout is not a TTY or `NO_COLOR` is set.

use colored::Colorize;

fn use_color() -> bool {
    colored::control::ShouldColorize::from_env().should_colorize()
}

pub fn heading(text: impl AsRef<str>) -> String {
    let text = text.as_ref();
    if use_color() {
        text.bold().to_string()
    } else {
        text.to_string()
    }
}

pub fn tag(text: impl AsRef<str>) -> String {
    let text = text.as_ref();
    if use_color() {
        text.cyan().to_string()
    } else {
        text.to_string()
    }
}

pub fn emphasis(text: impl AsRef<str>) -> String {
    let text = text.as_ref();
    if use_color() {
        text.bold().to_string()
    } else {
        text.to_string()
    }
}

pub fn ok(text: impl AsRef<str>) -> String {
    let text = text.as_ref();
    if use_color() {
        text.green().to_string()
    } else {
        text.to_string()
    }
}

pub fn warn(text: impl AsRef<str>) -> String {
    let text = text.as_ref();
    if use_color() {
        text.yellow().to_string()
    } else {
        text.to_string()
    }
}

pub fn err(text: impl AsRef<str>) -> String {
    let text = text.as_ref();
    if use_color() {
        text.red().to_string()
    } else {
        text.to_string()
    }
}

pub fn job_index(index: usize, total: usize) -> String {
    tag(format!("[{index}/{total}]"))
}

pub fn status_ok() -> String {
    ok("ok")
}

pub fn status_fail() -> String {
    err("FAIL")
}

pub fn status_warn() -> String {
    warn("warning")
}

pub fn status_error() -> String {
    err("error")
}
