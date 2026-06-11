// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Console text colorization helpers.

use std::env;

/// Manages ANSI terminal escape codes for colorizing output.
///
/// Colors are automatically disabled if standard output is not a TTY,
/// if `NO_COLOR` is set, or if the terminal is dumb, unless overrides like
/// `FORCE_COLOR` or `CLICOLOR_FORCE` are specified.
#[derive(Clone)]
pub struct Colors {
    enabled: bool,
}

impl Colors {
    /// Detects environment variables and TTY status to determine if colors should be enabled.
    pub fn new() -> Self {
        let is_tty = unsafe { libc::isatty(libc::STDOUT_FILENO) } != 0;
        let no_color = env::var("NO_COLOR").is_ok();
        let term_dumb = env::var("TERM").map(|v| v == "dumb").unwrap_or(false);
        let force_color = env::var("FORCE_COLOR").is_ok() || env::var("CLICOLOR_FORCE").is_ok();

        let enabled = (is_tty || force_color) && !no_color && !term_dumb;
        Colors { enabled }
    }

    /// Creates a manual instance of `Colors` for testing.
    #[cfg(test)]
    pub fn new_test(enabled: bool) -> Self {
        Colors { enabled }
    }

    /// Wraps text in ANSI green escape codes if color is enabled.
    pub fn green(&self, text: &str) -> String {
        if self.enabled {
            format!("\x1b[32m{}\x1b[0m", text)
        } else {
            text.to_string()
        }
    }

    /// Wraps text in ANSI blue escape codes if color is enabled.
    pub fn blue(&self, text: &str) -> String {
        if self.enabled {
            format!("\x1b[34m{}\x1b[0m", text)
        } else {
            text.to_string()
        }
    }

    /// Wraps text in ANSI yellow escape codes if color is enabled.
    pub fn yellow(&self, text: &str) -> String {
        if self.enabled {
            format!("\x1b[33m{}\x1b[0m", text)
        } else {
            text.to_string()
        }
    }

    /// Wraps text in ANSI magenta escape codes if color is enabled.
    pub fn magenta(&self, text: &str) -> String {
        if self.enabled {
            format!("\x1b[35m{}\x1b[0m", text)
        } else {
            text.to_string()
        }
    }

    /// Wraps text in ANSI bold escape codes if color is enabled.
    pub fn bold(&self, text: &str) -> String {
        if self.enabled {
            format!("\x1b[1m{}\x1b[0m", text)
        } else {
            text.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_colors_disabled() {
        let colors = Colors::new_test(false);
        assert_eq!(colors.green("hello"), "hello");
        assert_eq!(colors.blue("world"), "world");
        assert_eq!(colors.yellow("foo"), "foo");
        assert_eq!(colors.magenta("bar"), "bar");
        assert_eq!(colors.bold("bold"), "bold");
    }

    #[test]
    fn test_colors_enabled() {
        let colors = Colors::new_test(true);
        assert_eq!(colors.green("hello"), "\x1b[32mhello\x1b[0m");
        assert_eq!(colors.blue("world"), "\x1b[34mworld\x1b[0m");
        assert_eq!(colors.yellow("foo"), "\x1b[33mfoo\x1b[0m");
        assert_eq!(colors.magenta("bar"), "\x1b[35mbar\x1b[0m");
        assert_eq!(colors.bold("bold"), "\x1b[1mbold\x1b[0m");
    }
}
