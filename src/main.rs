use clap::{CommandFactory, FromArgMatches};
use tracing::level_filters::LevelFilter;
use tracing_subscriber::EnvFilter;

mod cachix;
mod cli;
mod commands;
mod config;
mod cve;
mod database;
mod directory_diff;
mod git;
mod github;
mod gitlab;
mod hash_discovery;
mod init;
mod nix;
mod package;
mod paths;
mod pypi;
mod repology;
mod rewrite;
mod variant_strategy;
mod vcs_sources;

#[cfg(test)]
mod tests;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Increase file descriptor limit to avoid "Too many open files" errors
    // when processing many packages concurrently
    if let Err(e) = init::increase_fd_limit() {
        tracing::warn!("Failed to increase file descriptor limit: {}", e);
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::INFO.into())
                .from_env_lossy(),
        )
        .with_ansi(true)
        .with_level(true)
        .with_target(true)
        .with_timer(tracing_subscriber::fmt::time())
        .init();

    let args = parse_args();
    args.command.execute().await
}

/// Parse CLI args, applying `--color <when>` to clap's help/usage rendering.
///
/// Clap's `Command::color()` controls ANSI emission for help/usage and error
/// output. We pre-scan `argv` for `--color` so the choice is honored even when
/// the user passes `--help` (which clap handles before our code sees the parsed
/// `Args`). The same flag is also declared on `Args`, so it appears in
/// `--help` and is validated against the `ColorWhen` enum during full parsing.
fn parse_args() -> cli::Args {
    let choice = preparse_color_choice(std::env::args_os().skip(1));
    let cmd = cli::Args::command().color(choice);
    let matches = cmd.get_matches();
    match cli::Args::from_arg_matches(&matches) {
        Ok(args) => args,
        Err(err) => err.exit(),
    }
}

/// Scan raw args for `--color <WHEN>` / `--color=<WHEN>` and translate to
/// `clap::ColorChoice`. Unknown / malformed values fall back to `Auto`; clap's
/// regular parser will surface the proper error message later.
fn preparse_color_choice<I, S>(args: I) -> clap::ColorChoice
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        let Some(s) = arg.as_ref().to_str() else {
            continue;
        };
        let value = if let Some(rest) = s.strip_prefix("--color=") {
            Some(rest.to_owned())
        } else if s == "--color" {
            iter.next()
                .and_then(|v| v.as_ref().to_str().map(str::to_owned))
        } else {
            None
        };
        if let Some(v) = value {
            return match v.as_str() {
                "always" => clap::ColorChoice::Always,
                "never" => clap::ColorChoice::Never,
                _ => clap::ColorChoice::Auto,
            };
        }
    }
    clap::ColorChoice::Auto
}
