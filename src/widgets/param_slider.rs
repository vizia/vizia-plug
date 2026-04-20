//! A slider that integrates with NIH-plug's [`Param`] types.

use nih_plug::prelude::{Param, ParamPtr};
use vizia::prelude::*;

use super::param_base::ParamWidgetBase;
use super::util::{self, ModifiersExt};

/// When shift+dragging a parameter, one pixel dragged corresponds to this much change in the
/// normalized parameter.
const GRANULAR_DRAG_MULTIPLIER: f32 = 0.1;

/// A slider that integrates with NIH-plug's [`Param`] types. Use the
/// [`set_style()`][ParamSliderExt::set_style] method to change how the value is displayed.
///
/// Under the signal-based API these three fields are held as [`SyncSignal`]s so the build
/// closure (which needs to be `'static`) can subscribe to them without borrowing the slider.
pub struct ParamSlider {
    param_base: ParamWidgetBase,

    /// Set to `true` when the field gets Alt+Click'ed — replaces the label with a text box.
    text_input_active: SyncSignal<bool>,
    /// What style to use for the slider.
    style: SyncSignal<ParamSliderStyle>,
    /// A specific label to use instead of displaying the parameter's value.
    label_override: SyncSignal<Option<String>>,

    /// Set to `true` while we're dragging the parameter. Resetting the parameter or entering a
    /// text value should not initiate a drag.
    drag_active: bool,
    /// Start coordinate and normalized value when holding down Shift while dragging for higher
    /// precision dragging. `None` when granular dragging is not active.
    granular_drag_status: Option<GranularDragStatus>,

    // These fields are set through modifiers:
    /// Whether or not to listen to scroll events for changing the parameter's value in steps.
    use_scroll_wheel: bool,
    /// Fractional scrolled lines not yet turned into parameter change events. Needed for
    /// trackpads with smooth scrolling.
    scrolled_lines: f32,
}

/// How the [`ParamSlider`] should display its values. Set this using
/// [`ParamSliderExt::set_style`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamSliderStyle {
    /// Visualize the offset from the default value for continuous parameters with a default
    /// value around the middle of the range, fill from the left for discrete parameters and
    /// continuous parameters without centered defaults.
    Centered,
    /// Always fill the bar starting from the left.
    FromLeft,
    /// Show the current step instead of filling a portion of the bar. Useful for discrete
    /// parameters. Set `even` to `true` to distribute the ticks evenly instead of following the
    /// parameter's distribution — discrete parameters span only half the range near the edges,
    /// which can make the display look odd.
    CurrentStep { even: bool },
    /// The same as `CurrentStep`, but overlays the labels over the steps instead of showing the
    /// active value. Only useful for discrete parameters with two or maybe three possible values.
    CurrentStepLabeled { even: bool },
}

enum ParamSliderEvent {
    /// Text input has been cancelled without submitting a new value.
    CancelTextInput,
    /// A new value has been sent by the text input dialog after pressing Enter.
    TextInput(String),
}

#[derive(Debug, Clone, Copy)]
struct GranularDragStatus {
    /// The mouse's X-coordinate when the granular drag was started.
    starting_x_coordinate: f32,
    /// The normalized value when the granular drag was started.
    starting_value: f32,
}

