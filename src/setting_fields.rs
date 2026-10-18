//! Reusable setting fields for host applications.
//!
//! GPUI Component's stock input is text-only. Hosts that store filesystem
//! paths need a picker beside the input, so this module supplies one field for
//! directories and one for files, plus a slider for token budgets.

use std::rc::Rc;

use gpui_kit::{
    App, AppContext as _, ElementId, Entity, ParentElement as _, PathPromptOptions, SharedString,
    Styled as _, Subscription,
    component::{
        Disableable as _, Sizable as _,
        button::Button,
        h_flex,
        input::{Input, InputEvent, InputState},
        setting::SettingField,
        slider::{Slider, SliderEvent, SliderState},
    },
    px,
};

struct PathInputState {
    input: Entity<InputState>,
    _subscription: Subscription,
}

type PathSetter = Rc<dyn Fn(String, &mut App)>;
type TokenSetter = Rc<dyn Fn(usize, &mut App)>;

/// A path field with a **Browse…** button.
///
/// `directories` and `files` are passed through to [`PathPromptOptions`].
/// `value` reads the current path from the application, `set_value` writes the
/// chosen or typed path back. The input is keyed by its setting position, so
/// rebuilding the page on each frame keeps the same editor state.
pub fn path_setting<G, S>(
    key: &'static str,
    prompt: &'static str,
    directories: bool,
    files: bool,
    value: G,
    set_value: S,
) -> SettingField<SharedString>
where
    G: Fn(&App) -> String + 'static,
    S: Fn(String, &mut App) + 'static,
{
    let setter: PathSetter = Rc::new(set_value);
    SettingField::render(move |options, window, cx| {
        let current: SharedString = value(cx).into();
        let keyed = SharedString::from(format!(
            "path-field-{key}-{}-{}-{}",
            options.page_ix(),
            options.group_ix(),
            options.item_ix()
        ));
        let setter_for_input = setter.clone();
        let state = window.use_keyed_state(keyed.clone(), cx, {
            let current = current.clone();
            move |window, cx| {
                let input = cx.new(|cx| InputState::new(window, cx).default_value(current.clone()));
                let subscription = cx.subscribe(&input, move |_, input, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        setter_for_input(input.read(cx).value().trim().to_owned(), cx);
                    }
                });
                PathInputState {
                    input,
                    _subscription: subscription,
                }
            }
        });

        let input = state.read(cx).input.clone();
        if input.read(cx).value() != current {
            input.update(cx, |input, cx| {
                input.set_value(current, window, cx);
            });
        }

        let browse_id = ElementId::Name(SharedString::from(format!("{keyed}-browse")));
        let setter_for_browse = setter.clone();
        let browse = {
            let input = input.clone();
            Button::new(browse_id)
                .label("Browse…")
                .small()
                .outline()
                .disabled(options.is_disabled())
                .on_click(move |_, window, cx| {
                    let receiver = cx.prompt_for_paths(PathPromptOptions {
                        files,
                        directories,
                        multiple: false,
                        prompt: Some(prompt.into()),
                    });
                    let input = input.clone();
                    let setter = setter_for_browse.clone();
                    window
                        .spawn(cx, async move |cx| {
                            let Ok(Ok(Some(paths))) = receiver.await else {
                                return;
                            };
                            let Some(path) = paths.into_iter().next() else {
                                return;
                            };
                            let value = path.display().to_string();
                            let _ = input.update_in(cx, |input, window, cx| {
                                input.set_value(value.clone(), window, cx);
                            });
                            let _ = cx.update(|_window, cx| setter(value, cx));
                        })
                        .detach();
                })
        };

        h_flex()
            .w(px(420.))
            .gap_2()
            .items_center()
            .child(
                Input::new(&input)
                    .disabled(options.is_disabled())
                    .with_size(options.size())
                    .flex_1(),
            )
            .child(browse)
    })
}

