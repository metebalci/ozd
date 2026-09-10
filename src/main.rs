// SPDX-FileCopyrightText: 2026 Mete Balci
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The `muir-ah` daemon: its command line, its startup, and its loop
//! (`DESIGN.md` §4, §10).
//!
//! ```text
//! muir-ah [--trace] [--check] <config>
//! ```
//!
//! `--trace` prints every packet, every packet passed on, and every drop
//! with why. `--check` reads the config and runs the startup checks, binds
//! nothing, and exits 0 if all is well and 1 if not. The usage and every
//! error go to stderr: a command line that is not one config file exits 2,
//! a daemon that cannot start exits 1, and one that starts runs until it is
//! stopped --- `SIGTERM`'s default action is the shutdown (`DESIGN.md` §10).

use muir_ah::config::Config;
use muir_ah::daemon::Daemon;
use muir_ah::log;
use muir_ah::roots::Tree;
use std::path::PathBuf;
use std::process::exit;
use std::time::{Duration, Instant};

const USAGE: &str = "usage: muir-ah [--trace] [--check] <config>";

/// How long the loop waits for a datagram before it turns anyway: an idle
/// daemon wakes ten times a second, which costs nothing measurable, and a
/// retransmission is at most this late against its 500 ms (`DESIGN.md`
/// §4).
const WAIT: Duration = Duration::from_millis(100);

fn main() {
    let (mut trace, mut check, mut path) = (false, false, None);
    for arg in std::env::args_os().skip(1) {
        match arg.to_str() {
            Some("--trace") => trace = true,
            Some("--check") => check = true,
            Some(flag) if flag.starts_with('-') => usage(&format!("{flag}: not a flag")),
            _ if path.is_some() => usage("one config file, not two"),
            _ => path = Some(PathBuf::from(arg)),
        }
    }
    let Some(path) = path else { usage("a config file is wanted") };

    // The startup checks, each a refusal to start, all before anything is
    // bound (`DESIGN.md` §6). The first is who is running this.
    if running_as_root() {
        fail(
            "refusing to run as root: nothing here needs a privilege, and as root a \
             containment bug would reach every file on this host; run it as a user that \
             owns its roots and nothing else (DESIGN.md §6)",
        );
    }
    let config = Config::load(&path).unwrap_or_else(|e| {
        eprintln!("{}: {e}", path.display());
        exit(1);
    });
    // The roots: each canonicalised, a directory, not `/`, writable unless
    // `readonly`, and inside no other; a base directory a mount covers is
    // warned of (`DESIGN.md` §6). `--check` runs these and stops there,
    // having changed nothing.
    let tree = Tree::new(config.roots.clone()).unwrap_or_else(|e| fail(&e));
    for warning in tree.warnings() {
        log::event(format_args!("warning: {warning}"));
    }
    if check {
        exit(0);
    }
    // A daemon killed mid-write leaves a FILE temporary: each is removed
    // before FILE can make another, and nothing else is touched.
    for removed in tree.remove_temporaries() {
        match removed {
            Ok(path) => log::event(format_args!("removed a stale temporary, {}", path.display())),
            Err(why) => log::event(format_args!("a stale temporary is not removed: {why}")),
        }
    }

    let mut daemon = Daemon::new(&config, trace)
        .unwrap_or_else(|e| fail(&format!("listen {}: {e}", config.listen)));
    log::event(format_args!(
        "muir-ah {}: {} at {:o}, listening at {}",
        env!("CARGO_PKG_VERSION"),
        config.names[0],
        config.address,
        daemon.at()
    ));
    let start = Instant::now();
    loop {
        daemon.turn(start.elapsed().as_nanos() as u64, WAIT);
    }
}

/// Whether this process runs as root, by its effective user id, which is
/// what decides what it may touch.
///
/// **The one `unsafe` in the package** (`DESIGN.md` §2): `geteuid`, from the
/// C library std links already. POSIX: it "shall always be successful", and
/// it takes nothing, so there is no memory for it to misuse. Its `uid_t` is
/// 32 bits unsigned --- `unsigned int` on Linux, read in glibc's
/// `bits/typesizes.h`; `__uint32_t` on macOS, by its `sys/_types.h`,
/// **unverified** here.
#[allow(unsafe_code)]
fn running_as_root() -> bool {
    unsafe extern "C" {
        fn geteuid() -> u32;
    }
    // SAFETY: no argument, no failure, no memory; as above.
    unsafe { geteuid() == 0 }
}

/// A command line that is not `muir-ah [--trace] [--check] <config>`: what
/// is wrong, and the usage, on stderr; exit code 2.
fn usage(msg: &str) -> ! {
    eprintln!("muir-ah: {msg}");
    eprintln!("{USAGE}");
    exit(2);
}

/// A daemon that cannot start, for a reason that is not the command line's
/// shape: why, on stderr; exit code 1.
fn fail(msg: &str) -> ! {
    eprintln!("muir-ah: {msg}");
    exit(1);
}
