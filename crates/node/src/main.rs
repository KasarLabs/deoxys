//! Deoxys node command line.
#![warn(missing_docs)]

#[macro_use]
mod service;
mod benchmarking;
mod chain_spec;
mod cli;
mod command;
mod commands;
mod configs;
mod genesis_block;
mod rpc;

fn main() -> sc_cli::Result<()> {
    command::run()
}
