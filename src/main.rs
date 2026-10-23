//! herdr-newtab-plus: a new tab that asks which folder, and which agent, first.

mod agents;
mod app;
mod cli;
mod client;
mod config;
mod fuzzy;
mod icons;
mod keys;
mod paint;
mod par;
mod paths;
mod proc;
mod pyjson;
mod pyos;
mod pystr;
mod render;
mod sequences;
mod session;
mod state;
mod term;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod testutil;
mod textutil;
mod theme;
#[rustfmt::skip]
mod unicode_tables;

fn main() {
    let args: Vec<String> = std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
    let code = cli::main(args);
    std::process::exit(code);
}
