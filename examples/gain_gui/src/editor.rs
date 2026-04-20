use atomic_float::AtomicF32;
use nih_plug::prelude::{util, Editor};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use vizia_plug::vizia::prelude::*;
use vizia_plug::widgets::*;
use vizia_plug::{create_vizia_editor, ViziaState, ViziaTheming};

use crate::GainParams;

pub const NOTO_SANS: &str = "Noto Sans";

/// Interval at which the UI polls the peak-meter atomic that the audio thread writes into.
/// 50 Hz is smooth enough for a meter and cheap.
const PEAK_METER_POLL_INTERVAL: Duration = Duration::from_millis(20);

// Makes sense to also define this here — easier to keep track of.
pub(crate) fn default_state() -> Arc<ViziaState> {
    ViziaState::new(|| (200, 150))
}

pub(crate) fn create(
    params: Arc<GainParams>,
    peak_meter: Arc<AtomicF32>,
    editor_state: Arc<ViziaState>,
) -> Option<Box<dyn Editor>> {
    create_vizia_editor(editor_state, ViziaTheming::Custom, move |cx, _| {
        // Bridge from the audio thread's `AtomicF32` peak-meter into a `SyncSignal<f32>` the
        // UI can bind to. The audio thread writes the atomic; we poll it on a short vizia
        // `Timer` and push the converted dBFS value into the signal. The `PeakMeter` widget
        // then uses the signal's `SignalGet` implementation to drive its bar and hold-peak
        // display.
        let level_dbfs: SyncSignal<f32> = SyncSignal::new(util::MINUS_INFINITY_DB);
        let poll_target = peak_meter.clone();
        let timer = cx.add_timer(PEAK_METER_POLL_INTERVAL, None, move |_cx, reason| {
            if matches!(reason, TimerAction::Tick(_)) {
                let raw = poll_target.load(Ordering::Relaxed);
                level_dbfs.set_if_changed(util::gain_to_db(raw));
            }
        });
        cx.start_timer(timer);

        VStack::new(cx, |cx| {
            Label::new(cx, "Gain GUI")
                .font_family(vec![FamilyOwned::Named(String::from(NOTO_SANS))])
                .font_weight(FontWeightKeyword::Light)
                .font_size(30.0)
                .height(Pixels(50.0))
                .alignment(Alignment::BottomCenter);

            Label::new(cx, "Gain");
            ParamSlider::new(cx, &params.gain);

            PeakMeter::new(cx, level_dbfs, Some(Duration::from_millis(600)));
        })
        .alignment(Alignment::TopCenter);
    })
}
