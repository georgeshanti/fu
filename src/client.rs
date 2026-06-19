use crate::server::{ClientEvent, Controller, ServerEvent};
use bevy::prelude::*;
use std::{sync::{Arc, Mutex, RwLock, mpsc::{self, Receiver, Sender}}, thread::{self, JoinHandle}};

pub struct ClientPlayer {
    pub name: String,
    pub id: u8,
    pub controller: Controller,
}

/// Client-side handle: handle local game UI state and capture player input events
pub struct GameClient {
    /// Client id
    pub client_id: Arc<RwLock<Option<u8>>>,
    /// Inbound events arriving from the server.
    pub receiver: Arc<Mutex<Receiver<ServerEvent>>>,
    /// Outbound events sent to the server. `None` until a sender is assigned.
    pub sender: Option<Sender<ClientEvent>>,
    /// Accumulated server events received since last drain.
    pub received_events: Arc<Mutex<Vec<ServerEvent>>>,
    pub players: Arc<RwLock<Vec<ClientPlayer>>>,
}

impl GameClient {
    pub fn new() -> (Self, Sender<ServerEvent>) {
        let (sender, receiver) = mpsc::channel();
        let client = GameClient {
            client_id: Arc::new(RwLock::new(None)),
            receiver: Arc::new(Mutex::new(receiver)),
            sender: None,
            received_events: Arc::new(Mutex::new(Vec::new())),
            players: Arc::new(RwLock::new(vec![])),
        };
        (client, sender)
    }

    pub fn attach_sender(&mut self, sender: Sender<ClientEvent>) {
        self.sender = Some(sender);
    }

    pub fn start_client(&self) -> JoinHandle<()> {
        let receiver = self.receiver.clone();
        let received_events = Arc::clone(&self.received_events);
        let client_id = self.client_id.clone();
        thread::spawn(move || {
            let receiver = receiver.lock().unwrap();
            loop {
                let event = receiver.recv().unwrap();
                match event {
                    ServerEvent::ClientRegistered { client_id: id } => {
                        println!("Got client id: {}", id);
                        *client_id.write().unwrap() = Some(id);
                    },
                    _ => received_events.lock().unwrap().push(event.clone()),
                };
            }
        })
    }
}
