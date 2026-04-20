//! The [`Editor`] trait implementation for Vizia editors.


use crossbeam::atomic::AtomicCell;
use nih_plug::debug::*;
use nih_plug::prelude::{Editor, GuiContext, Modifiers, ParentWindowHandle, VirtualKeyCode};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use vizia::prelude::*;
use vizia::views::TextEvent;

use vizia_reactive::Runtime;

use crate::widgets::param_registry::ParamRegistry;
use crate::widgets::RawParamEvent;
use crate::{widgets, ViziaState, ViziaTheming};

/// A key-down event queued by the host-thread
/// `Editor::on_virtual_key_from_host` callback, waiting for the next
/// `on_idle` tick to dispatch on the GUI thread.
///
/// Split between character input (goes through `TextEvent::InsertText`)
/// and non-printable control keys (goes through `WindowEvent::KeyDown`)
/// so the GUI-thread drain stays trivially correct without having to
/// re-guess a key's semantics: the classification is made on the host
/// thread from the host's virtual key code.
pub(crate) enum KeyInject {
    /// A printable character (derived from a virtual key that maps 1:1
    /// to a printable character: Space, Numpad0..Numpad9, the numpad
    /// operator keys, Equals). Dispatched as `TextEvent::InsertText`.
    Char(char),
    /// A non-printable key (Backspace, Enter, Tab, Escape, arrows,
    /// Home/End, Delete, F-keys, etc.). Dispatched as
    /// `WindowEvent::KeyDown(code, Some(key))` so the target view's
    /// own key handling (e.g. textbox's `WindowEvent::KeyDown`
    /// match arm) runs.
    ControlKey(Code, Key),
}

/// State shared between [`ViziaEditor`] (invoked from the host UI
/// thread) and the vizia `on_idle` callback (invoked on the GUI thread).
///
/// The `Editor::on_virtual_key_from_host` callback runs on the host
/// thread and cannot reach into the live vizia `Context`. Instead, it
/// consults `text_focused` (kept in sync from `on_idle`) to decide
/// whether to claim the key, and pushes a [`KeyInject`] into `pending`.
/// The next `on_idle` tick drains the queue and dispatches each entry
/// to the currently focused entity.
pub(crate) struct KeyInjectState {
    /// `true` while the vizia focused view reports element name `"textbox"`.
    /// Updated on every `on_idle` tick.
    text_focused: AtomicBool,
    /// Keys the host delivered via the virtual-key hook that still need
    /// to be dispatched into the focused view on the next idle tick.
    pending: Mutex<VecDeque<KeyInject>>,
}

impl KeyInjectState {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            text_focused: AtomicBool::new(false),
            pending: Mutex::new(VecDeque::new()),
        })
    }
}

/// An [`Editor`] implementation that calls a vizia draw loop.
pub(crate) struct ViziaEditor {
    pub(crate) vizia_state: Arc<ViziaState>,
    /// The user's app function.
    pub(crate) app: Arc<dyn Fn(&mut Context, Arc<dyn GuiContext>) + 'static + Send + Sync>,
    /// What level of theming to apply. See [`ViziaEditorTheming`].
    pub(crate) theming: ViziaTheming,

    /// The scaling factor reported by the host, if any. On macOS this will never be set and we
    /// should use the system scaling factor instead.
    pub(crate) scaling_factor: AtomicCell<Option<f32>>,

    /// Whether to emit a parameters changed event during the next idle callback. This is set in the
    /// `parameter_values_changed()` implementation and it can be used by widgets to explicitly
    /// check for new parameter values. This is useful when the parameter value is (indirectly) used
    /// to compute a property in an event handler. Like when positioning an element based on the
    /// display value's width.
    pub(crate) emit_parameters_changed_event: Arc<AtomicBool>,

