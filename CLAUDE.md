# Project Structure
- app: This module should contain the code to manage the application such as the application state and settings
- client: This module should contain the code to manage the game entities, detecting events and updating the game state on screen
- server: This module should contain the code to manage the game state, such as score and player state.

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