impl ParamSlider {
    /// Creates a new [`ParamSlider`] for the given parameter. Pass a reference to the
    /// parameter directly — e.g. `ParamSlider::new(cx, &params.gain)`.
    ///
    /// Parameter changes are handled by emitting [`ParamEvent`](super::ParamEvent)s, which
    /// are automatically processed by the vizia-plug wrapper.
    ///
    /// See [`ParamSliderExt`] for additional options.
    pub fn new<'c, 'p, P>(cx: &'c mut Context, param: &'p P) -> Handle<'c, Self>
    where
        'p: 'c,
        P: Param + 'static,
    {
        let param_base = ParamWidgetBase::new(cx, param);
        let text_input_active = SyncSignal::new(false);
        let style = SyncSignal::new(ParamSliderStyle::Centered);
        let label_override: SyncSignal<Option<String>> = SyncSignal::new(None);

        let unmodulated_signal = param_base.unmodulated_signal(cx);
        let modulated_signal = param_base.modulated_signal(cx);

        Self {
            param_base,
            text_input_active,
            style,
            label_override,
            drag_active: false,
            granular_drag_status: None,
            use_scroll_wheel: true,
            scrolled_lines: 0.0,
        }
        .build(
            cx,
            ParamWidgetBase::build_view(param, move |cx, _param_data| {
                // Bind on the style signal — style only changes when `.set_style()` is called,
                // so this rebuilds the slider contents rarely in practice.
                Binding::new(cx, style, move |cx| {
                    let style = style.get();
                    let param_ptr = param_base.param_ptr();

                    // Derived display string. Single reactive input: the unmodulated value.
                    // SAFETY for the `ParamPtr` read: resolved from a valid `&impl Param` at
                    // widget construction; the pointer stays valid for the plugin's lifetime.
                    let display_value: Memo<String> = Memo::new(move |_| {
                        let current = unmodulated_signal.get();
                        unsafe { param_ptr.normalized_value_to_string(current, true) }
                    });

                    // `(start_t, delta)` for the filled portion of the bar. `start_t ∈ [0, 1]`,
                    // `delta ∈ [-1, 1]`. Reactive input: the unmodulated value. The helper also
                    // reads static parameter metadata (default value, step count, step
                    // distribution) via the `ParamPtr`; those are invariant for the plugin's
                    // lifetime so they don't need to be tracked as reactive dependencies.
                    let fill_start_delta: Memo<(f32, f32)> = Memo::new(move |_| {
                        let current = unmodulated_signal.get();
                        Self::compute_fill_start_delta(style, param_ptr, current)
                    });

                    // Modulation offset bar. Reactive inputs: both unmodulated and modulated
                    // values — if either moves, the delta must be recomputed. Reading both
                    // via `.get()` inside the memo closure subscribes to both signals.
                    let modulation_start_delta: Memo<(f32, f32)> = Memo::new(move |_| {
                        let unmod = unmodulated_signal.get();
                        let modulated = modulated_signal.get();
                        Self::compute_modulation_fill_start_delta(style, unmod, modulated)
                    });

                    // Only draw the text input when it's active. Otherwise overlay the label
                    // on the slider fill. Creating the textbox based on `text_input_active`
                    // lets us focus it when it gets created.
                    Binding::new(cx, text_input_active, move |cx| {
                        if text_input_active.get() {
                            Self::text_input_view(cx, display_value);
                        } else {
                            ZStack::new(cx, |cx| {
                                Self::slider_fill_view(
                                    cx,
                                    fill_start_delta,
                                    modulation_start_delta,
                                );
                                Self::slider_label_view(
                                    cx,
                                    param_base,
                                    style,
                                    display_value,
                                    label_override,
                                );
                            })
                            .hoverable(false);
                        }
                    });
                });
            }),
        )
    }

    /// Create a text input that's shown in place of the slider.
    fn text_input_view(cx: &mut Context, display_value: Memo<String>) {
        Textbox::new(cx, display_value)
            .class("value-entry")
            .on_submit(|cx, string, success| {
                if success {
                    cx.emit(ParamSliderEvent::TextInput(string))
                } else {
                    cx.emit(ParamSliderEvent::CancelTextInput);
                }
            })
            .on_cancel(|cx| {
                cx.emit(ParamSliderEvent::CancelTextInput);
            })
            .on_build(|cx| {
                cx.emit(TextEvent::StartEdit);
                cx.emit(TextEvent::SelectAll);
            })
            .class("align_center")
            .alignment(Alignment::Left)
            .height(Stretch(1.0))
            .width(Stretch(1.0));
    }

    /// Create the fill part of the slider.
    fn slider_fill_view(
        cx: &mut Context,
        fill_start_delta: Memo<(f32, f32)>,
        modulation_start_delta: Memo<(f32, f32)>,
    ) {
        // The filled bar portion. Visualized differently depending on the current style — see
        // [`ParamSliderStyle`].
        Element::new(cx)
            .class("fill")
            .height(Stretch(1.0))
            .left(fill_start_delta.map(|(start_t, _)| Percentage(*start_t * 100.0)))
            .width(fill_start_delta.map(|(_, delta)| Percentage(*delta * 100.0)))
            // Hovering is handled on the param slider as a whole.
            .hoverable(false);

        // If the parameter is being modulated, another filled bar shows the modulation delta.
        Element::new(cx)
            .class("fill")
            .class("fill--modulation")
            .height(Stretch(1.0))
            .visibility(modulation_start_delta.map(|(_, delta)| *delta != 0.0))
            // Widths can't be negative, so compensate the start position if the width is
            // negative.
            .width(modulation_start_delta.map(|(_, delta)| Percentage(delta.abs() * 100.0)))
            .left(modulation_start_delta.map(|(start_t, delta)| {
                if *delta < 0.0 {
                    Percentage((*start_t + *delta) * 100.0)
                } else {
                    Percentage(*start_t * 100.0)
                }
            }))
            .hoverable(false);
    }

