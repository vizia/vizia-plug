//! A base widget for creating other widgets that integrate with NIH-plug's [`Param`] types.

use nih_plug::prelude::*;
use vizia::prelude::*;

use super::param_registry::{ParamAxis, ParamRegistry};
use super::RawParamEvent;

/// A helper for creating parameter widgets. The general idea is that a parameter widget struct
/// stores a [`ParamWidgetBase`] field, calls [`ParamWidgetBase::view`] in its build function,
/// and uses the base's action methods ([`begin_set_parameter`](Self::begin_set_parameter),
/// [`set_normalized_value`](Self::set_normalized_value),
/// [`end_set_parameter`](Self::end_set_parameter)) in its event handlers.
///
/// Signals for a parameter's live values are owned by the editor's [`ParamRegistry`] model and
/// shared across every widget that targets the same parameter, so binding a label and a knob to
/// the same parameter costs a single `SyncSignal<f32>`.
///
/// `ParamWidgetBase` is `Copy` because its entire state is a [`ParamPtr`]. Widgets can freely
/// capture a copy into any `'static` closure (e.g. a `Binding::new` builder) without dragging a
/// borrow of the user's `Params` struct along with it.
#[derive(Clone, Copy)]
pub struct ParamWidgetBase {
    /// Opaque handle to the parameter. Stable for the lifetime of the plugin; `Copy`, so safe to
    /// reuse across widget instances.
    param_ptr: ParamPtr,
}

/// Data and signal accessors passed to the build closure in
/// [`ParamWidgetBase::view`] / [`ParamWidgetBase::build_view`]. Carries a borrow of the parameter
/// for typed access to static metadata (name, step count, formatters) plus lazy accessors for
/// the registry-owned signals that track live values.
///
/// The `'a` lifetime matches the borrow passed into `view` / `build_view`. In the normal
/// vizia-plug setup the plugin's `Params` struct is pinned for the plugin's lifetime via
/// `Arc<Params>`, so a borrow of a specific parameter inside it easily outlives any widget that
/// refers to it.
pub struct ParamWidgetData<'a, P: Param + 'static> {
    param: &'a P,
    param_ptr: ParamPtr,
}

impl<P: Param + 'static> Clone for ParamWidgetData<'_, P> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<P: Param + 'static> Copy for ParamWidgetData<'_, P> {}

impl<'a, P: Param + 'static> ParamWidgetData<'a, P> {
    /// The parameter itself. Use for static information (name, unit, step count, formatters). For
    /// reading the live value, use [`modulated_signal`](Self::modulated_signal) instead so the
    /// reactive graph can track the dependency.
    pub fn param(&self) -> &'a P {
        self.param
    }

    /// The underlying [`ParamPtr`]. Widgets don't usually need this directly; prefer the signal
    /// accessors or the action methods on [`ParamWidgetBase`].
    pub fn param_ptr(&self) -> ParamPtr {
        self.param_ptr
    }

    /// Signal tracking the parameter's modulated normalised value. This is what the user sees
    /// driving the audio; most widgets should bind to this.
    pub fn modulated_signal(&self, cx: &Context) -> SyncSignal<f32> {
        registry(cx).modulated(self.param_ptr)
    }

    /// Signal tracking the parameter's unmodulated (user/host-set) normalised value. Useful when
    /// a widget needs to distinguish the user-set value from host modulation.
    pub fn unmodulated_signal(&self, cx: &Context) -> SyncSignal<f32> {
        registry(cx).unmodulated(self.param_ptr)
    }
}

/// Generate a [`ParamWidgetBase`] method that forwards the call to the underlying [`ParamPtr`].
macro_rules! param_ptr_forward(
    (pub fn $method:ident(&self $(, $arg_name:ident: $arg_ty:ty)*) -> $ret:ty) => {
        /// Calls the corresponding method on the underlying [`ParamPtr`].
        pub fn $method(&self $(, $arg_name: $arg_ty)*) -> $ret {
            unsafe { self.param_ptr.$method($($arg_name),*) }
        }
    };
);

impl ParamWidgetBase {
    /// Creates a [`ParamWidgetBase`] for the given parameter. The reference is only used at
    /// construction time to resolve the parameter's opaque [`ParamPtr`] — the widget does not
    /// keep a borrow of it. Callers typically pass a field of their `Params` struct, e.g.
    /// `ParamSlider::new(cx, &params.gain)`.
    ///
    /// Parameter changes are handled by emitting [`ParamEvent`](super::ParamEvent)s, which are
    /// automatically processed by the vizia-plug wrapper.
    pub fn new<P: Param>(_cx: &Context, param: &P) -> Self {
        Self { param_ptr: param.as_ptr() }
    }