    /// Shared registry of `SyncSignal<f32>`s tracking each parameter's live value. Widgets
    /// subscribe via `cx.data::<ParamRegistry>()`; the editor calls `flush_all()` from the
    /// `parameter_value_changed` / `parameter_values_changed` hooks so the reactive graph picks
    /// up value changes that nih-plug reports.
    pub(crate) param_registry: ParamRegistry,

    /// Shared state bridging the host-thread
    /// `on_virtual_key_from_host` callback with the GUI-thread
    /// `on_idle` callback. See [`KeyInjectState`].
    pub(crate) key_inject: Arc<KeyInjectState>,
}

impl Editor for ViziaEditor {
    fn spawn(
        &self,
        parent: ParentWindowHandle,
        context: Arc<dyn GuiContext>,
    ) -> Box<dyn std::any::Any + Send> {
        let app = self.app.clone();
        let vizia_state = self.vizia_state.clone();
        let theming = self.theming;
        let param_registry = self.param_registry.clone();

        let (unscaled_width, unscaled_height) = vizia_state.inner_logical_size();
        let system_scaling_factor = self.scaling_factor.load();
        let user_scale_factor = vizia_state.user_scale_factor();

        let mut application = Application::new(move |cx| {
            // Set some default styles to match the iced integration
            //if theming >= ViziaTheming::Custom {
                // NOTE: `Context::set_default_font` was removed upstream as a deprecated API
                // (vizia commit ff943a0b, "Context: remove deprecated APIs and clarify docs").
                // The default font is now controlled through stylesheets — `theme.css` below
                // can set `* { font-family: ...; }` if a specific font is required.
                if let Err(err) = cx.add_stylesheet(include_style!("src/assets/theme.css")) {
                    nih_error!("Failed to load stylesheet: {err:?}");
                    panic!();
                }

                // There doesn't seem to be any way to bundle styles with a widget, so we'll always
                // include the style sheet for our custom widgets at context creation
                widgets::register_theme(cx);
            //}

            // Install the parameter signal registry so widgets can find it via
            // `cx.data::<ParamRegistry>()`. `ParamRegistry` is a cheap handle (Arc internally),
            // so the editor keeps a clone for flushing on parameter changes.
            param_registry.clone().build(cx);

            // Any widget can change the parameters by emitting `ParamEvent` events. This model will
            // handle them automatically.
            widgets::ParamModel {
                context: context.clone(),
            }
            .build(cx);

            // And we'll link `WindowEvent::ResizeWindow` and `WindowEvent::SetScale` events to our
            // `ViziaState`. We'll notify the host when any of these change.
            let current_inner_window_size = EventContext::new(cx).cache.get_bounds(Entity::root());
            widgets::WindowModel {
                context: context.clone(),
                vizia_state: vizia_state.clone(),
                last_inner_window_size: AtomicCell::new((
                    current_inner_window_size.width() as u32,
                    current_inner_window_size.height() as u32,
                )),
            }
            .build(cx);

            app(cx, context.clone())
        })
        .with_scale_policy(
            system_scaling_factor
                .map(|factor| WindowScalePolicy::ScaleFactor(factor as f64))
                .unwrap_or(WindowScalePolicy::SystemScaleFactor),
        )
        .inner_size((unscaled_width, unscaled_height))
        .user_scale_factor(user_scale_factor)
        .on_idle({
            let emit_parameters_changed_event = self.emit_parameters_changed_event.clone();
            let key_inject = self.key_inject.clone();
            move |cx| {
                // Drain effects queued in `SYNC_RUNTIME` by off-UI-thread signal writes (our
                // `ParamRegistry::flush_all()` runs on nih-plug's parameter-change callback,
                // which is typically the host / audio thread). This ensures `Binding::new`
                // subscribers get their rebuild-on-change notifications processed.
                //
                // Belt-and-suspenders: `vizia_baseview` should ideally call
                // `Runtime::drain_pending_work()` from its own `on_frame_update` (matching
                // what `vizia_winit` already does), in which case this call here becomes a
                // redundant no-op. Until that lands upstream, this guarantees that
                // vizia-plug-backed plugins see reactive updates with at most one event-loop
                // tick of latency.
                //
                // TODO: remove once `vizia_baseview` integrates the sync runtime itself.
                // Tracked by the companion vizia PR — see the vizia-plug PR description.
                Runtime::drain_pending_work();

                if emit_parameters_changed_event
                    .compare_exchange(true, false, Ordering::AcqRel, Ordering::Relaxed)
                    .is_ok()
                {
                    cx.emit_custom(
                        Event::new(RawParamEvent::ParametersChanged)
                            .propagate(Propagation::Subtree),
                    );
                }

                // Keep `text_focused` in sync so the host-thread
                // `on_virtual_key_from_host` callback can decide
                // synchronously whether to claim a key. The element
                // name `"textbox"` is set by
                // `vizia::views::Textbox::element()`.
                key_inject.text_focused.store(
                    cx.focused_element() == Some("textbox"),
                    Ordering::Release,
                );

                // Drain any keys queued by `on_virtual_key_from_host`
                // and dispatch them to the focused view. Buffered
                // inside a short-lived lock to keep the critical
                // section bounded.
                //
                // `on_virtual_key_from_host` has already classified
                // each item on the host thread using the VST3
                // `key_code`, so the drain just mechanically
                // dispatches each variant: chars via
                // `TextEvent::InsertText`, control keys via
                // `WindowEvent::KeyDown` so the focused view's own
                // key handler (e.g. textbox's `KeyDown` match arm)
                // runs.
                let drained: Vec<KeyInject> = {
                    let mut q = key_inject
                        .pending
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    q.drain(..).collect()
                };
                if !drained.is_empty() {
                    let mut ec = EventContext::new(cx);
                    let target = ec.focused();
                    for entry in drained {
                        match entry {
                            KeyInject::Char(c) => {
                                ec.emit_to(target, TextEvent::InsertText(c.to_string()));
                            }
                            KeyInject::ControlKey(code, key) => {
                                ec.emit_to(target, WindowEvent::KeyDown(code, Some(key)));
                            }
                        }
                    }
                }
            }
        });

        // This way the plugin can decide to use none of the built in theming
        if theming == ViziaTheming::None {
            application = application.ignore_default_theme();
        }

        let window = application.open_parented(&parent);

        self.vizia_state.open.store(true, Ordering::Release);
        Box::new(ViziaEditorHandle {
            vizia_state: self.vizia_state.clone(),
            window,
        })
    }