    /// Create the text part of the slider. Shown on top of the fill using a `ZStack`.
    fn slider_label_view(
        cx: &mut Context,
        param_base: ParamWidgetBase,
        style: ParamSliderStyle,
        display_value: Memo<String>,
        label_override: SyncSignal<Option<String>>,
    ) {
        let step_count = param_base.step_count();

        // Either display the current value, or display all values over the parameter's steps.
        match (style, step_count) {
            (ParamSliderStyle::CurrentStepLabeled { .. }, Some(step_count)) => {
                HStack::new(cx, |cx| {
                    // step_count + 1 possible values for a discrete parameter. Each preview
                    // label is a static string derived from the parameter's own formatter at
                    // its step position — it never changes at runtime, so we format it once
                    // here rather than threading it through the reactive graph.
                    for value in 0..step_count + 1 {
                        let normalized_value = value as f32 / step_count as f32;
                        let preview = param_base.normalized_value_to_string(normalized_value, true);

                        Label::new(cx, preview)
                            .class("value")
                            .class("value--multiple")
                            .alignment(Alignment::Center)
                            .size(Stretch(1.0))
                            .hoverable(false);
                    }
                })
                .height(Stretch(1.0))
                .width(Stretch(1.0))
                .hoverable(false);
            }
            _ => {
                // Derived label text: either the `.with_label(...)` override when set, or the
                // parameter's own formatted display value (before modulation). Built as a
                // `Memo<String>` so the Label updates its text in place when either input
                // changes — cheaper than rebuilding the view subtree via `Binding::new`.
                let text: Memo<String> =
                    Memo::new(move |_| label_override.get().unwrap_or_else(|| display_value.get()));
                Label::new(cx, text)
                    .class("value")
                    .class("value--single")
                    .alignment(Alignment::Center)
                    .size(Stretch(1.0))
                    .hoverable(false);
            }
        }
    }

    /// Start position and width of the slider's fill region based on the selected style, the
    /// parameter's current value, and the parameter's step sizes.
    ///
    /// Returns `(start_t, delta)` where `start_t ∈ [0, 1]` and `delta ∈ [-1, 1]`.
    fn compute_fill_start_delta(
        style: ParamSliderStyle,
        param_ptr: ParamPtr,
        current_value: f32,
    ) -> (f32, f32) {
        // SAFETY: `param_ptr` was resolved from a valid `&impl Param` at widget construction;
        // it stays valid for the plugin's lifetime.
        let default_value = unsafe { param_ptr.default_normalized_value() };
        let step_count = unsafe { param_ptr.step_count() };
        let draw_fill_from_default = matches!(style, ParamSliderStyle::Centered)
            && step_count.is_none()
            && (0.45..=0.55).contains(&default_value);

        match style {
            ParamSliderStyle::Centered if draw_fill_from_default => {
                let delta = (default_value - current_value).abs();

                // Don't draw the filled portion at all if it could be a rounding error — those
                // slivers look weird.
                (
                    default_value.min(current_value),
                    if delta >= 1e-3 { delta } else { 0.0 },
                )
            }
            ParamSliderStyle::Centered | ParamSliderStyle::FromLeft => (0.0, current_value),
            ParamSliderStyle::CurrentStep { even: true }
            | ParamSliderStyle::CurrentStepLabeled { even: true }
                if step_count.is_some() =>
            {
                // Assume the normalized value is distributed evenly across the range.
                let step_count = step_count.unwrap() as f32;
                let discrete_values = step_count + 1.0;
                let previous_step = (current_value * step_count) / discrete_values;

                (previous_step, discrete_values.recip())
            }
            ParamSliderStyle::CurrentStep { .. } | ParamSliderStyle::CurrentStepLabeled { .. } => {
                let previous_step =
                    unsafe { param_ptr.previous_normalized_step(current_value, false) };
                let next_step = unsafe { param_ptr.next_normalized_step(current_value, false) };

                (
                    (previous_step + current_value) / 2.0,
                    ((next_step - current_value) + (current_value - previous_step)) / 2.0,
                )
            }
        }
    }