    /// Create a view using the parameter's data. The `content` closure receives a
    /// [`ParamWidgetData`] that gives typed access to the parameter (for static metadata) and
    /// signals for binding live values.
    ///
    /// `param` only needs to outlive the call; the `'a` lifetime is carried through
    /// [`ParamWidgetData`] so the builder closure can safely borrow it.
    pub fn view<'a, P, F, R>(cx: &mut Context, param: &'a P, content: F) -> R
    where
        P: Param + 'static,
        F: FnOnce(&mut Context, ParamWidgetData<'a, P>) -> R,
    {
        let param_data = ParamWidgetData { param, param_ptr: param.as_ptr() };
        content(cx, param_data)
    }

    /// Shorthand for [`view`](Self::view) that returns a builder closure suitable for
    /// [`View::build`](vizia::prelude::View::build).
    pub fn build_view<'a, P, F, R>(
        param: &'a P,
        content: F,
    ) -> impl FnOnce(&mut Context) -> R + 'a
    where
        P: Param + 'static,
        F: FnOnce(&mut Context, ParamWidgetData<'a, P>) -> R + 'a,
    {
        move |cx| Self::view(cx, param, content)
    }

    /// Returns the signal tracking this widget's parameter on the given axis. Widgets can bind
    /// view properties to this signal directly, or wrap it in a [`Memo`] for derived views.
    pub fn signal(&self, cx: &Context, axis: ParamAxis) -> SyncSignal<f32> {
        registry(cx).signal(self.param_ptr, axis)
    }

    /// Shorthand for `signal(cx, ParamAxis::Modulated)` — the value most widgets want to display.
    pub fn modulated_signal(&self, cx: &Context) -> SyncSignal<f32> {
        registry(cx).modulated(self.param_ptr)
    }

    /// Shorthand for `signal(cx, ParamAxis::Unmodulated)`.
    pub fn unmodulated_signal(&self, cx: &Context) -> SyncSignal<f32> {
        registry(cx).unmodulated(self.param_ptr)
    }

    /// The [`ParamPtr`] backing this widget. Usually not needed directly.
    pub fn param_ptr(&self) -> ParamPtr {
        self.param_ptr
    }

    /// Start an automation gesture. **Must** be called before
    /// [`set_normalized_value`](Self::set_normalized_value); typically fired on mouse-down.
    pub fn begin_set_parameter(&self, cx: &mut EventContext) {
        cx.emit(RawParamEvent::BeginSetParameter(self.param_ptr));
    }

    /// Set the normalised value for a parameter. Must be wrapped in matching
    /// [`begin_set_parameter`](Self::begin_set_parameter) /
    /// [`end_set_parameter`](Self::end_set_parameter) calls.
    pub fn set_normalized_value(&self, cx: &mut EventContext, normalized_value: f32) {
        // Snap to the nearest plain value for stepped params.
        let plain_value = unsafe { self.param_ptr.preview_plain(normalized_value) };
        let normalized_plain_value = unsafe { self.param_ptr.preview_normalized(plain_value) };
        cx.emit(RawParamEvent::SetParameterNormalized(
            self.param_ptr,
            normalized_plain_value,
        ));
    }

    /// End an automation gesture. Typically fired on mouse-up.
    pub fn end_set_parameter(&self, cx: &mut EventContext) {
        cx.emit(RawParamEvent::EndSetParameter(self.param_ptr));
    }

    param_ptr_forward!(pub fn name(&self) -> &str);
    param_ptr_forward!(pub fn unit(&self) -> &'static str);
    param_ptr_forward!(pub fn poly_modulation_id(&self) -> Option<u32>);
    param_ptr_forward!(pub fn modulated_plain_value(&self) -> f32);
    param_ptr_forward!(pub fn unmodulated_plain_value(&self) -> f32);
    param_ptr_forward!(pub fn modulated_normalized_value(&self) -> f32);
    param_ptr_forward!(pub fn unmodulated_normalized_value(&self) -> f32);
    param_ptr_forward!(pub fn default_plain_value(&self) -> f32);
    param_ptr_forward!(pub fn default_normalized_value(&self) -> f32);
    param_ptr_forward!(pub fn step_count(&self) -> Option<usize>);
    param_ptr_forward!(pub fn previous_normalized_step(&self, from: f32, finer: bool) -> f32);
    param_ptr_forward!(pub fn next_normalized_step(&self, from: f32, finer: bool) -> f32);
    param_ptr_forward!(pub fn normalized_value_to_string(&self, normalized: f32, include_unit: bool) -> String);
    param_ptr_forward!(pub fn string_to_normalized_value(&self, string: &str) -> Option<f32>);
    param_ptr_forward!(pub fn preview_normalized(&self, plain: f32) -> f32);
    param_ptr_forward!(pub fn preview_plain(&self, normalized: f32) -> f32);
    param_ptr_forward!(pub fn flags(&self) -> ParamFlags);
}

/// Look up the [`ParamRegistry`] installed on the editor root. Panics (via
/// [`Context::data`]'s own assertion) if no registry is found, which indicates the editor was
/// not created via [`create_vizia_editor`](crate::create_vizia_editor).
fn registry(cx: &Context) -> &ParamRegistry {
    cx.data::<ParamRegistry>()
}
