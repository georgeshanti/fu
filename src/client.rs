use crate::server::{ClientEvent, ServerEvent};
use std::sync::mpsc::{self, Receiver, Sender};

/// Client-side handle: handle local game UI state and capture player input events
pub struct GameClient {
    /// Inbound events arriving from the server.
    pub receiver: Receiver<ServerEvent>,
    /// Outbound events sent to the server. `None` until a sender is assigned.
    pub sender: Option<Sender<ClientEvent>>,
}

impl GameClient {
    fn new() -> (Self, Sender<ServerEvent>) {
        let (sender, receiver) = mpsc::channel();
        let client = GameClient {
            receiver,
            sender: None,
        };
        (client, sender)
    }

    fn attach_sender(&mut self, sender: Sender<ClientEvent>) {
        self.sender = Some(sender);
    }
}
