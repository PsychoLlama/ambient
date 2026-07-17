//! Ambient programming language CLI library.
//!
//! The `ambient` binary is a thin driver over [`run`]. The crate is also
//! exposed as a library so integration tests can drive the REPL session in
//! process (see [`repl::session`]) instead of through a PTY.

use std::path::Path;

use anyhow::{Context, Result, bail};
use clap::Parser;

pub mod cli;
pub mod commands;
pub mod diagnostic;
pub mod repl;

use cli::{Args, Command};
use commands::{cmd_build, cmd_check, cmd_dev, cmd_init, cmd_run, cmd_store};
use diagnostic::print_diagnostic;
use repl::cmd_repl;

/// Parse CLI arguments and dispatch to the selected command.
pub fn run() -> Result<()> {
    let args = Args::parse();

    match args.command {
        Command::Init { path, name } => cmd_init(&path, name.as_deref())?,
        Command::Build {
            file,
            output,
            package,
        } => cmd_build(&file, output.as_deref(), package.as_deref())?,
        Command::Run {
            path,
            entry,
            package,
            args,
        } => cmd_run(&path, &entry, args, package.as_deref())?,
        Command::Check { file } => cmd_check(&file)?,
        Command::Ast { file } => cmd_ast(&file)?,
        Command::Repl { project } => cmd_repl(project.as_deref())?,
        Command::Lsp => cmd_lsp()?,
        Command::Dev {
            file,
            entry,
            package,
            watch,
        } => cmd_dev(&file, &entry, package.as_deref(), watch.as_deref())?,
        Command::Store {
            path,
            package,
            command,
        } => cmd_store(&path, package.as_deref(), &command)?,
    }

    Ok(())
}

/// Parse and dump the AST.
fn cmd_ast(file: &Path) -> Result<()> {
    let source = commands::read_source(file)?;

    let module = match ambient_parser::parse(&source) {
        Ok(m) => m,
        Err(e) => {
            print_diagnostic(&source, file, &e);
            bail!("parse error in {}", file.display());
        }
    };

    println!("{module:#?}");

    Ok(())
}

/// Run the LSP server.
fn cmd_lsp() -> Result<()> {
    ambient_lsp::run_server().context("LSP server error")
}
