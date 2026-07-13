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
    /// Initialize a new Ambient package.
    Init {
        /// Directory to initialize. Creates it if it doesn't exist.
        /// Defaults to the current directory.
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,

        /// Package name. Defaults to directory name.
        #[arg(long)]
        name: Option<String>,
    },

    /// Compile an Ambient source file to bytecode.
    Compile {
        /// The source file to compile (.ab).
        #[arg(value_name = "FILE")]
        file: PathBuf,

        /// Output file path. Defaults to <input>.ambient.
        #[arg(short, long, value_name = "OUTPUT")]
        output: Option<PathBuf>,
    },

    /// Run an Ambient package.
    Run {
        /// Path to package directory or .ambient bytecode file.
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,

        /// Function to execute (defaults to "run").
        #[arg(long, default_value = "run")]
        entry: String,

        /// Arguments passed to the program, available through
        /// `core::system::Env::args!()` after the program path at index 0.
        /// Everything after `--` lands here (hyphen-led values included).
        #[arg(
            trailing_var_arg = true,
            allow_hyphen_values = true,
            value_name = "ARGS"
        )]
        args: Vec<String>,
    },

    /// Check an Ambient source file or package for errors without compiling.
    Check {
        /// The source file (.ab) or package directory to check.
        #[arg(value_name = "PATH")]
        file: PathBuf,
    },

    /// Parse and dump the AST of an Ambient source file.
    Ast {
        /// The source file to parse (.ab).
        #[arg(value_name = "FILE")]
        file: PathBuf,
    },

    /// Start an interactive REPL session.
    Repl {
        /// Project directory for completions (defaults to current directory).
        /// Can be the package root (containing ambient.toml) or any subdirectory.
        #[arg(long, value_name = "DIR")]
        project: Option<PathBuf>,
    },

    /// Start the Language Server Protocol (LSP) server.
    ///
    /// This command starts an LSP server that communicates via stdin/stdout.
    /// It is typically invoked by an editor or IDE, not run manually.
    Lsp,

    /// Inspect and maintain a package's content-addressed store.
    Store {
        /// Package directory (defaults to the current directory; searches
        /// upward for ambient.toml).
        #[arg(long, value_name = "DIR", default_value = ".")]
        package: PathBuf,

        #[command(subcommand)]
        command: StoreCommand,
    },

    /// Run an Ambient program with live upgrade.
    ///
    /// Watches for source changes; each change compiles and deploys onto
    /// the running system — rebound names land at the next late-bound
    /// point (a task's next pass, a `Live::latest!` read), cells keep
    /// their values, undeclared tasks drain, and programs without tasks
    /// simply rerun.
    Dev {
        /// Path to a package directory or bare source file (.ab).
        #[arg(value_name = "PATH", default_value = ".")]
        file: PathBuf,

        /// Function to execute (defaults to "run").
        #[arg(long, default_value = "run")]
        entry: String,

        /// Directories to watch for changes (defaults to the package
        /// directory or the file's directory).
        #[arg(long, value_name = "DIR")]
        watch: Option<Vec<PathBuf>>,
    },
}

/// Subcommands for `ambient store`.
#[derive(Subcommand, Debug)]
pub enum StoreCommand {
    /// Show object counts, sizes, and binding counts.
    Stats,

    /// List named bindings and their hashes.
    Ls,

    /// Show an object: metadata, dependencies, and disassembly.
    ///
    /// REF is a bound name (see `ambient store ls`) or a hash prefix.
    Show {
        #[arg(value_name = "REF")]
        reference: String,
    },

    /// Print the transitive dependency tree of a function.
    Deps {
        #[arg(value_name = "REF")]
        reference: String,
    },

    /// Verify every object: decode, re-hash, and check references.
    Verify,

    /// Delete objects unreachable from the names index.
    Gc,

    /// Summarize the current build snapshot (incremental-compilation
    /// manifest): its hash, package, and per-module interface/AST hashes.
    Snapshot,

    /// Alias a build snapshot's manifest under a human name, or list tags.
    ///
    /// With no NAME, lists every tag. With NAME only, tags the current
    /// snapshot. With NAME and TARGET (a tag or manifest-hash prefix), tags
    /// that manifest. A tagged snapshot (and its objects) survives `gc`.
    Tag {
        /// The tag name. Omit to list all tags.
        #[arg(value_name = "NAME")]
        name: Option<String>,

        /// The manifest to tag: a tag name or manifest-hash prefix. Defaults
        /// to the current snapshot.
        #[arg(value_name = "TARGET")]
        target: Option<String>,
    },

    /// Diff two build snapshots: modules and per-item name bindings
    /// added/removed/changed, plus object-count and byte-size deltas.
    ///
    /// A and B are tags, manifest-hash prefixes, or `current` (the snapshot
    /// pointer). Both are required — there is no recorded "previous".
    Diff {
        /// The base snapshot (tag, manifest-hash prefix, or `current`).
        #[arg(value_name = "A")]
        a: Option<String>,

        /// The target snapshot (tag, manifest-hash prefix, or `current`).
        #[arg(value_name = "B")]
        b: Option<String>,

        /// Emit machine-readable JSON instead of human-readable text.
        #[arg(long)]
        json: bool,
    },
}
