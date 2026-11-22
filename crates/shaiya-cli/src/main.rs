//! `shaiya`: the Shaiya toolkit command line. Thin over the libraries: it parses arguments, calls a library function with a terminal
//! progress sink, prints the result as text or JSON, and maps the outcome to an exit code (see docs/tools/shaiya.md).
//!
//! Exit codes: 0 success; 1 the answer is "no" (differences, a failed check); 2 usage error; 3 input error (unreadable or bad file);
//! 4 refused by a safety rule; 5 internal error (a panic); 130 cancelled.

/// `writeln!` to a writer, ignoring errors (a closed pipe is not worth reporting).
macro_rules! outln {
    ($w:expr) => {{
        let _ = writeln!($w);
    }};
    ($w:expr, $($arg:tt)*) => {{
        let _ = writeln!($w, $($arg)*);
    }};
}

mod cmd;
mod config;
mod console;
mod output;
mod progress;

use clap::{ArgAction, CommandFactory, Parser, Subcommand, ValueEnum};
use shaiya_core::{Cancel, ErrorKind, Job};
use std::io::IsTerminal;
use std::process::ExitCode;

#[derive(Clone, Copy, ValueEnum)]
enum ProgressArg {
    Auto,
    Always,
    Never,
}

/// What every command needs to know about the global flags.
pub struct Ctx {
    /// `--data-dir`: where the saved client folders live (default: the toolkit data folder).
    pub data_dir: Option<std::path::PathBuf>,
    pub json: bool,
    pub quiet: bool,
    pub verbose: u8,
    pub progress: progress::Mode,
}

#[derive(Parser)]
#[command(
    name = "shaiya",
    bin_name = "shaiya",
    about = "Tools for Shaiya game data. These work on your own copy of the game; nothing is downloaded.",
    disable_version_flag = true,
    arg_required_else_help = true,
    after_help = "Your own copy: these tools never download game files. Changes to a client folder are journaled so they can usually be rolled back, but not with --no-journal or archive compact --in-place; other output goes only where you say."
)]
struct Cli {
    /// Print one JSON document on stdout instead of text
    #[arg(long, global = true)]
    json: bool,
    /// No progress and no notes; errors still go to stderr
    #[arg(short, long, global = true)]
    quiet: bool,
    /// More detail (repeat for more)
    #[arg(short, long, global = true, action = ArgAction::Count)]
    verbose: u8,
    /// When to show progress on stderr
    #[arg(long, global = true, value_enum, default_value = "auto", value_name = "WHEN")]
    progress: ProgressArg,
    /// Use this folder for the saved client folders and settings instead of the default data folder
    #[arg(long, global = true, value_name = "DIR")]
    data_dir: Option<std::path::PathBuf>,
    /// Print the version
    #[arg(short = 'V', long = "version")]
    show_version: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(clap::Args)]
#[command(arg_required_else_help = true, subcommand_required = true)]
struct ArchiveGroup {
    #[command(subcommand)]
    cmd: cmd::archive::Cmd,
}

#[derive(clap::Args)]
#[command(arg_required_else_help = true, subcommand_required = true)]
struct PatchGroup {
    #[command(subcommand)]
    cmd: cmd::patch::Cmd,
}

#[derive(clap::Args)]
#[command(arg_required_else_help = true, subcommand_required = true)]
struct SdataGroup {
    #[command(subcommand)]
    cmd: cmd::sdata::Cmd,
}

#[derive(clap::Args)]
#[command(arg_required_else_help = true, subcommand_required = true)]
struct AnimGroup {
    #[command(subcommand)]
    cmd: cmd::anim::Cmd,
}

#[derive(clap::Args)]
#[command(arg_required_else_help = true, subcommand_required = true)]
struct ClientsGroup {
    #[command(subcommand)]
    cmd: cmd::clients::Cmd,
}

#[derive(clap::Args)]
#[command(arg_required_else_help = true, subcommand_required = true)]
struct SvmapGroup {
    #[command(subcommand)]
    cmd: cmd::svmap::Cmd,
}

