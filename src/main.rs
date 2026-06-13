use std::sync::{Arc, RwLock};

use crate::{app::GameClientWrapper, client::GameClient, server::GameServer};

mod app;
mod client;
mod server;

fn main() {
    let (mut game_server, client_event_sender) = GameServer::new();
    let (game_client, server_event_sender) = GameClient::new();
    let client = GameClientWrapper{client: Arc::new(RwLock::new(game_client))};

    client.client.write().unwrap().attach_sender(client_event_sender);
    game_server.attach_sender(server_event_sender);

    let client_thread = client.client.read().unwrap().start_client();
    let server_thread = game_server.start_server();

    app::run(client);

    server_thread.join().unwrap();
    client_thread.join().unwrap();
}
