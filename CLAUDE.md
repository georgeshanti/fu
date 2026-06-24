# Project summary
It is a 3D game built in Rust using on Bevy as the game engine and Avian 3D as the physics engine.

# Gameplay
Players spawn on a map, with boomerangs in their hands and chase each other down to slash them with the boomerang until only one remains.

# Application Architecture
The application can be split up into 3 layers:
 - The Bevy App: This manages the UI, game engine and physics engine, and is the part of the application that the user interacts with. All keyboard and mouse events are capture in this layer and all UI change is carried out by this layer. In a multi-player game, there might be multiple Bevy Apps running on different computers.
 - The GameServer: This manages the state of a game, the state of the players that are currently playing, what characteristics they have, what abilities they have, the state of the map, what is present and where, and the state of the scoreboard, which players have what score and who wins.
 - The GameClient: This manages the connection between the bevy application and the GameServer. A single GameServer could be connected to multiple Bevy Apps running on different computers and connected over some network. All user events captured by the bevy app is sent to the game server through the GameClient and all server events to propgate game change is sent from the game server to the bevy application through the GameClient  

# Project Structure
- app: This module should contain the code to manage the application such as the application state and settings
  - screens: Contains the different screens the App displays to the user
    - game_menu: Options to either join or create a game
    - join_game: Options to enter game server details and join a game
    - create_game: Options to create a game server
    - lobby: Screen to wait before the game server starts the game
    - game_state: Enum to describe the various game states
  - common: Common ui elements used across the application
    - text: Contains text input field component
- client: This module should contain the code to manage the connection to the game server, forwarding events to the game server and storing incoming events in a buffer that can be fetched by the game client when required.
- server: This module should contain the code to manage the game state, and player state, map state and scoreboard.
- connection: This module provides network capabilities for the game
  - server: Provides a function that starts a server(websocket/tcp/udp/whatever) and returns abstracted mpsc Sender and Receiver channels
  - client: Provides a function that starts opens a connection to a server(websocket/tcp/udp/whatever) and returns abstracted mpsc Sender and Receiver channels

# General working

## Game Setup
- Application opens with open to start a GameServer or join a GameServer by creating a GameClient and connecting the GameClient to a running GameServer
- There is a global vairable for a single running GameServer, which is created once whenever the user chooses to start, and it also starts a network listener to accept connections from external GameClient that may want to Join
- On Joining a GameServer, either through a network connection or directly attaching itself to the GameServer in the global variable in the running process, it eneter a lobby where players can join the GameServer through the GameClient
- Once all players have joined, any client can start the game

## Round
- All players spawn with their boomerangs, they can move around with the keyboard or joystick and slash at the other players with their boomrangs and eliminate them.
- Last man standing wins.

# Note
- Do not use `cargo build` to check for errors, instead use the IDE's native error reporting to check if there are any errors.