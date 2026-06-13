use bevy::prelude::*;
use std::{sync::{Arc, Mutex, mpsc::{self, Receiver, Sender}}, thread::{self, JoinHandle}};

/// Events originating from the server, sent out to clients.
#[derive(Event, Debug, Clone)]
pub enum ServerEvent {
    Movement { player_id: u8, x: f32, y: f32 },
}

/// Events originating from a client, sent to the server.
#[derive(Event, Debug, Clone)]
pub enum ClientEvent {
    Movement { player_id: u8, x: f32, y: f32 },
}

/// Server-side hub: maintains game state
pub struct GameServer {
    /// One sender per connected client, used to push events out to each client.
    pub clients: Arc<Mutex<Vec<Sender<ServerEvent>>>>,
    /// Channel of inbound events arriving from clients.
    pub receiver: Arc<Mutex<Receiver<ClientEvent>>>,
}

impl GameServer {
    pub fn new() -> (Self, Sender<ClientEvent>) {
        let (sender, receiver) = mpsc::channel();
        let server = GameServer {
            clients: Arc::new(Mutex::new(Vec::new())),
            receiver: Arc::new(Mutex::new(receiver)),
        };
        (server, sender)
    }

    pub fn attach_sender(&mut self, sender: Sender<ServerEvent>) {
        self.clients.lock().unwrap().push(sender);
    }

    pub fn start_server(&mut self) -> JoinHandle<()> {
        let receiver = self.receiver.clone();
        let clients = self.clients.clone();
        thread::spawn(move || {
            let receiver = receiver.lock().unwrap();
            loop {
                let event = receiver.recv().unwrap();
                let clients = clients.lock().unwrap();
                match event {
                    ClientEvent::Movement{player_id, x, y} => {
                        for client in clients.iter() {
                            let _ = client.send(ServerEvent::Movement { player_id, x, y });
                        }
                    }
                }
            }
        })
    }
}