    fn size(&self) -> (u32, u32) {
        // This includes the user scale factor if set, but not any HiDPI scaling
        self.vizia_state.scaled_logical_size()
    }

    fn set_scale_factor(&self, factor: f32) -> bool {
        // If the editor is currently open then the host must not change the current HiDPI scale as
        // we don't have a way to handle that. Ableton Live does this.
        if self.vizia_state.is_open() {
            return false;
        }

        // We're making things a bit more complicated by having both a system scale factor, which is
        // used for HiDPI and also known to the host, and a user scale factor that the user can use
        // to arbitrarily resize the GUI
        self.scaling_factor.store(Some(factor));
        true
    }

    fn param_value_changed(&self, _id: &str, _normalized_value: f32) {
        // Push the new value into the registry's signals — observers bound via `Binding::new`
        // wake up and rebuild. Also flag a `ParametersChanged` idle event for any widgets that
        // still rely on the older (pre-signal) notification path.
        self.param_registry.flush_all();
        self.emit_parameters_changed_event
            .store(true, Ordering::Relaxed);
    }

    fn param_modulation_changed(&self, _id: &str, _modulation_offset: f32) {
        self.param_registry.flush_all();
        self.emit_parameters_changed_event
            .store(true, Ordering::Relaxed);
    }

    fn param_values_changed(&self) {
        self.param_registry.flush_all();
        self.emit_parameters_changed_event
            .store(true, Ordering::Relaxed);
    }

