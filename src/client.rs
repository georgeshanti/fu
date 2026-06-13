use crate::server::{ClientEvent, ServerEvent};
use bevy::prelude::*;
use std::{sync::{Arc, Mutex, mpsc::{self, Receiver, Sender}}, thread::{self, JoinHandle}};

/// Client-side handle: handle local game UI state and capture player input events
pub struct GameClient {
    /// Inbound events arriving from the server.
    pub receiver: Arc<Mutex<Receiver<ServerEvent>>>,
    /// Outbound events sent to the server. `None` until a sender is assigned.
    pub sender: Option<Sender<ClientEvent>>,
    /// Accumulated server events received since last drain.
    pub received_events: Arc<Mutex<Vec<ServerEvent>>>,
}

impl GameClient {
    pub fn new() -> (Self, Sender<ServerEvent>) {
        let (sender, receiver) = mpsc::channel();
        let client = GameClient {
            receiver: Arc::new(Mutex::new(receiver)),
            sender: None,
            received_events: Arc::new(Mutex::new(Vec::new())),
        };
        (client, sender)
    }

    pub fn attach_sender(&mut self, sender: Sender<ClientEvent>) {
        self.sender = Some(sender);
    }

    pub fn start_client(&self) -> JoinHandle<()> {
        let receiver = self.receiver.clone();
        let received_events = Arc::clone(&self.received_events);
        thread::spawn(move || {
            let receiver = receiver.lock().unwrap();
            loop {
                let event = receiver.recv().unwrap();
                received_events.lock().unwrap().push(event.clone());
            }
        })
    }
}
