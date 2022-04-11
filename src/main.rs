#[macro_use]
extern crate lazy_static;

mod audio;
mod battle;
mod bn6;
mod config;
mod datachannel;
mod fastforwarder;
mod game;
mod gui;
mod input;
mod mgba;
mod protocol;
mod signor;
mod tps;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    mgba::log::init();
    let g = game::Game::new()?;
    g.run();
    Ok(())
}
