---
name: add-networked-feature
description: Checklist for adding any new networked gameplay action or event to fu (new player ability, pickup, round logic, anything that must replicate across clients). Use whenever a change needs a new ClientEvent/ServerEvent or must be visible to other players.
---

# Adding a networked feature

Every networked action in this game follows the same six-touchpoint path. The boomerang strike
(commits `4e52e91` → `4f2b64e`) is the reference implementation — mirror it.

## The six touchpoints

1. **Wire protocol — `src/server.rs`**: add variants to `ClientEvent` (client → server) and/or
   `ServerEvent` (server → client). Both enums are `#[derive(Event, Debug, Clone, Serialize,
   Deserialize)]` and serialize as JSON text frames. `server.rs` is the single shared protocol
   module — both sides import these types, so the schema can't drift. Give each variant a doc
   comment like the existing ones.

2. **Server handler — `GameServer::start_server()` in `src/server.rs`**: add a match arm for the
   new `ClientEvent`. The server is a relay/arbiter, not a simulator: validate/dedupe against its
   authoritative state (`players`, `phase`), then broadcast the resulting `ServerEvent` to all
   client senders in `self.clients`. Precedent: `StrikePlayer` only relays `PlayerStriked` on the
   alive→dead transition, collapsing duplicate reports from multiple clients.

3. **Client receive — `drain_server_events` in `src/app/screens/game_play.rs`** (or `update_lobby`
   in `lobby.rs` for lobby-phase events): add a match arm applying the `ServerEvent` to the local
   ECS world. Events arrive via `GameClientWrapper` → `client.received_events` (locked `Vec`,
   drained and cleared once per frame).

4. **Gameplay — `src/app/screens/game_play.rs`**: new marker/data components, tunable `const`s at
   the top of the file (doc-comment the math/why, matching the existing style), and the systems
   that detect input or collisions and *send* the new `ClientEvent` via
   `client.sender.send(...)`.

5. **Spawning** (if the feature is an entity): spawn it in `setup_game_play`. Follow the existing
   hierarchy pattern — the boomerang is a child of the player body with its blade colliders as
   grandchildren, and several systems walk that fixed depth-2 hierarchy directly.

6. **Registration — `src/app/mod.rs::run()`**: register every new system in the tuple for the
   right state, e.g. `.run_if(in_state(AppState::Playing))`. Nothing is auto-registered; there
   are no per-screen Plugins or observers.

## Rules that keep the netcode coherent

- **Ownership rule**: only the client that owns the acting player reports the action (check
  `client.players` for whether the striker/actor is local — see `detect_strikes`). The server
  arbitrates and dedupes; all clients apply the result on receipt.
- **Throttle high-frequency events**: continuous input is only sent on change or on a heartbeat
  (`DIRECTION_EVENT_INTERVAL` = 50 ms) with quantization (`DIRECTION_QUANTIZATION`). Do the same
  for any new continuous signal.
- Movement replicates **velocity intents, not positions** — each client simulates physics
  independently. Anything that must not drift needs an authoritative server event, not physics.
- Remember the three player types: `server::Player` (wire), `game_play::Player` (ECS component),
  `client::ClientPlayer` (locally-owned). Wire messages use `player_id: u8` / `client_id: u8` —
  two separate id spaces.
