//! A base widget for creating other widgets that integrate with NIH-plug's [`Param`] types.

use std::sync::Arc;

use nih_plug::prelude::*;
use vizia::prelude::*;

use super::param_registry::{ParamAxis, ParamRegistry};
use super::RawParamEvent;

/// A helper for creating parameter widgets. The general idea is that a parameter widget struct
/// adds a [`ParamWidgetBase`] field on its struct, and then calls [`ParamWidgetBase::view`] in its
/// view build function. The stored `ParamWidgetBase` object can then be used in the widget's event
/// handlers to interact with the parameter, and provides accessors (via
/// [`ParamWidgetBase::modulated_signal`] / [`ParamWidgetBase::unmodulated_signal`]) for binding
/// the parameter's current value into views.
///
/// Signals are owned by the editor's [`ParamRegistry`] model and shared across all widgets that
/// reference the same parameter.
pub struct ParamWidgetBase {
    /// Opaque handle to the parameter. Stable for the lifetime of the plugin; safe to copy and
    /// re-use across widget instances.
    param_ptr: ParamPtr,
}

/// Data and signal accessors that can be used to draw the parameter widget. The [`param`][Self::param]
/// field should only be used for looking up static data (parameter name, step count, formatters).
/// For binding live values to view properties, use the `signal` accessors which return
/// [`SyncSignal<f32>`]s tracked by the reactive graph.
pub struct ParamWidgetData<P: Param + 'static> {
    // HACK: This needs to be a static reference because of the way bindings in vizia work. The
    //       field is not `pub` for this reason — widgets access it via `ParamWidgetData::param()`.
    param: &'static P,
    param_ptr: ParamPtr,
}

impl<P: Param + 'static> Clone for ParamWidgetData<P> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<P: Param + 'static> Copy for ParamWidgetData<P> {}

impl<P: Param + 'static> ParamWidgetData<P> {
    /// The parameter in question. Use for querying static information (name, unit, step count,
    /// formatters). Don't use this to read the parameter's current value — use
    /// [`modulated_signal`][Self::modulated_signal] instead so the reactive graph can track the
    /// dependency.
    pub fn param(&self) -> &P {
        self.param
    }

    /// The underlying [`ParamPtr`] for this parameter. Widgets don't usually need this directly;
    /// prefer the signal accessors or the action methods on [`ParamWidgetBase`].
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
    /// Creates a [`ParamWidgetBase`] for the given parameter. Parameter changes are handled by
    /// emitting [`ParamEvent`][super::ParamEvent]s, which are automatically processed by the
    /// vizia-plug wrapper.
    ///
    /// `params` is a shared reference to the plugin's `Params` struct; `params_to_param` projects
    /// into the specific parameter you want this widget to drive. The `Params` reference is only
    /// used at construction time to resolve the opaque [`ParamPtr`]; widgets hold onto the pointer
    /// and use signals from the editor's [`ParamRegistry`] for live values.
    pub fn new<Params, P, FMap>(
        _cx: &Context,
        params: Arc<Params>,
        params_to_param: FMap,
    ) -> Self
    where
        Params: 'static,
        P: Param,
        FMap: Fn(&Params) -> &P,
    {
        let param_ptr = params_to_param(&params).as_ptr();
        Self { param_ptr }
    }

    /// Create a view using the parameter's data. The `content` closure receives a
    /// [`ParamWidgetData`] that gives access to static parameter metadata and to signals for
    /// binding live values.
    ///
    /// SAFETY: The `&'static P` made available via [`ParamWidgetData::param`] does not actually
    /// outlive the call — but in the vizia-plug setup the `&P` outlives the editor (the plugin's
    /// `Params` struct is pinned for the plugin's lifetime). This mirrors the pre-signal API.
    pub fn view<Params, P, FMap, F, R>(
        cx: &mut Context,
        params: Arc<Params>,
        params_to_param: FMap,
        content: F,
    ) -> R
    where
        Params: 'static,
        P: Param + 'static,
        FMap: Fn(&Params) -> &P,
        F: FnOnce(&mut Context, ParamWidgetData<P>) -> R,
    {
        // SAFETY: see function docs.
        let param_ref = params_to_param(&params);
        let param_ptr = param_ref.as_ptr();
        let param: &'static P = unsafe { &*(param_ref as *const P) };

        let param_data = ParamWidgetData { param, param_ptr };
        content(cx, param_data)
    }

    /// Shorthand for [`view`][Self::view] that returns a closure suitable for
    /// [`View::build`](vizia::prelude::View::build).
    pub fn build_view<Params, P, FMap, F, R>(
        params: Arc<Params>,
        params_to_param: FMap,
        content: F,
    ) -> impl FnOnce(&mut Context) -> R
    where
        Params: 'static,
        P: Param + 'static,
        FMap: Fn(&Params) -> &P + 'static,
        F: FnOnce(&mut Context, ParamWidgetData<P>) -> R,
    {
        move |cx| Self::view(cx, params, params_to_param, content)
    }

    /// Returns the signal tracking this widget's parameter on the given axis. Widgets can bind
    /// view properties to this signal directly, or wrap it in a [`Memo`] for derived values.
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

    /// Start an automation gesture. **Must** be called before [`set_normalized_value`][Self::set_normalized_value];
    /// typically fired on mouse-down.
    pub fn begin_set_parameter(&self, cx: &mut EventContext) {
        cx.emit(RawParamEvent::BeginSetParameter(self.param_ptr));
    }

    /// Set the normalised value for a parameter. Must be wrapped in matching
    /// [`begin_set_parameter`][Self::begin_set_parameter] /
    /// [`end_set_parameter`][Self::end_set_parameter] calls.
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
/// not created via [`create_vizia_editor`][crate::create_vizia_editor].
fn registry(cx: &Context) -> &ParamRegistry {
    cx.data::<ParamRegistry>()
}
