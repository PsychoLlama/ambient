//! Command-line interface for the Ambient language.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// Ambient programming language CLI.
#[derive(Parser, Debug)]
#[command(name = "ambient")]
#[command(author, version, about, long_about = None)]
pub struct Args {
    #[command(subcommand)]
    pub command: Command,
}

/// Available commands.
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Compile an Ambient source file to bytecode.
    Compile {
        /// The source file to compile (.ab).
        #[arg(value_name = "FILE")]
        file: PathBuf,

        /// Output file path. Defaults to <input>.ambient.
        #[arg(short, long, value_name = "OUTPUT")]
        output: Option<PathBuf>,
    },

    /// Run an Ambient program.
    Run {
        /// The file to run (.ab source or .ambient bytecode).
        #[arg(value_name = "FILE")]
        file: PathBuf,

        /// Function to execute (defaults to "main").
        #[arg(long, default_value = "main")]
        entry: String,
    },

    /// Check an Ambient source file for errors without compiling.
    Check {
        /// The source file to check (.ab).
        #[arg(value_name = "FILE")]
        file: PathBuf,
    },

    /// Parse and dump the AST of an Ambient source file.
    Ast {
        /// The source file to parse (.ab).
        #[arg(value_name = "FILE")]
        file: PathBuf,
    },

    /// Start an interactive REPL session.
    Repl,

    /// Start the Language Server Protocol (LSP) server.
    ///
    /// This command starts an LSP server that communicates via stdin/stdout.
    /// It is typically invoked by an editor or IDE, not run manually.
    Lsp,
}
