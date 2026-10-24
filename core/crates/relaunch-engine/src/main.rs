//! `relaunch`: the engine's command line. Everything is in the library.
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        reason = "test code: a failed unwrap/index is a failed assertion"
    )
)]

use relaunch_engine::{cli, env::Env};

fn main() {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let env = Env::from_process();
    let (mut stdin, mut stdout, mut stderr) =
        (std::io::stdin(), std::io::stdout(), std::io::stderr());
    let mut io = cli::Io {
        stdin: &mut stdin,
        stdout: &mut stdout,
        stderr: &mut stderr,
    };
    std::process::exit(cli::run(args, &env, &mut io));
}
