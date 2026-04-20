//! A toggleable button that integrates with NIH-plug's [`Param`] types.

use nih_plug::prelude::Param;
use vizia::prelude::*;

use super::param_base::ParamWidgetBase;

/// A toggleable button that integrates with NIH-plug's [`Param`] types. Only makes sense with
/// [`BoolParam`][nih_plug::prelude::BoolParam]s. Clicking the button toggles between the
/// parameter's minimum and maximum value. The `:checked` pseudoclass indicates whether the
/// button is currently pressed.
pub struct ParamButton {
    param_base: ParamWidgetBase,

    // These fields are set through modifiers:
    /// Whether or not to listen to scroll events for changing the parameter's value in steps.
    use_scroll_wheel: bool,
    /// A specific label to use instead of displaying the parameter's value. Wrapped in a signal
    /// so the `.with_label` modifier can update it after construction and the label rebinds.
    label_override: SyncSignal<Option<String>>,

    /// The number of (fractional) scrolled lines that have not yet been turned into parameter
    /// change events. Supports trackpads with smooth scrolling.
    scrolled_lines: f32,
}

impl ParamButton {
    /// Creates a new [`ParamButton`] for the given parameter. Pass a reference to the
    /// parameter directly — e.g. `ParamButton::new(cx, &params.my_toggle)`.
    pub fn new<'c, 'p, P>(cx: &'c mut Context, param: &'p P) -> Handle<'c, Self>
    where
        'p: 'c,
        P: Param + 'static,
    {
        let param_base = ParamWidgetBase::new(cx, param);
        let modulated_signal = param_base.modulated_signal(cx);
        let label_override: SyncSignal<Option<String>> = SyncSignal::new(None);

        Self {
            param_base,
            use_scroll_wheel: true,
            label_override,
            scrolled_lines: 0.0,
        }
        .build(
            cx,
            ParamWidgetBase::build_view(param, move |cx, param_data| {
                let param_name = param_data.param().name().to_owned();
                // Derived label text: either the `.with_label(...)` override when set, or the
                // parameter's own name. Built as a `Memo<String>` so the Label updates its
                // text in place when the override changes — cheaper than rebuilding the view
                // subtree via `Binding::new`.
                let text: Memo<String> =
                    Memo::new(move |_| label_override.get().unwrap_or_else(|| param_name.clone()));
                Label::new(cx, text).hoverable(false);
            }),
        )
        // `:checked` pseudo-class when the button is on. Uses modulated value — there's no
        // convenient way to display both modulated and unmodulated for a button.
        .checked(modulated_signal.map(|v| *v >= 0.5))
    }

    /// Set the parameter's normalised value to either 0.0 or 1.0 depending on its current value.
    fn toggle_value(&self, cx: &mut EventContext) {
        let current_value = self.param_base.unmodulated_normalized_value();
        let new_value = if current_value >= 0.5 { 0.0 } else { 1.0 };

        self.param_base.begin_set_parameter(cx);
        self.param_base.set_normalized_value(cx, new_value);
        self.param_base.end_set_parameter(cx);
    }
}

impl View for ParamButton {
    fn element(&self) -> Option<&'static str> {
        Some("param-button")
    }

    fn event(&mut self, cx: &mut EventContext, event: &mut Event) {
        event.map(|window_event, meta| match window_event {
            // We don't need special double and triple click handling
            WindowEvent::MouseDown(MouseButton::Left)
            | WindowEvent::MouseDoubleClick(MouseButton::Left)
            | WindowEvent::MouseTripleClick(MouseButton::Left) => {
                self.toggle_value(cx);
                meta.consume();
            }
            WindowEvent::MouseScroll(_scroll_x, scroll_y) if self.use_scroll_wheel => {
                // A regular scroll wheel sends ±1; smooth-scrolling trackpads send anything.
                self.scrolled_lines += scroll_y;

                if self.scrolled_lines.abs() >= 1.0 {
                    self.param_base.begin_set_parameter(cx);

                    if self.scrolled_lines >= 1.0 {
                        self.param_base.set_normalized_value(cx, 1.0);
                        self.scrolled_lines -= 1.0;
                    } else {
                        self.param_base.set_normalized_value(cx, 0.0);
                        self.scrolled_lines += 1.0;
                    }

                    self.param_base.end_set_parameter(cx);
                }

                meta.consume();
            }
            _ => {}
        });
    }
}

/// Extension methods for [`ParamButton`] handles.
pub trait ParamButtonExt {
    /// Don't respond to scroll wheel events. Useful when the button sits inside a scrolling
    /// container.
    fn disable_scroll_wheel(self) -> Self;

    /// Change the colour scheme for a bypass button. Adds the `bypass` CSS class.
    fn for_bypass(self) -> Self;

    /// Change the label used for the button. If this is not set, the parameter's name is used.
    fn with_label(self, value: impl Into<String>) -> Self;
}

impl ParamButtonExt for Handle<'_, ParamButton> {
    fn disable_scroll_wheel(self) -> Self {
        self.modify(|param_button: &mut ParamButton| param_button.use_scroll_wheel = false)
    }

    fn for_bypass(self) -> Self {
        self.class("bypass")
    }

    fn with_label(self, value: impl Into<String>) -> Self {
        self.modify(|param_button: &mut ParamButton| {
            param_button.label_override.set(Some(value.into()));
        })
    }
}
