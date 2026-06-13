use std::sync::{Arc, RwLock};

use crate::{app::GameClientWrapper, client::GameClient, server::GameServer};

mod app;
mod client;
mod connection;
mod server;

fn main() {
    app::run();
}
