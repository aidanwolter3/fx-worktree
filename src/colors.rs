// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::env;

#[derive(Clone)]
pub struct Colors {
    enabled: bool,
}

impl Colors {
    pub fn new() -> Self {
        let is_tty = unsafe { libc::isatty(libc::STDOUT_FILENO) } != 0;
        let no_color = env::var("NO_COLOR").is_ok();
        let term_dumb = env::var("TERM").map(|v| v == "dumb").unwrap_or(false);
        let force_color = env::var("FORCE_COLOR").is_ok() || env::var("CLICOLOR_FORCE").is_ok();
        
        let enabled = (is_tty || force_color) && !no_color && !term_dumb;
        Colors { enabled }
    }

    pub fn green(&self, text: &str) -> String {
        if self.enabled {
            format!("\x1b[32m{}\x1b[0m", text)
        } else {
            text.to_string()
        }
    }

    pub fn blue(&self, text: &str) -> String {
        if self.enabled {
            format!("\x1b[34m{}\x1b[0m", text)
        } else {
            text.to_string()
        }
    }

    pub fn yellow(&self, text: &str) -> String {
        if self.enabled {
            format!("\x1b[33m{}\x1b[0m", text)
        } else {
            text.to_string()
        }
    }

    pub fn bold(&self, text: &str) -> String {
        if self.enabled {
            format!("\x1b[1m{}\x1b[0m", text)
        } else {
            text.to_string()
        }
    }
}