#[derive(Subcommand)]
enum Command {
    /// Identify files and describe them (kind, size, details, optional SHA-256)
    Info(cmd::info::Args),
    /// Work with data.sah/data.saf archives: list, extract, edit with a backup, compare, pack
    Archive(ArchiveGroup),
    /// Patch files: build, verify, apply with a backup, roll back, sign
    Patch(PatchGroup),
    /// SData files: info, check, decrypt, encrypt
    Sdata(SdataGroup),
    /// Animation and model files (.ANI, .3DC, .VANI, .MANI, .3DO, .3DE): describe them; check .ANI files and compare their skeleton with the file a candidate replaces
    Anim(AnimGroup),
    /// The saved "my client folders" list (use them as @name)
    Clients(ClientsGroup),
    /// Server map files (.svmap): check, dump, build, round trip, compare; check a client's .wld files; spawn report; bulk-edit dry run
    Svmap(SvmapGroup),
    /// Print the version, git revision and target
    Version,
}

pub(crate) fn exit_code_for(kind: ErrorKind) -> u8 {
    match kind {
        ErrorKind::Io | ErrorKind::Format | ErrorKind::Unsupported => 3,
        ErrorKind::Refused => 4,
        ErrorKind::Cancelled => 130,
    }
}

fn run() -> u8 {
    let raw: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if raw.len() == 1 && console::started_by_double_click() {
        let _ = Cli::command().print_help();
        println!("\nThis is a command-line program. Open a terminal and run `shaiya --help`.\nPress Enter to close.");
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        return 0;
    }
    let cli = match Cli::try_parse_from(&raw) {
        Ok(c) => c,
        Err(e) => {
            use clap::error::ErrorKind as K;
            let ok = matches!(e.kind(), K::DisplayHelp | K::DisplayVersion);
            let wants_json = raw.iter().any(|a| a == "--json");
            if wants_json && !ok {
                let msg = e.render().to_string();
                println!("{}", serde_json::json!({ "error": { "kind": "usage", "message": msg.trim() } }));
            }
            let _ = e.print();
            return if ok { 0 } else { 2 };
        }
    };
    let ctx = Ctx {
        data_dir: cli.data_dir.clone(),
        json: cli.json,
        quiet: cli.quiet,
        verbose: cli.verbose,
        progress: match cli.progress {
            ProgressArg::Auto => progress::Mode::Auto,
            ProgressArg::Always => progress::Mode::Always,
            ProgressArg::Never => progress::Mode::Never,
        },
    };
    if cli.show_version && cli.command.is_none() {
        println!("{}", cmd::version::current().line());
        return 0;
    }
    match cli.command {
        Some(Command::Version) => {
            output::emit(&ctx, &cmd::version::current());
            0
        }
        Some(command) => {
            let sink = progress::TermProgress::new(ctx.progress, ctx.quiet, std::io::stderr().is_terminal());
            let job = Job::new(&sink, Cancel::new());
            let code = match command {
                Command::Info(args) => cmd::info::run(&ctx, &args, &job),
                Command::Archive(g) => cmd::archive::run(&ctx, &g.cmd, &job),
                Command::Patch(g) => cmd::patch::run(&ctx, &g.cmd, &job),
                Command::Sdata(g) => cmd::sdata::run(&ctx, &g.cmd, &job),
                Command::Anim(g) => cmd::anim::run(&ctx, &g.cmd, &job),
                Command::Clients(g) => cmd::clients::run(&ctx, &g.cmd),
                Command::Svmap(g) => cmd::svmap::run(&ctx, &g.cmd, &job),
                Command::Version => unreachable!("handled above"),
            };
            sink.finish();
            code as u8
        }
        None => {
            let _ = Cli::command().print_help();
            2
        }
    }
}

fn main() -> ExitCode {
    std::panic::set_hook(Box::new(|info| {
        eprintln!("error: internal error (this is a bug): {info}");
        eprintln!("hint: please report it at https://github.com/byyako/shaiya-toolkit/issues and say which command you ran");
    }));
    match std::panic::catch_unwind(run) {
        Ok(code) => ExitCode::from(code),
        Err(_) => ExitCode::from(5),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_follow_the_error_kind() {
        assert_eq!(exit_code_for(ErrorKind::Io), 3);
        assert_eq!(exit_code_for(ErrorKind::Format), 3);
        assert_eq!(exit_code_for(ErrorKind::Unsupported), 3);
        assert_eq!(exit_code_for(ErrorKind::Refused), 4);
        assert_eq!(exit_code_for(ErrorKind::Cancelled), 130);
    }

    #[test]
    fn the_command_line_is_well_formed() {
        Cli::command().debug_assert();
    }
}