/// A slider over powers of two.
///
/// `min_exponent` and `max_exponent` are base-2 exponents; the field shows
/// `2^min ..= 2^max` (for example 512, 1024, 2048, …). The value is written
/// back as a token count, so the stored setting is always an exact power.
pub fn token_slider<G, S>(
    key: &'static str,
    min_exponent: u32,
    max_exponent: u32,
    value: G,
    set_value: S,
) -> SettingField<SharedString>
where
    G: Fn(&App) -> usize + 'static,
    S: Fn(usize, &mut App) + 'static,
{
    let setter: TokenSetter = Rc::new(set_value);
    let min = min_exponent as f32;
    let max = max_exponent as f32;
    SettingField::render(move |options, window, cx| {
        let tokens = value(cx);
        let exponent = exponent_for(tokens, min_exponent, max_exponent);
        let keyed = SharedString::from(format!(
            "token-slider-{key}-{}-{}-{}",
            options.page_ix(),
            options.group_ix(),
            options.item_ix()
        ));
        let setter_for_slider = setter.clone();
        let state = window.use_keyed_state(keyed, cx, move |_, cx| {
            let slider = cx.new(|_| {
                SliderState::new()
                    .min(min)
                    .max(max)
                    .step(1.0)
                    .default_value(exponent as f32)
            });
            let subscription = cx.subscribe(&slider, move |_, _, event: &SliderEvent, cx| {
                let exponent = match event {
                    SliderEvent::Change(value) | SliderEvent::Release(value) => {
                        value.end().round() as u32
                    }
                };
                let tokens = 1usize << exponent;
                setter_for_slider(tokens, cx);
                cx.notify();
            });
            SliderFieldState {
                slider,
                _subscription: subscription,
            }
        });
        let slider = state.read(cx).slider.clone();
        // Keep the thumb honest when a profile or the agent changes the value.
        let expected = exponent as f32;
        if (slider.read(cx).value().end() - expected).abs() > f32::EPSILON {
            slider.update(cx, |slider, cx| {
                slider.set_value(expected, window, cx);
            });
        }
        let shown = 1usize << exponent;
        h_flex()
            .w(px(280.))
            .gap_3()
            .items_center()
            .child(
                Slider::new(&slider)
                    .disabled(options.is_disabled())
                    .flex_1(),
            )
            .child(
                gpui_kit::div()
                    .w(px(96.))
                    .text_sm()
                    .child(format_tokens(shown)),
            )
    })
}

struct SliderFieldState {
    slider: Entity<SliderState>,
    _subscription: Subscription,
}

fn exponent_for(tokens: usize, min: u32, max: u32) -> u32 {
    let clamped = tokens.max(1);
    let exponent = usize::BITS - 1 - clamped.leading_zeros();
    // Round to the nearest power of two rather than always down.
    let lower = 1usize << exponent;
    let upper = lower.saturating_mul(2);
    let rounded = if clamped.saturating_sub(lower) > upper.saturating_sub(clamped) {
        exponent + 1
    } else {
        exponent
    };
    rounded.clamp(min, max)
}

fn format_tokens(tokens: usize) -> String {
    let digits = tokens.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_values_snap_to_powers_of_two() {
        assert_eq!(exponent_for(512, 9, 20), 9);
        assert_eq!(exponent_for(700, 9, 20), 9);
        assert_eq!(exponent_for(900, 9, 20), 10);
        assert_eq!(exponent_for(262_144, 9, 20), 18);
        assert_eq!(exponent_for(1, 9, 20), 9);
        assert_eq!(exponent_for(usize::MAX, 9, 20), 20);
    }

    #[test]
    fn token_labels_use_thousands_separators() {
        assert_eq!(format_tokens(512), "512");
        assert_eq!(format_tokens(1_048_576), "1,048,576");
    }

    #[test]
    fn a_non_power_value_rounds_to_the_nearest_step() {
        assert_eq!(exponent_for(1_500, 9, 20), 10);
        assert_eq!(exponent_for(1_800, 9, 20), 11);
    }
}
