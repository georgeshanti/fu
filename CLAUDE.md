# Project summary
It is a 3D game built in Rust using on Bevy and Avian 3D.

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
- client: This module should contain the code to manage the game entities, detecting events and updating the game state on screen
- server: This module should contain the code to manage the game state, such as score and player state.
- connection: This module provides network capabilities for the game
  - server: Provides a function that starts a server(websocket/tcp/udp/whatever) and returns abstracted mpsc Sender and Receiver channels
  - client: Provides a function that starts opens a connection to a server(websocket/tcp/udp/whatever) and returns abstracted mpsc Sender and Receiver channels

# Gameplay
Players spawn with knives in their hands and must hunt each other down and kill them.

# General working
- Spawn app
- Spawn game server
- Spawn game client
- Attach game client to server
- Start game at server
- Capture player events at client and send to server
- Process player events at server and determine game events and send to client
- Process game events at client and update Game UI

# Note
- Do not use `cargo build` to check for errors, instead use the IDE's native error reporting to check if there are any errors.