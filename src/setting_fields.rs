//! Reusable setting fields for host applications.
//!
//! GPUI Component's stock input is text-only. Hosts that store filesystem
//! paths need a picker beside the input, so this module supplies one field for
//! directories and one for files, plus a slider for token budgets.
//!
//! The stock [`SettingItem`](gpui_kit::component::setting::SettingItem) cannot
//! style its title, so unsaved settings use [`SettingRow`], which draws the
//! title itself and marks it while it differs from the applied configuration.

use std::rc::Rc;

use gpui_kit::{
    Anchor, AnyElement, App, AppContext as _, Axis, ElementId, Entity, FontWeight, Hsla,
    IntoElement, ParentElement as _, PathPromptOptions, SharedString, Styled as _, Subscription,
    Window,
    component::{
        ActiveTheme as _, AxisExt as _, Disableable as _, Sizable as _,
        button::Button,
        h_flex,
        input::{Input, InputEvent, InputState, NumberInput},
        menu::{DropdownMenu, PopupMenuItem},
        setting::{NumberFieldOptions, RenderOptions, SettingField, SettingItem},
        slider::{Slider, SliderEvent, SliderState},
        switch::Switch,
        v_flex,
    },
    hsla,
    prelude::FluentBuilder as _,
    px,
};

struct PathInputState {
    input: Entity<InputState>,
    _subscription: Subscription,
}

type PathSetter = Rc<dyn Fn(String, &mut App)>;
type TokenSetter = Rc<dyn Fn(usize, &mut App)>;
type TextSetter = Rc<dyn Fn(String, &mut App)>;
type NumberSetter = Rc<dyn Fn(f64, &mut App)>;
type SwitchSetter = Rc<dyn Fn(bool, &mut App)>;

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
    SettingField::render(path_control(
        key,
        prompt,
        directories,
        files,
        value,
        set_value,
    ))
}