    fn on_virtual_key_from_host(
        &self,
        key_code: VirtualKeyCode,
        is_down: bool,
        modifiers: Modifiers,
    ) -> bool {
        // Called from the host's UI thread (e.g. REAPER dispatching
        // `IPlugView::onKeyDown` / `onKeyUp`). Claim the key only when
        // a textbox is currently focused; otherwise the host's
        // accelerator (e.g. space -> transport) should run normally.
        if !self.key_inject.text_focused.load(Ordering::Acquire) {
            return false;
        }

        // Modifier-held combinations (Cmd+A, Cmd+Left, Shift+Arrow,
        // Option+Backspace, etc.) are claimed by the host or handled by
        // AppKit's `keyDown:` + `doCommandBySelector:` path where
        // vizia's textbox reads modifier state for line/word movement.
        // Dispatching through our injection queue here would double-fire
        // and lose modifier context. Return `false` so the host keeps
        // the key and AppKit's normal path runs.
        if !modifiers.is_empty() {
            return false;
        }

        // Classify the virtual key. The host hands us virtual keys that
        // split into two groups vizia consumes differently:
        //
        // - Keys that represent a printable character (Space, numpad
        //   digits/operators, `=`) go in as `TextEvent::InsertText`.
        // - Named control keys (Backspace, Enter, arrows, F-keys,
        //   etc.) go in as `WindowEvent::KeyDown(code, Some(key))` so
        //   the focused view's own key handler (textbox's `KeyDown`
        //   match arm for Backspace / Enter / arrows) runs.
        //
        // Keys we don't enumerate here (media / volume keys, Select,
        // Print, modifier-only presses, Super) fall through to
        // `return false` so the host's own binding runs.
        let inject = match key_code {
            VirtualKeyCode::Space => Some(KeyInject::Char(' ')),
            VirtualKeyCode::Numpad0 => Some(KeyInject::Char('0')),
            VirtualKeyCode::Numpad1 => Some(KeyInject::Char('1')),
            VirtualKeyCode::Numpad2 => Some(KeyInject::Char('2')),
            VirtualKeyCode::Numpad3 => Some(KeyInject::Char('3')),
            VirtualKeyCode::Numpad4 => Some(KeyInject::Char('4')),
            VirtualKeyCode::Numpad5 => Some(KeyInject::Char('5')),
            VirtualKeyCode::Numpad6 => Some(KeyInject::Char('6')),
            VirtualKeyCode::Numpad7 => Some(KeyInject::Char('7')),
            VirtualKeyCode::Numpad8 => Some(KeyInject::Char('8')),
            VirtualKeyCode::Numpad9 => Some(KeyInject::Char('9')),
            VirtualKeyCode::NumpadMultiply => Some(KeyInject::Char('*')),
            VirtualKeyCode::NumpadAdd => Some(KeyInject::Char('+')),
            VirtualKeyCode::NumpadSeparator => Some(KeyInject::Char(',')),
            VirtualKeyCode::NumpadSubtract => Some(KeyInject::Char('-')),
            VirtualKeyCode::NumpadDecimal => Some(KeyInject::Char('.')),
            VirtualKeyCode::NumpadDivide => Some(KeyInject::Char('/')),
            VirtualKeyCode::Equals => Some(KeyInject::Char('=')),

            VirtualKeyCode::Backspace => {
                Some(KeyInject::ControlKey(Code::Backspace, Key::Backspace))
            }
            VirtualKeyCode::Tab => Some(KeyInject::ControlKey(Code::Tab, Key::Tab)),
            VirtualKeyCode::Return => Some(KeyInject::ControlKey(Code::Enter, Key::Enter)),
            VirtualKeyCode::NumpadEnter => {
                Some(KeyInject::ControlKey(Code::NumpadEnter, Key::Enter))
            }
            VirtualKeyCode::Pause => Some(KeyInject::ControlKey(Code::Pause, Key::Pause)),
            VirtualKeyCode::Escape => Some(KeyInject::ControlKey(Code::Escape, Key::Escape)),
            VirtualKeyCode::End => Some(KeyInject::ControlKey(Code::End, Key::End)),
            VirtualKeyCode::Home => Some(KeyInject::ControlKey(Code::Home, Key::Home)),
            VirtualKeyCode::ArrowLeft => {
                Some(KeyInject::ControlKey(Code::ArrowLeft, Key::ArrowLeft))
            }
            VirtualKeyCode::ArrowUp => Some(KeyInject::ControlKey(Code::ArrowUp, Key::ArrowUp)),
            VirtualKeyCode::ArrowRight => {
                Some(KeyInject::ControlKey(Code::ArrowRight, Key::ArrowRight))
            }
            VirtualKeyCode::ArrowDown => {
                Some(KeyInject::ControlKey(Code::ArrowDown, Key::ArrowDown))
            }
            VirtualKeyCode::PageUp => Some(KeyInject::ControlKey(Code::PageUp, Key::PageUp)),
            VirtualKeyCode::PageDown => Some(KeyInject::ControlKey(Code::PageDown, Key::PageDown)),
            VirtualKeyCode::Insert => Some(KeyInject::ControlKey(Code::Insert, Key::Insert)),
            VirtualKeyCode::Delete => Some(KeyInject::ControlKey(Code::Delete, Key::Delete)),
            VirtualKeyCode::F1 => Some(KeyInject::ControlKey(Code::F1, Key::F1)),
            VirtualKeyCode::F2 => Some(KeyInject::ControlKey(Code::F2, Key::F2)),
            VirtualKeyCode::F3 => Some(KeyInject::ControlKey(Code::F3, Key::F3)),
            VirtualKeyCode::F4 => Some(KeyInject::ControlKey(Code::F4, Key::F4)),
            VirtualKeyCode::F5 => Some(KeyInject::ControlKey(Code::F5, Key::F5)),
            VirtualKeyCode::F6 => Some(KeyInject::ControlKey(Code::F6, Key::F6)),
            VirtualKeyCode::F7 => Some(KeyInject::ControlKey(Code::F7, Key::F7)),
            VirtualKeyCode::F8 => Some(KeyInject::ControlKey(Code::F8, Key::F8)),
            VirtualKeyCode::F9 => Some(KeyInject::ControlKey(Code::F9, Key::F9)),
            VirtualKeyCode::F10 => Some(KeyInject::ControlKey(Code::F10, Key::F10)),
            VirtualKeyCode::F11 => Some(KeyInject::ControlKey(Code::F11, Key::F11)),
            VirtualKeyCode::F12 => Some(KeyInject::ControlKey(Code::F12, Key::F12)),

            _ => None,
        };

        let Some(entry) = inject else {
            return false;
        };

        // Vizia's text-input model is press-driven: TextEvent::InsertText
        // and the textbox's KeyDown handlers run on the press only. Push
        // the queued event on key-down; on key-up, just claim the event
        // so the host doesn't pick the release up as a separate
        // accelerator (BillyDM's reasoning on nih-plug#9).
        if is_down {
            self.key_inject
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push_back(entry);
        }
        true
    }
}

/// The window handle used for [`ViziaEditor`].
struct ViziaEditorHandle {
    vizia_state: Arc<ViziaState>,
    window: WindowHandle,
}

/// The window handle enum stored within 'WindowHandle' contains raw pointers. Is there a way around
/// having this requirement?
unsafe impl Send for ViziaEditorHandle {}

impl Drop for ViziaEditorHandle {
    fn drop(&mut self) {
        self.vizia_state.open.store(false, Ordering::Release);
        // XXX: This should automatically happen when the handle gets dropped, but apparently not
        self.window.close();
    }
}
