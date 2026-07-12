//! Ambient programming language CLI.
//!
//! This is the main entry point for the `ambient` command-line tool; all
//! logic lives in the `ambient_cli` library crate.

use anyhow::Result;

pub fn main() -> Result<()> {
    ambient_cli::run()
}