/// A path input with a **Browse…** button, for use inside a [`SettingRow`].
pub fn path_control<G, S>(
    key: &'static str,
    prompt: &'static str,
    directories: bool,
    files: bool,
    value: G,
    set_value: S,
) -> Control
where
    G: Fn(&App) -> String + 'static,
    S: Fn(String, &mut App) + 'static,
{
    let setter: PathSetter = Rc::new(set_value);
    control(move |options, window, cx| {
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
            .w(px(340.))
            .gap_2()
            .items_center()
            .child(
                Input::new(&input)
                    .disabled(options.is_disabled())
                    .with_size(options.size())
                    .flex_1(),
            )
            .child(browse)
            .into_any_element()
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
    SettingField::render(token_control(
        key,
        min_exponent,
        max_exponent,
        value,
        set_value,
    ))
}

/// A power-of-two token slider, for use inside a [`SettingRow`].
pub fn token_control<G, S>(
    key: &'static str,
    min_exponent: u32,
    max_exponent: u32,
    value: G,
    set_value: S,
) -> Control
where
    G: Fn(&App) -> usize + 'static,
    S: Fn(usize, &mut App) + 'static,
{
    let setter: TokenSetter = Rc::new(set_value);
    let min = min_exponent as f32;
    let max = max_exponent as f32;
    control(move |options, window, cx| {
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
            .into_any_element()
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

/// A control rendered beside a [`SettingRow`] title.
pub type Control = Box<dyn Fn(&RenderOptions, &mut Window, &mut App) -> AnyElement>;

/// Box a control closure.
pub fn control<E>(render: impl Fn(&RenderOptions, &mut Window, &mut App) -> E + 'static) -> Control
where
    E: IntoElement + 'static,
{
    Box::new(move |options, window, cx| render(options, window, cx).into_any_element())
}

/// The bold violet that marks unsaved settings.
///
/// Light surfaces take a deep violet; dark surfaces take a lighter tint of the
/// same hue so the mark keeps contrast.
pub fn dirty_color(cx: &App) -> Hsla {
    if cx.theme().is_dark() {
        hsla(266.0 / 360.0, 0.62, 0.74, 1.0)
    } else {
        hsla(266.0 / 360.0, 0.55, 0.36, 1.0)
    }
}

/// A titled setting row whose title marks unsaved changes.
///
/// The stock `SettingItem` renders its title through `Label`, which fixes the
/// colour before any caller style applies. A row draws both halves itself, so
/// the title can turn bold violet while the field differs from the applied
/// configuration.
pub struct SettingRow {
    title: SharedString,
    description: Option<SharedString>,
    control: Control,
    is_dirty: Rc<dyn Fn(&App) -> bool>,
    layout: Option<Axis>,
}

impl SettingRow {
    /// Create a row with a title and a control.
    pub fn new(title: impl Into<SharedString>, control: Control) -> Self {
        Self {
            title: title.into(),
            description: None,
            control,
            is_dirty: Rc::new(|_| false),
            layout: None,
        }
    }

    /// Add the muted second line.
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Put the control below the title instead of beside it.
    pub fn vertical(mut self) -> Self {
        self.layout = Some(Axis::Vertical);
        self
    }

    /// Mark the title when this predicate reports an unsaved change.
    pub fn dirty<F>(mut self, is_dirty: F) -> Self
    where
        F: Fn(&App) -> bool + 'static,
    {
        self.is_dirty = Rc::new(is_dirty);
        self
    }

    /// Build the item GPUI Component's settings page expects.
    pub fn into_item(self) -> SettingItem {
        let title = self.title;
        let keywords = [title.clone()];
        let description = self.description;
        let control = self.control;
        let is_dirty = self.is_dirty;
        let layout = self.layout;
        SettingItem::render(move |options, window, cx| {
            let dirty = is_dirty(cx);
            let layout = layout.unwrap_or(options.layout());
            let title = gpui_kit::div()
                .text_sm()
                .when(dirty, |this| {
                    this.font_weight(FontWeight::BOLD)
                        .text_color(dirty_color(cx))
                })
                .child(title.clone());
            let text = v_flex()
                .map(|this| {
                    if layout.is_horizontal() {
                        this.flex_1().max_w(gpui_kit::relative(0.6))
                    } else {
                        this.w_full()
                    }
                })
                .child(title)
                .when_some(description.clone(), |this, description| {
                    this.child(
                        gpui_kit::div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(description),
                    )
                });
            let control = (control)(&options.with_layout(layout), window, cx);
            if layout.is_horizontal() {
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .gap_3()
                    .child(text)
                    .child(control)
                    .into_any_element()
            } else {
                v_flex()
                    .w_full()
                    .gap_3()
                    .child(text)
                    .child(control)
                    .into_any_element()
            }
        })
        .keywords(keywords)
    }
}

/// The style refinement that marks a page header as unsaved.
pub fn dirty_header(cx: &App) -> gpui_kit::StyleRefinement {
    let mut style = gpui_kit::StyleRefinement::default();
    style.text.color = Some(dirty_color(cx));
    style.text.font_weight = Some(FontWeight::BOLD);
    style
}

struct TextControlState {
    input: Entity<InputState>,
    _subscription: Subscription,
}

/// A one-line text control.
pub fn text_control<V, W>(key: &'static str, value: V, write: W) -> Control
where
    V: Fn(&App) -> String + 'static,
    W: Fn(String, &mut App) + 'static,
{
    let write: TextSetter = Rc::new(write);
    let key = SharedString::from(key);
    control(move |options, window, cx| {
        let current: SharedString = value(cx).into();
        let keyed = SharedString::from(format!(
            "row-input-{key}-{}-{}-{}",
            options.page_ix(),
            options.group_ix(),
            options.item_ix()
        ));
        let setter = write.clone();
        let state = window.use_keyed_state(keyed, cx, {
            let current = current.clone();
            move |window, cx| {
                let input = cx.new(|cx| InputState::new(window, cx).default_value(current.clone()));
                let subscription = cx.subscribe(&input, move |_, input, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        setter(input.read(cx).value().to_string(), cx);
                    }
                });
                TextControlState {
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
        Input::new(&input)
            .disabled(options.is_disabled())
            .with_size(options.size())
            .map(|this| {
                if options.layout().is_horizontal() {
                    this.w_64()
                } else {
                    this.w_full()
                }
            })
            .into_any_element()
    })
}

/// A numeric control with the same stepping and bounds as the stock field.
pub fn number_control<V, W>(
    key: &'static str,
    options: NumberFieldOptions,
    value: V,
    write: W,
) -> Control
where
    V: Fn(&App) -> f64 + 'static,
    W: Fn(f64, &mut App) + 'static,
{
    let write: NumberSetter = Rc::new(write);
    let key = SharedString::from(key);
    control(move |render_options, window, cx| {
        let current = value(cx);
        let keyed = SharedString::from(format!(
            "row-number-{key}-{}-{}-{}",
            render_options.page_ix(),
            render_options.group_ix(),
            render_options.item_ix()
        ));
        let options = options.clone();
        let setter = write.clone();
        let state = window.use_keyed_state(keyed, cx, move |window, cx| {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(current.to_string())
                    .step(options.step)
                    .min(options.min)
                    .max(options.max)
            });
            let subscription = cx.subscribe(&input, move |_, input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change)
                    && let Ok(value) = input.read(cx).value().trim().parse::<f64>()
                {
                    setter(value, cx);
                }
            });
            TextControlState {
                input,
                _subscription: subscription,
            }
        });
        let input = state.read(cx).input.clone();
        if input.read(cx).value() != current.to_string() {
            input.update(cx, |input, cx| {
                input.set_value(current.to_string(), window, cx);
            });
        }
        NumberInput::new(&input)
            .disabled(render_options.is_disabled())
            .with_size(render_options.size())
            .map(|this| {
                if render_options.layout().is_horizontal() {
                    this.w_32()
                } else {
                    this.w_full()
                }
            })
            .into_any_element()
    })
}

/// A switch control.
pub fn switch_control<V, W>(key: &'static str, value: V, write: W) -> Control
where
    V: Fn(&App) -> bool + 'static,
    W: Fn(bool, &mut App) + 'static,
{
    let key = SharedString::from(key);
    let write: SwitchSetter = Rc::new(write);
    control(move |options, _window, cx| {
        let checked = value(cx);
        let setter = write.clone();
        Switch::new(ElementId::Name(
            format!("row-switch-{key}-{}", options.item_ix()).into(),
        ))
        .checked(checked)
        .disabled(options.is_disabled())
        .with_size(options.size())
        .on_change(move |value, _window, cx| setter(*value, cx))
        .into_any_element()
    })
}

/// A dropdown control built from `(value, label)` pairs.
pub fn dropdown_control<O, V, W>(key: &'static str, options: O, value: V, write: W) -> Control
where
    O: Fn(&App) -> Vec<(SharedString, SharedString)> + 'static,
    V: Fn(&App) -> String + 'static,
    W: Fn(String, &mut App) + 'static,
{
    let key = SharedString::from(key);
    let write: TextSetter = Rc::new(write);
    control(move |render_options, _window, cx| {
        let current: SharedString = value(cx).into();
        let options = options(cx);
        let label = options
            .iter()
            .find(|(value, _)| *value == current)
            .map(|(_, label)| label.clone())
            .unwrap_or_else(|| {
                if current.is_empty() {
                    SharedString::from("Choose…")
                } else {
                    current.clone()
                }
            });
        let setter = write.clone();
        Button::new(ElementId::Name(
            format!("row-dropdown-{key}-{}", render_options.item_ix()).into(),
        ))
        .when(render_options.layout().is_vertical(), |this| this.w_full())
        .label(label)
        .dropdown_caret(true)
        .outline()
        .disabled(render_options.is_disabled())
        .with_size(render_options.size())
        .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
            options
                .iter()
                .fold(menu, |menu, (value, label)| {
                    let checked = *value == current;
                    let value = value.clone();
                    let setter = setter.clone();
                    menu.item(
                        PopupMenuItem::new(label.clone())
                            .checked(checked)
                            .on_click(move |_, _, cx| setter(value.to_string(), cx)),
                    )
                })
                .scrollable(true)
        })
        .into_any_element()
    })
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
