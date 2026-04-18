//! Bridge between nih-plug's pull-based [`Param`] model and vizia's push-based
//! [`SyncSignal`] reactive graph.
//!
//! nih-plug exposes parameters through [`ParamPtr`] — stable opaque handles
//! whose current values are read on demand via unsafe accessors. vizia's new
//! signal-based binding system (vizia#619) requires observable values to be
//! wrapped in [`SyncSignal`] so the reactive graph can track dependencies and
//! push updates to subscribers.
//!
//! [`ParamRegistry`] owns one [`SyncSignal<f32>`] per (ParamPtr, axis) pair
//! (axes: `Modulated`, `Unmodulated`). Widgets call
//! [`ParamRegistry::normalized_signal`] on construction to get a signal for
//! the param value they care about; the registry lazily creates signals on
//! first access and reuses them on subsequent accesses.
//!
//! The editor side is responsible for flushing current values from
//! [`ParamPtr`]s into the registry's signals whenever nih-plug reports a
//! parameter change (typically via
//! [`Editor::parameter_values_changed`][nih_plug::prelude::Editor::parameter_values_changed]).
//! See [`ParamRegistry::flush_all`].

use std::collections::HashMap;
use std::sync::Mutex;

use nih_plug::prelude::ParamPtr;
use vizia::prelude::*;

/// Which value of a parameter a signal tracks. nih-plug distinguishes
/// between the raw user/host-set value (*unmodulated*) and the value after
/// any monophonic modulation has been applied (*modulated*). Most widgets
/// want modulated — it's what the user sees driving the audio — but some
/// (e.g. a slider that visualises both) want both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParamAxis {
    /// `ParamPtr::modulated_normalized_value()`.
    Modulated,
    /// `ParamPtr::unmodulated_normalized_value()`.
    Unmodulated,
}

/// Model that holds one [`SyncSignal<f32>`] per (ParamPtr, axis) pair, lazily
/// created on first widget access.
///
/// Installed on the editor root by [`ViziaEditor::spawn`][super::super::editor::ViziaEditor::spawn]
/// so widgets can reach it via [`Context::data`].
pub struct ParamRegistry {
    /// Lazily populated map of (`ParamPtr`, axis) → signal. The mutex is
    /// only contended during widget construction and the editor's
    /// `parameter_values_changed` flush, neither of which is hot.
    signals: Mutex<HashMap<(ParamPtr, ParamAxis), SyncSignal<f32>>>,
}

impl ParamRegistry {
    /// Creates an empty registry. Call on editor spawn and
    /// [`Model::build`](vizia::prelude::Model::build) into the root context.
    pub fn new() -> Self {
        Self { signals: Mutex::new(HashMap::new()) }
    }

    /// Returns the signal tracking `param_ptr`'s value on the given `axis`,
    /// creating it (initialised from the current unsafe `ParamPtr` value) if
    /// it does not yet exist.
    pub fn signal(&self, param_ptr: ParamPtr, axis: ParamAxis) -> SyncSignal<f32> {
        let mut signals = self
            .signals
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        *signals.entry((param_ptr, axis)).or_insert_with(|| {
            let initial = unsafe {
                match axis {
                    ParamAxis::Modulated => param_ptr.modulated_normalized_value(),
                    ParamAxis::Unmodulated => param_ptr.unmodulated_normalized_value(),
                }
            };
            SyncSignal::new(initial)
        })
    }

    /// Shorthand for the common case: the modulated normalised value.
    pub fn modulated(&self, param_ptr: ParamPtr) -> SyncSignal<f32> {
        self.signal(param_ptr, ParamAxis::Modulated)
    }

    /// Shorthand for the unmodulated (user/host-set) value.
    pub fn unmodulated(&self, param_ptr: ParamPtr) -> SyncSignal<f32> {
        self.signal(param_ptr, ParamAxis::Unmodulated)
    }

    /// Re-read every registered parameter via unsafe `ParamPtr` and write the
    /// current value into its signal. Intended to be called from the editor's
    /// `parameter_values_changed` hook; the reactive graph will then notify
    /// any bound widgets.
    pub fn flush_all(&self) {
        let signals = self
            .signals
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        for ((param_ptr, axis), signal) in signals.iter() {
            let current = unsafe {
                match axis {
                    ParamAxis::Modulated => param_ptr.modulated_normalized_value(),
                    ParamAxis::Unmodulated => param_ptr.unmodulated_normalized_value(),
                }
            };
            signal.set_if_changed(current);
        }
    }
}

impl Default for ParamRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl Model for ParamRegistry {}