    /// Same as [`compute_fill_start_delta`](Self::compute_fill_start_delta), but showing only
    /// the modulation offset. Pure function of its inputs — the caller is responsible for
    /// feeding fresh `unmodulated_normalized` and `modulated_normalized` values, so the
    /// reactive graph can track both as dependencies.
    fn compute_modulation_fill_start_delta(
        style: ParamSliderStyle,
        unmodulated_normalized: f32,
        modulated_normalized: f32,
    ) -> (f32, f32) {
        match style {
            // Don't show modulation for stepped parameters — visually meaningless.
            ParamSliderStyle::CurrentStep { .. } | ParamSliderStyle::CurrentStepLabeled { .. } => {
                (0.0, 0.0)
            }
            ParamSliderStyle::Centered | ParamSliderStyle::FromLeft => (
                unmodulated_normalized,
                modulated_normalized - unmodulated_normalized,
            ),
        }
    }

    /// `self.param_base.set_normalized_value()`, but resulting from a mouse drag. When using the
    /// 'even' stepped slider styles this remaps the normalized range to match the fill-value
    /// display. Still needs to be wrapped in a parameter automation gesture.
    fn set_normalized_value_drag(&self, cx: &mut EventContext, normalized_value: f32) {
        let normalized_value = match (self.style.get(), self.param_base.step_count()) {
            (
                ParamSliderStyle::CurrentStep { even: true }
                | ParamSliderStyle::CurrentStepLabeled { even: true },
                Some(step_count),
            ) => {
                // Remap the value range to the displayed range (each value occupies an equal
                // area on the slider instead of the centers of those ranges being distributed
                // over the entire `[0, 1]` range).
                let discrete_values = step_count as f32 + 1.0;
                let rounded_value = ((normalized_value * discrete_values) - 0.5).round();
                rounded_value / step_count as f32
            }
            _ => normalized_value,
        };

        self.param_base.set_normalized_value(cx, normalized_value);
    }
}

