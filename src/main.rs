use std::sync::{Arc, RwLock};

use crate::{app::GameClientWrapper, client::GameClient, server::GameServer};

mod app;
mod client;
mod connection;
mod server;

fn main() {
    if std::env::args().any(|arg| arg == "--server") {
        server::run_dedicated_server();
    } else {
        app::run();
    }
}
