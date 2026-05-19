use atomic_float::AtomicF32;
use nice_plug::prelude::{util, Editor};
use std::sync::Arc;
use std::time::Duration;
use vizia_plug::vizia::prelude::*;
use vizia_plug::widgets::*;
use vizia_plug::{create_vizia_editor, ViziaState, ViziaTheming};

use crate::GainParams;

pub const NOTO_SANS: &str = "Noto Sans";

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
        // Read directly from the shared peak-meter atomic inside the widget's draw path.
        // This avoids editor-local timer callbacks that can be sensitive to host behavior.
        let meter_source = {
            let peak_meter = peak_meter.clone();
            move || util::gain_to_db(peak_meter.load(std::sync::atomic::Ordering::Relaxed))
        };

        VStack::new(cx, |cx| {
            Label::new(cx, "Gain GUI")
                .font_family(vec![FamilyOwned::Named(String::from(NOTO_SANS))])
                .font_weight(FontWeightKeyword::Light)
                .font_size(30.0)
                .height(Pixels(50.0))
                .alignment(Alignment::BottomCenter);

            Label::new(cx, "Gain");
            ParamSlider::new(cx, &params.gain);

            PeakMeter::new_with_getter(cx, meter_source, Some(Duration::from_millis(600)));
        })
        .alignment(Alignment::TopCenter);
    })
}