impl View for ParamSlider {
    fn element(&self) -> Option<&'static str> {
        Some("param-slider")
    }

    fn event(&mut self, cx: &mut EventContext, event: &mut Event) {
        event.map(|param_slider_event, meta| match param_slider_event {
            ParamSliderEvent::CancelTextInput => {
                self.text_input_active.set(false);
                cx.set_active(false);

                meta.consume();
            }
            ParamSliderEvent::TextInput(string) => {
                if let Some(normalized_value) = self.param_base.string_to_normalized_value(string) {
                    self.param_base.begin_set_parameter(cx);
                    self.param_base.set_normalized_value(cx, normalized_value);
                    self.param_base.end_set_parameter(cx);
                }

                self.text_input_active.set(false);

                meta.consume();
            }
        });

        event.map(|window_event, meta| match window_event {
            // Vizia always captures the third mouse click as a triple click. Treating triple
            // click as a regular mouse button makes double-click-then-drag work as expected
            // without requiring a delay or an additional click. Double double click still
            // won't work.
            WindowEvent::MouseDown(MouseButton::Left)
            | WindowEvent::MouseTripleClick(MouseButton::Left) => {
                if cx.modifiers().alt() {
                    // Alt+Click brings up a text entry dialog.
                    self.text_input_active.set(true);
                    cx.set_active(true);
                } else if cx.modifiers().command() {
                    // Ctrl+Click, double click, and right clicks reset the parameter.
                    self.param_base.begin_set_parameter(cx);
                    self.param_base
                        .set_normalized_value(cx, self.param_base.default_normalized_value());
                    self.param_base.end_set_parameter(cx);
                } else if !self.text_input_active.get() {
                    // The `!text_input_active` check shouldn't be needed, but the textbox
                    // doesn't consume the mouse-down event. Without this, clicking on the
                    // textbox to move the cursor would also change the slider.
                    self.drag_active = true;
                    cx.capture();
                    // Otherwise we don't get key-up events.
                    cx.focus();
                    cx.set_active(true);

                    // Holding shift while clicking initiates granular editing without jumping.
                    self.param_base.begin_set_parameter(cx);
                    if cx.modifiers().shift() {
                        self.granular_drag_status = Some(GranularDragStatus {
                            starting_x_coordinate: cx.mouse().cursor_x,
                            starting_value: self.param_base.unmodulated_normalized_value(),
                        });
                    } else {
                        self.granular_drag_status = None;
                        self.set_normalized_value_drag(
                            cx,
                            util::remap_current_entity_x_coordinate(cx, cx.mouse().cursor_x),
                        );
                    }
                }

                meta.consume();
            }
            WindowEvent::MouseDoubleClick(MouseButton::Left)
            | WindowEvent::MouseDown(MouseButton::Right)
            | WindowEvent::MouseDoubleClick(MouseButton::Right)
            | WindowEvent::MouseTripleClick(MouseButton::Right) => {
                self.param_base.begin_set_parameter(cx);
                self.param_base
                    .set_normalized_value(cx, self.param_base.default_normalized_value());
                self.param_base.end_set_parameter(cx);

                meta.consume();
            }
            WindowEvent::MouseUp(MouseButton::Left) => {
                if self.drag_active {
                    self.drag_active = false;
                    cx.release();
                    cx.set_active(false);

                    self.param_base.end_set_parameter(cx);

                    meta.consume();
                }
            }
            WindowEvent::MouseMove(x, _y) => {
                if self.drag_active {
                    // If shift is held, dragging is granular rather than absolute.
                    if cx.modifiers().shift() {
                        let granular_drag_status =
                            *self
                                .granular_drag_status
                                .get_or_insert_with(|| GranularDragStatus {
                                    starting_x_coordinate: *x,
                                    starting_value: self.param_base.unmodulated_normalized_value(),
                                });

                        // Compensate for the DPI scale to keep the drag consistent.
                        let start_x =
                            util::remap_current_entity_x_t(cx, granular_drag_status.starting_value);
                        let delta_x = ((*x - granular_drag_status.starting_x_coordinate)
                            * GRANULAR_DRAG_MULTIPLIER)
                            * cx.scale_factor();

                        self.set_normalized_value_drag(
                            cx,
                            util::remap_current_entity_x_coordinate(cx, start_x + delta_x),
                        );
                    } else {
                        self.granular_drag_status = None;
                        self.set_normalized_value_drag(
                            cx,
                            util::remap_current_entity_x_coordinate(cx, *x),
                        );
                    }
                }
            }
            WindowEvent::KeyUp(_, Some(Key::Shift)) => {
                // If this happens mid-drag, snap back to the current screen position.
                if self.drag_active && self.granular_drag_status.is_some() {
                    self.granular_drag_status = None;
                    self.param_base.set_normalized_value(
                        cx,
                        util::remap_current_entity_x_coordinate(cx, cx.mouse().cursor_x),
                    );
                }
            }
            WindowEvent::MouseScroll(_scroll_x, scroll_y) if self.use_scroll_wheel => {
                // A regular scroll wheel sends ±1; smooth-scrolling trackpads send anything.
                self.scrolled_lines += scroll_y;

                if self.scrolled_lines.abs() >= 1.0 {
                    let use_finer_steps = cx.modifiers().shift();

                    // Scrolling while dragging needs to be taken into account here.
                    if !self.drag_active {
                        self.param_base.begin_set_parameter(cx);
                    }

                    let mut current_value = self.param_base.unmodulated_normalized_value();

                    while self.scrolled_lines >= 1.0 {
                        current_value = self
                            .param_base
                            .next_normalized_step(current_value, use_finer_steps);
                        self.param_base.set_normalized_value(cx, current_value);
                        self.scrolled_lines -= 1.0;
                    }

                    while self.scrolled_lines <= -1.0 {
                        current_value = self
                            .param_base
                            .previous_normalized_step(current_value, use_finer_steps);
                        self.param_base.set_normalized_value(cx, current_value);
                        self.scrolled_lines += 1.0;
                    }

                    if !self.drag_active {
                        self.param_base.end_set_parameter(cx);
                    }
                }

                meta.consume();
            }
            _ => {}
        });
    }
}

/// Extension methods for [`ParamSlider`] handles.
pub trait ParamSliderExt {
    /// Don't respond to scroll wheel events. Useful when the slider sits inside a scrolling
    /// container.
    fn disable_scroll_wheel(self) -> Self;

    /// Change how the [`ParamSlider`] visualizes the current value.
    fn set_style(self, style: ParamSliderStyle) -> Self;

    /// Manually set a fixed label for the slider instead of displaying the current value.
    fn with_label(self, value: impl Into<String>) -> Self;
}

impl ParamSliderExt for Handle<'_, ParamSlider> {
    fn disable_scroll_wheel(self) -> Self {
        self.modify(|param_slider: &mut ParamSlider| param_slider.use_scroll_wheel = false)
    }

    fn set_style(self, style: ParamSliderStyle) -> Self {
        self.modify(|param_slider: &mut ParamSlider| param_slider.style.set(style))
    }

    fn with_label(self, value: impl Into<String>) -> Self {
        self.modify(|param_slider: &mut ParamSlider| {
            param_slider.label_override.set(Some(value.into()));
        })
    }
}
