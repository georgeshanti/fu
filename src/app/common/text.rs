use bevy::{input::keyboard::{Key, KeyboardInput}, prelude::*};



/// Holds the text currently typed into the join-game input field.
#[derive(Component, Default)]
pub struct InputField {
    value: String,
}

impl InputField {
    /// The text currently entered into this field.
    pub fn value(&self) -> &str {
        &self.value
    }
}

/// Marks the `Text` entity that displays a join input field's contents.
#[derive(Component)]
pub struct InputText;

/// Marks the input field that currently has keyboard focus (at most one).
#[derive(Component)]
pub struct TextInputFocused;

/// System-local blink state for the join input caret.
#[derive(Default)]
pub struct CursorBlink {
    /// Time remaining until the next visibility toggle.
    timer: f32,
    /// Whether the caret is currently shown.
    visible: bool,
}

/// How long (seconds) the input caret stays visible / hidden between blinks.
const CURSOR_BLINK_INTERVAL: f32 = 0.5;

/// Appends typed characters (and handles Backspace) into the join input field,
/// keeping the displayed `Text` in sync.
pub fn update_input(
    time: Res<Time>,
    mut key_events: MessageReader<KeyboardInput>,
    mut blink: Local<CursorBlink>,
    mut fields: Query<(&mut InputField, &Children, Has<TextInputFocused>)>,
    mut texts: Query<&mut Text, With<InputText>>,
) {
    // Collect this frame's edits; they apply only to the focused field.
    let mut typed = String::new();
    let mut backspaces = 0u32;
    let mut edited = false;
    for event in key_events.read() {
        if !event.state.is_pressed() {
            continue;
        }
        match &event.logical_key {
            Key::Character(s) => {
                typed.push_str(s.as_str());
                edited = true;
            }
            Key::Space => {
                typed.push(' ');
                edited = true;
            }
            Key::Backspace => {
                backspaces += 1;
                edited = true;
            }
            _ => {}
        }
    }

    // Advance the blink. Editing forces the caret visible so typing feels responsive.
    blink.timer -= time.delta_secs();
    if edited {
        blink.visible = true;
        blink.timer = CURSOR_BLINK_INTERVAL;
    } else if blink.timer <= 0.0 {
        blink.visible = !blink.visible;
        blink.timer = CURSOR_BLINK_INTERVAL;
    }

    // Apply edits to the focused field, then re-render every field's display so
    // the unfocused fields drop their caret.
    for (mut field, children, focused) in &mut fields {
        if focused {
            for _ in 0..backspaces {
                field.value.pop();
            }
            field.value.push_str(&typed);
        }
        let caret = if focused && blink.visible { "|" } else { "" };
        let display = format!("{}{}", field.value, caret);
        for child in children.iter() {
            if let Ok(mut text) = texts.get_mut(child) {
                if text.0 != display {
                    text.0 = display.clone();
                }
            }
        }
    }
}

/// Moves keyboard focus to whichever input field was last clicked.
pub fn focus_input_field(
    mut commands: Commands,
    clicked: Query<(Entity, &Interaction), (Changed<Interaction>, With<InputField>)>,
    focused: Query<Entity, With<TextInputFocused>>,
) {
    for (entity, interaction) in &clicked {
        if *interaction == Interaction::Pressed {
            for prev in &focused {
                commands.entity(prev).remove::<TextInputFocused>();
            }
            commands.entity(entity).insert(TextInputFocused);
        }
    }
}