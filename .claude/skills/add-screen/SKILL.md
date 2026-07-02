---
name: add-screen
description: Checklist for adding a new UI screen or app state to fu (menus, settings, score screen, etc.). Use whenever creating a screen, adding an AppState variant, or wiring screen transitions.
---

# Adding a screen

Every screen follows one convention. `src/app/screens/join_game.rs` is the smallest complete
example; `lobby.rs` is the most feature-dense (dropdown, live roster refresh).

## Checklist

1. **State variant** — add it to `AppState` in `src/app/screens/app_state.rs`
   (`Menu → JoinGame → Lobby → SpawningPlayers → Playing`).
2. **Module** — create `src/app/screens/<name>.rs` and declare it in `src/app/screens/mod.rs`.
3. **Marker components** — `XxxRoot` on the UI tree root, `XxxCamera` on the camera, plus
   per-widget markers (`XxxButton`, tuple structs for option data like
   `ControllerOption(pub Controller)`).
4. **`setup_xxx`** on `OnEnter(AppState::Xxx)` — spawn a `Camera2d` (UI screens; `Camera3d` only
   for gameplay) tagged `XxxCamera`, and a full-screen `Node` tree tagged `XxxRoot`. UI is plain
   Bevy `Node`/`Text`/`Button` with inline `Color::srgb` values and the default font — no assets.
5. **Update systems** gated with `.run_if(in_state(AppState::Xxx))`. Free-function systems only —
   no observers, no per-screen Plugins.
6. **`cleanup_xxx`** on `OnExit(AppState::Xxx)` — despawn everything matching `XxxRoot` and
   `XxxCamera`.
7. **Register everything** in `src/app/mod.rs::run()` — the single source of truth. Follow the
   existing grouping: `OnEnter`, then the `Update` tuple, then `OnExit`.

## Common pieces to reuse

- **Text input**: `src/app/common/text.rs` provides `InputField`/`InputText` and the
  `focus_input_field` + `update_input` systems — add those two systems to your state's `Update`
  tuple (see how `JoinGame` and `Lobby` register them).
- **Transitions**: set `ResMut<NextState<AppState>>` from a button-`Interaction` system, or from
  a server-event drain (e.g. `update_lobby` transitions on `ServerEvent::SpawnPlayers`).
- **Passing data between screens**: insert a resource before transitioning, consume it in the
  next screen's setup (precedent: `PendingSpawns` inserted in `lobby.rs`, consumed by
  `setup_game_play`).
- **Talking to the server**: read the `GameClientWrapper` resource; send via `client.sender`,
  receive by draining `client.received_events` (lock, clone, replace with `vec![]`) once per
  frame.
