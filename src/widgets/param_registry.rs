//! Bridge between nih-plug's pull-based [`Param`] model and vizia's push-based
//! [`SyncSignal`] reactive graph.
//!
//! nih-plug exposes parameters through [`ParamPtr`] — stable opaque handles whose current
//! values are read on demand via unsafe accessors. vizia's new signal-based binding system
//! (vizia#619) requires observable values to be wrapped in [`SyncSignal`] so the reactive
//! graph can track dependencies and push updates to subscribers.
//!
//! [`ParamRegistry`] owns one [`SyncSignal<f32>`] per `(ParamPtr, axis)` pair (axes:
//! `Modulated`, `Unmodulated`). Widgets call
//! [`ParamRegistry::modulated`] / [`ParamRegistry::unmodulated`] on construction to obtain a
//! signal for the param value they care about; the registry lazily creates signals on first
//! access and reuses them on subsequent accesses.
//!
//! The editor side is responsible for flushing current values from [`ParamPtr`]s into the
//! registry's signals whenever nih-plug reports a parameter change (via
//! [`Editor::parameter_value_changed`](nih_plug::prelude::Editor::parameter_value_changed) /
//! [`Editor::parameter_values_changed`](nih_plug::prelude::Editor::parameter_values_changed)).
//! See [`ParamRegistry::flush_all`].
//!
//! The type is cheaply `Clone` (it's an `Arc` internally), so the editor can keep its own
//! handle for flushing while also installing a clone as a vizia [`Model`] for widget lookup.

use std::collections::HashMap;
use std::sync::Arc;

use nih_plug::prelude::ParamPtr;
use parking_lot::Mutex;
use vizia::prelude::*;

/// Which value of a parameter a signal tracks. nih-plug distinguishes between the raw
/// user/host-set value (*unmodulated*) and the value after any monophonic modulation has been
/// applied (*modulated*). Most widgets want modulated — it's what the user sees driving the
/// audio — but some (e.g. a slider that visualises both) want both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParamAxis {
    /// `ParamPtr::modulated_normalized_value()`.
    Modulated,
    /// `ParamPtr::unmodulated_normalized_value()`.
    Unmodulated,
}

/// Shared, `Clone`-able handle to a set of param-tracking [`SyncSignal`]s. The same value
/// backs both the editor's flush path and the widget-facing lookup path — cloning a
/// `ParamRegistry` returns another handle to the same underlying signal map.
#[derive(Clone)]
pub struct ParamRegistry {
    inner: Arc<ParamRegistryInner>,
}

struct ParamRegistryInner {
    /// Lazily populated map of `(ParamPtr, axis)` → signal.
    ///
    /// Locked briefly by widgets on construction (UI thread) and by the editor on every
    /// parameter-change callback ([`flush_all`](ParamRegistry::flush_all), which nih-plug
    /// calls on the host / audio thread). Using `parking_lot::Mutex` rather than
    /// `std::sync::Mutex` keeps the audio-thread side light — no poisoning checks, no
    /// priority-inversion hazard if the UI thread is holding the lock during widget build.
    signals: Mutex<HashMap<(ParamPtr, ParamAxis), SyncSignal<f32>>>,
}

impl ParamRegistry {
    /// Creates an empty registry.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(ParamRegistryInner { signals: Mutex::new(HashMap::new()) }),
        }
    }

    /// Returns the signal tracking `param_ptr`'s value on the given `axis`, creating it
    /// (initialised from the current unsafe `ParamPtr` value) if it does not yet exist.
    pub fn signal(&self, param_ptr: ParamPtr, axis: ParamAxis) -> SyncSignal<f32> {
        let mut signals = self.inner.signals.lock();

        *signals.entry((param_ptr, axis)).or_insert_with(|| {
            // SAFETY: `param_ptr` was resolved from a valid `&impl Param` at widget
            // construction; it stays valid for the plugin's lifetime.
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

    /// Re-read every registered parameter via unsafe `ParamPtr` and write the current value
    /// into its signal. Intended to be called from the editor's `parameter_value_changed` /
    /// `parameter_values_changed` hooks; the reactive graph then notifies any bound widgets.
    pub fn flush_all(&self) {
        let signals = self.inner.signals.lock();

        for ((param_ptr, axis), signal) in signals.iter() {
            // SAFETY: see `signal()`.
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
