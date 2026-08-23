pub mod screens;
pub mod common;

use std::sync::{Arc, RwLock};

use avian3d::prelude::*;
use bevy::prelude::*;

use crate::app::screens::app_state::*;
use crate::app::common::text::*;
use crate::app::screens::game_menu::*;
use crate::app::screens::game_play::actions::apply_dead_collision_layers;
use crate::app::screens::game_play::actions::drain_server_events;
use crate::app::screens::game_play::actions::move_player;
use crate::app::screens::game_play::actions::release_throw;
use crate::app::screens::game_play::actions::start_jump;
use crate::app::screens::game_play::actions::start_swing;
use crate::app::screens::game_play::actions::start_throwing;
use crate::app::screens::game_play::animations::animate_death;
use crate::app::screens::game_play::animations::animate_swing;
use crate::app::screens::game_play::animations::animate_throwing_action;
use crate::app::screens::game_play::animations::start_throw_animation;
use crate::app::screens::game_play::detect_events::detect_parries;
use crate::app::screens::game_play::detect_events::detect_swing_strikes;
use crate::app::screens::game_play::detect_events::detect_throw_strikes;
use crate::app::screens::game_play::phases::end::animate_round_end;
use crate::app::screens::game_play::phases::end::cleanup_round_ended;
use crate::app::screens::game_play::phases::end::handle_continue_button;
use crate::app::screens::game_play::phases::end::setup_round_ended;
use crate::app::screens::game_play::phases::end::start_round_end_animation;
use crate::app::screens::game_play::phases::end::wait_for_next_round;
use crate::app::screens::game_play::state::record_tick_state;
use crate::app::screens::game_play::world::setup_game_play;
use crate::app::screens::game_play::world::tick_countdown;
use crate::app::screens::game_play::world::wait_for_start;
use crate::app::screens::game_play::*;
use crate::app::screens::join_game::*;
use crate::app::screens::lobby::*;
use crate::{client::GameClient, server::{ClientEvent, ServerEvent}};

#[derive(Resource)]
pub struct GameClientWrapper {
    pub client: Arc<RwLock<GameClient>>,
}

pub fn run() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(PhysicsPlugins::default())
        .init_state::<AppState>()
        .init_resource::<SelectedController>()
        .init_resource::<SelectedColor>()
        .add_systems(OnEnter(AppState::Menu), setup_menu)
        .add_systems(
            Update,
            (handle_join_game_button, handle_create_game_button, update_server_status_text)
                .run_if(in_state(AppState::Menu)),
        )
        .add_systems(OnExit(AppState::Menu), cleanup_menu)
        .add_systems(OnEnter(AppState::JoinGame), setup_join_screen)
        .add_systems(
            Update,
            (
                focus_input_field,
                update_input,
                handle_paste_button,
                handle_join_online_submit_button,
                handle_join_local_server_button,
            )
                .run_if(in_state(AppState::JoinGame)),
        )
        .add_systems(OnExit(AppState::JoinGame), cleanup_join_screen)
        .add_systems(OnEnter(AppState::Lobby), setup_lobby)
        .add_systems(
            Update,
            (
                focus_input_field,
                update_input,
                update_lobby,
                handle_lobby_join_button,
                handle_lobby_start_button,
                populate_controller_options,
                toggle_controller_dropdown,
                handle_controller_option_click,
                handle_color_swatch_click,
                update_join_button_state,
            )
                .run_if(in_state(AppState::Lobby)),
        )
        .add_systems(OnExit(AppState::Lobby), cleanup_lobby)
        .add_systems(OnEnter(AppState::SpawningPlayers), setup_game_play)
        .add_systems(
            Update,
            (tick_countdown, wait_for_start).run_if(in_state(AppState::SpawningPlayers)),
        )
        .add_systems(
            Update,
            (
                start_jump,
                start_swing,
                move_player,
                apply_dead_collision_layers,
                animate_death,
                start_throwing,
                start_throw_animation,
                animate_throwing_action,
                release_throw,
            )
                .run_if(in_state(AppState::Playing)),
        )
        .add_systems(
            PhysicsSchedule,
            (
                detect_throw_strikes.before(detect_parries),
                detect_parries.before(animate_swing),
                animate_swing.before(detect_swing_strikes),
                detect_swing_strikes.before(PhysicsStepSystems::First),
                record_tick_state.after(PhysicsStepSystems::Last),
            )
                .run_if(in_state(AppState::Playing)),
        )
        .add_systems(
            FixedUpdate,
            (drain_server_events).run_if(in_state(AppState::Playing))
        )
        .add_systems(OnEnter(AppState::RoundEndAnimation), start_round_end_animation)
        .add_systems(Update, animate_round_end.run_if(in_state(AppState::RoundEndAnimation)))
        .add_systems(OnEnter(AppState::RoundEnded), setup_round_ended)
        .add_systems(
            Update,
            (handle_continue_button, wait_for_next_round).run_if(in_state(AppState::RoundEnded)),
        )
        .add_systems(OnExit(AppState::RoundEnded), cleanup_round_ended)
        .run();
}
