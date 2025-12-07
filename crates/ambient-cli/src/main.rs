//! Ambient programming language CLI.
//!
//! This is the main entry point for the `ambient` command-line tool.

use std::path::Path;

use anyhow::{bail, Context, Result};
use clap::Parser;

mod cli;
mod commands;
mod diagnostic;
mod repl;
mod serialize;

use cli::{Args, Command};
use commands::{cmd_check, cmd_compile, cmd_dev, cmd_init, cmd_run};
use diagnostic::print_diagnostic;
use repl::cmd_repl;

pub fn main() -> Result<()> {
    let args = Args::parse();

    match args.command {
        Command::Init { path, name } => cmd_init(&path, name.as_deref())?,
        Command::Compile { file, output } => cmd_compile(&file, output.as_deref())?,
        Command::Run { path, entry } => cmd_run(&path, &entry)?,
        Command::Check { file } => cmd_check(&file)?,
        Command::Ast { file } => cmd_ast(&file)?,
        Command::Repl => cmd_repl()?,
        Command::Lsp => cmd_lsp()?,
        Command::Dev { file, entry, watch } => cmd_dev(&file, &entry, watch.as_deref())?,
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
