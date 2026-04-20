//! Generic UIs for NIH-plug using VIZIA.

use nih_plug::prelude::{ParamFlags, ParamPtr, Params};
use vizia::prelude::*;

use super::{ParamSlider, ParamSliderExt, ParamSliderStyle};

/// Shows a generic UI for a [`Params`] object. For additional flexibility use
/// [`new_custom`](Self::new_custom) to override the widget chosen for each parameter.
pub struct GenericUi;

impl GenericUi {
    /// Creates a new [`GenericUi`] for all parameters on the given `Params` object. Use
    /// [`new_custom`](Self::new_custom) to decide which widget gets used for each parameter.
    ///
    /// `params` only needs to outlive the call (not the whole widget lifetime): the builder
    /// calls `params.param_map()` once up-front to enumerate the parameters and never holds
    /// onto `params` after that. The individual parameter widgets (e.g. [`ParamSlider`]) are
    /// constructed via `unsafe { &*p }` off of the `ParamPtr`s returned by `param_map`, which
    /// relies on the plugin's `Arc<Params>` outliving the editor — same as everywhere else in
    /// vizia-plug.
    ///
    /// Wrap this in a [`ScrollView`] for plugins with long parameter lists:
    ///
    /// ```ignore
    /// ScrollView::new(cx, 0.0, 0.0, false, true, |cx| {
    ///     GenericUi::new(cx, &*params);
    /// })
    /// .width(Percentage(100.0));
    /// ```
    pub fn new<'c, Ps>(cx: &'c mut Context, params: &Ps) -> Handle<'c, GenericUi>
    where
        Ps: Params + 'static,
    {
        // Basic styling is done in the `theme.css` style sheet.
        Self::new_custom(cx, params, move |cx, param_ptr| {
            HStack::new(cx, |cx| {
                // Align this on the right
                // `Label::new` takes `impl Res<T> + 'static` — `param_ptr.name()` returns
                // a borrowed `&str`, so we own it here for the static-lifetime bound.
                Label::new(cx, unsafe { param_ptr.name() }.to_owned()).class("label");

                Self::draw_widget(cx, param_ptr);
            })
            .class("row");
        })
    }

    /// Creates a new [`GenericUi`] using a custom closure that draws the widget for each
    /// parameter.
    pub fn new_custom<'c, Ps>(
        cx: &'c mut Context,
        params: &Ps,
        mut make_widget: impl FnMut(&mut Context, ParamPtr),
    ) -> Handle<'c, Self>
    where
        Ps: Params + 'static,
    {
        Self.build(cx, |cx| {
            let param_map = params.param_map();
            for (_, param_ptr, _) in param_map {
                let flags = unsafe { param_ptr.flags() };
                if flags.contains(ParamFlags::HIDE_IN_GENERIC_UI) {
                    continue;
                }

                make_widget(cx, param_ptr);
            }
        })
    }

    /// The standard widget-drawing function. Use with [`new_custom`](Self::new_custom) to keep
    /// the default widget choice while customising the surrounding label.
    pub fn draw_widget(cx: &mut Context, param_ptr: ParamPtr) {
        // SAFETY: the `*mut P` raw pointers inside `ParamPtr` come from the plugin's pinned
        // `Params` struct (held behind an `Arc`), so the `&P` borrows below are valid for the
        // plugin's entire lifetime.
        let handle = unsafe {
            match param_ptr {
                ParamPtr::FloatParam(p) => ParamSlider::new(cx, &*p),
                ParamPtr::IntParam(p) => ParamSlider::new(cx, &*p),
                ParamPtr::BoolParam(p) => ParamSlider::new(cx, &*p),
                ParamPtr::EnumParam(p) => ParamSlider::new(cx, &*p),
            }
        };

        handle
            .set_style(match unsafe { param_ptr.step_count() } {
                // This looks nice for boolean values, but gets too crowded for anything beyond
                // that without making the widget wider.
                Some(step_count) if step_count <= 1 => {
                    ParamSliderStyle::CurrentStepLabeled { even: true }
                }
                Some(step_count) if step_count <= 2 => ParamSliderStyle::CurrentStep { even: true },
                Some(_) => ParamSliderStyle::FromLeft,
                // Default. Continuous parameters are drawn from the centre if the default is
                // also centred, or from the left if it is not.
                None => ParamSliderStyle::Centered,
            })
            .class("widget");
    }
}

impl View for GenericUi {
    fn element(&self) -> Option<&'static str> {
        Some("generic-ui")
    }
}
