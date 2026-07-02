---
name: playtest-multiplayer
description: How to run and manually verify fu's multiplayer end-to-end with two game instances on one machine. Use when asked to run the game, test a networked change, or reproduce a multiplayer bug.
---

# Playtesting multiplayer locally

The game is one binary that is both client and (optionally) in-process server. There is no
headless/server-only mode and no CLI flags — everything is driven through the UI. There are also
no automated tests, so this manual loop is the only end-to-end verification.

## Two-instance loop

1. Build once, then launch two instances (each needs a window/display):
   `cargo run` in two terminals.
2. **Instance A (host)**: Menu → **Create Game** (starts the embedded server on
   `ws://0.0.0.0:8765`; the bottom-right status text confirms it's running) → **Join Game** →
   **Join Local Server** (this button only appears when the in-process server is running; it
   bypasses the socket entirely).
3. **Instance B (client)**: Menu → **Join Game** → type `127.0.0.1:8765` in the address field →
   **Join Online**. The address format is `hostname:port` (the code prepends `ws://`).
4. **Lobby**: on each instance, type a player name, pick a controller from the dropdown, press
   **Join**. Each client can join multiple local players (couch co-op) as long as each uses a
   different controller.
5. Either instance presses **Start Game**. Expect the barrier: both instances switch to the 3D
   scene, run a 3-second countdown, and gameplay only starts once *every* client has finished its
   countdown (server waits for all `PlayersSpawned` before broadcasting `StartRound`).

## Controls & expected behavior

- **Move**: WASD (keyboard) or left stick / d-pad (gamepad). Movement on one instance must
  replicate to the other (velocity relay — small positional drift between instances is a known
  limitation, not a bug).
- **Swing**: gamepad **West** button only — keyboard players currently cannot swing. Testing
  strikes therefore requires at least one gamepad (or temporarily wiring a keyboard swing).
- **Strike**: a swinging blade touching another player kills them on *both* instances: the victim
  shrinks to half size over 0.4 s and thereafter falls through living players (platform-only
  collision layer).
- There is no round-end: after deaths the round just continues. That's expected.

## Gotchas

- Port `8765` is hardcoded; a second Create Game (or a stale process) will panic on bind. Kill
  leftover `fu` processes between runs.
- Only plain `ws://` is supported — no TLS.
- The server keeps roster state for the process lifetime; restart the host to reset the lobby.
- Expect debug `println!` noise (`Here1`, `Got client event`, ...) on the host's terminal — useful
  for confirming events arrive.
