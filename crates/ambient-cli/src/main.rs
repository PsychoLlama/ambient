use clap::Parser;

mod cli;

pub fn main() -> anyhow::Result<()> {
    let args = cli::Args::parse();
    println!("{args:?}");

    Ok(())
}
