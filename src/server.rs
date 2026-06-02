use bevy::prelude::*;
use std::sync::mpsc::{self, Receiver, Sender};

/// Events originating from the server, sent out to clients.
#[derive(Event, Debug, Clone)]
pub enum ServerEvent {
    Movement,
}

/// Events originating from a client, sent to the server.
#[derive(Event, Debug, Clone)]
pub enum ClientEvent {
    Movement,
}

/// Server-side hub: maintains game state
pub struct GameServer {
    /// One sender per connected client, used to push events out to each client.
    pub clients: Vec<Sender<ServerEvent>>,
    /// Channel of inbound events arriving from clients.
    pub receiver: Receiver<ClientEvent>,
}

impl GameServer {
    fn new() -> (Self, Sender<ClientEvent>) {
        let (sender, receiver) = mpsc::channel();
        let server = GameServer {
            clients: Vec::new(),
            receiver,
        };
        (server, sender)
    }
}