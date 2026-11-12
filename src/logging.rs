// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! Tracing setup. Verbosity comes from the CLI, overridable with `IAGO_LOG`.
//!
//! Both binaries also append to a log file, because the control center offers
//! "Open logs" for troubleshooting and that file has to exist even when the
//! process was started from a terminal where nobody is reading the output.

use std::path::Path;

use tracing_subscriber::EnvFilter;

/// Installs the global subscriber, writing to stderr only.
pub fn init(verbose: u8) {
    init_with_file(verbose, None);
}

/// Installs the global subscriber, appending to `log_file` when given.
pub fn init_with_file(verbose: u8, log_file: Option<&Path>) {
    let fallback = match verbose {
        0 => "iago=info,warn",
        1 => "iago=debug,info",
        _ => "iago=trace,debug",
    };

    let filter = EnvFilter::try_from_env("IAGO_LOG")
        .or_else(|_| EnvFilter::try_new(fallback))
        .unwrap_or_else(|_| EnvFilter::new("iago=info"));

    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_ansi(false);

    if let Some(path) = log_file {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        if let Ok(file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = builder
                .with_writer(move || file.try_clone().expect("the log file went away"))
                .try_init();
            return;
        }
    }

    let _ = builder.with_writer(std::io::stderr).try_init();
}
