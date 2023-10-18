use nih_plug::prelude::Editor;
use nih_plug_vizia::vizia::prelude::*;
use nih_plug_vizia::{create_vizia_editor, ViziaState, ViziaTheming};
use std::sync::Arc;

use crate::ViziaPlugParams;

pub(crate) fn create(_params: Arc<ViziaPlugParams>) -> Option<Box<dyn Editor>> {
    create_vizia_editor(
        ViziaState::new(|| (200, 150)),
        ViziaTheming::Custom,
        move |cx, _| {
            Label::new(cx, "Hello Plugin GUI");
        },
    )
}
