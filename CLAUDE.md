# fu

A multiplayer 3D party game built on **Bevy 0.18** + **Avian3d 0.6** (physics). Players are
cylinders on a floating platform, each wielding an L-shaped boomerang blade they can swing to
strike other players. Struck players die (shrink, drop to a platform-only collision layer).
Multiplayer is a hand-rolled WebSocket layer (`tungstenite` + JSON via `serde_json`) — no netcode
library. Rust edition 2024, single binary `fu`.

## Commands

- Build: `cargo build` · Run: `cargo run`
- There are **no tests** in this codebase; `cargo test` is a no-op.
- Linux system deps (Bevy): `libasound2-dev`, `libudev-dev`.
- CI (`.github/workflows/build.yml`) is manual-dispatch only: release builds for linux-x86 and macos-arm64.
- `[profile.dev]` is already tuned Bevy-style (crate at `opt-level = 1`, deps at `3`) — leave it alone.
- Dependency API lookup: grep `.claude/deps-index.txt` (generate it first if missing — see the
  `lookup-dep-api` skill). **Never read dependency sources in `target/` or `~/.cargo`.**

## Architecture

One binary that is always a client and can also host an **in-process server** (started from the
menu's "Create Game" button; binds `ws://0.0.0.0:8765`, hardcoded, no TLS).

| File | Role |
|---|---|
| `src/main.rs` | Entry point → `app::run()` |
| `src/app/mod.rs` | **All system registration lives in `run()`** — manual `OnEnter`/`OnExit`/`Update + run_if(in_state(...))`. No per-screen Plugins, no observers. Also defines the `GameClientWrapper` resource. |
| `src/app/screens/app_state.rs` | Client state machine `AppState`: `Menu → JoinGame → Lobby → SpawningPlayers → Playing` |
| `src/app/screens/{game_menu,join_game,lobby,game_play}.rs` | One module per screen. `game_play.rs` holds *all* 3D gameplay: components, tunable constants, movement, swing/strike, death. |
| `src/app/common/text.rs` | Reusable text-input widget (`InputField`, `InputText`, `focus_input_field`, `update_input`) |
| `src/server.rs` | `GameServer` (threaded event-loop relay) **and the shared wire protocol**: `ServerEvent`, `ClientEvent`, `Player`, `Controller`. Globals `GAME_SERVER` / `CLIENT_EVENT_SENDER` back the in-process server. |
| `src/client.rs` | `GameClient`: inbound events buffered in `received_events`, drained per-frame by Bevy systems |
| `src/connection/{server,client}.rs` | Generic WebSocket transport (`create_server::<Req,Resp,Id>` / `create_client`), `Handshake<Id>` as first frame, JSON text frames |

### Sync model

The server is a **relay/arbiter, not a simulator** — it holds the roster, phase, and liveness, but
runs no physics. Every client runs its own full Avian simulation:

- **Movement**: client reads input → sends `ClientEvent::Movement` (velocity intent, throttled to
  direction changes or a 50 ms heartbeat, gamepad axes quantized) → server rebroadcasts → each
  client applies velocity + `ConstantLinearAcceleration` locally. No position reconciliation, so
  clients can drift.
- **Strikes**: only the client that *owns* the striking player sends `StrikePlayer`; the server
  dedupes (relays `PlayerStriked` only on the alive→dead transition).
- **Round start is a barrier**: server broadcasts `SpawnPlayers` → each client builds the scene,
  runs a 3 s countdown, sends `PlayersSpawned` → once all clients report in, server broadcasts
  `StartRound`.

One client can own several players (couch co-op): each lobby join picks a `Controller`
(`Keyboard` or `Gamepad(id)`).

## Conventions

- **Screen pattern** (every screen follows it): marker components `XxxRoot` + `XxxCamera`,
  `setup_xxx` on `OnEnter`, state-gated `Update` systems, `cleanup_xxx` on `OnExit` despawning by
  the markers, all registered in `src/app/mod.rs::run()`.
- **Tunable constants** are `const`s at the top of `game_play.rs` with doc comments explaining the
  *why*/math (e.g. `PLAYER_ACCEL = 4.905 = μg`). Follow that heavily-commented style.
- **Coordinates**: forward = −Z, right = +X, up = +Y. Facing yaw = `atan2(-x, -z)`; player bodies
  use `LockedAxes::ROTATION_LOCKED` and are rotated manually.
- **Threading**: cross-thread state is `Arc<Mutex/RwLock>`; the Bevy⇄network boundary is always an
  mpsc channel drained once per frame (see `drain_server_events` / `update_lobby`).
- **No assets**: all visuals are code-generated primitive meshes + `StandardMaterial`; UI is plain
  Bevy `Node`/`Text`/`Button`. There is no `AssetServer` plumbing.

## Gotchas

- Three distinct "player" types — don't confuse them:
  `server::Player` (wire/roster entry), `game_play::Player` (ECS component on the physics body),
  `client::ClientPlayer` (this client's locally-owned players).
- `client_id` and player `id` are **separate id spaces**: client ids come from a per-connection
  counter; player ids are assigned as `players.len()` on `JoinLobby`.
- Client `AppState` ≠ server `GamePhase` (`Lobby/RoundStarting/RoundPlaying/RoundEnded`).
  `RoundEnded` is defined but never set — there is no win/round-reset flow yet.
- Swing input: `GamepadButton::West` for gamepad players, `KeyCode::KeyZ` for keyboard.
  Keyboard movement uses the arrow keys (not WASD).
- Known cruft, intentionally untouched: debug `println!`s (`"Here1"`, `"Got client event"`, …),
  unused imports in `main.rs`, the `extract-cargo-expoorts.sh` filename typo.

## Skills

Project skills in `.claude/skills/`:

- **add-networked-feature** — the six-touchpoint checklist for any new networked gameplay action
- **add-screen** — checklist for adding a UI screen / `AppState`
- **playtest-multiplayer** — how to run two instances and verify multiplayer end-to-end
- **lookup-dep-api** — find Bevy/Avian APIs via `.claude/deps-index.txt` instead of reading dep sources
