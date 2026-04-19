//! A super simple peak meter widget.

use nih_plug::prelude::util;
use std::cell::Cell;
use std::time::Duration;
use std::time::Instant;
use vizia::prelude::*;
use vizia::vg;

/// The thickness of a tick inside of the peak meter's bar.
const TICK_WIDTH: f32 = 1.0;
/// The gap between individual ticks.
const TICK_GAP: f32 = 1.0;

/// The decibel value corresponding to the very left of the bar.
const MIN_TICK: f32 = -90.0;
/// The decibel value corresponding to the very right of the bar.
const MAX_TICK: f32 = 20.0;
/// The ticks shown beneath the peak meter's bar. The first value is shown as -infinity, and at
/// the last position we draw the `dBFS` string.
const TEXT_TICKS: [i32; 6] = [-80, -60, -40, -20, 0, 12];

/// How often the bar repaints to drive wall-clock-based hold-peak decay when the level signal
/// itself isn't updating (e.g. the source has gone silent). 20 Hz is fast enough to feel
/// responsive at typical hold times (~600 ms) and cheap enough to ignore.
const DECAY_TICK_INTERVAL: Duration = Duration::from_millis(50);

/// A simple horizontal peak meter.
///
/// TODO: There are currently no styling options at all.
/// TODO: Vertical peak meter — this is just a proof of concept to fit the gain GUI example.
pub struct PeakMeter;

/// The bar bit for the peak meter, manually drawn using vertical lines.
///
/// Holds the current level signal, the optional hold-time, and (for the hold-peak display)
/// two `Cell`s tracking the latched peak value and the instant it was latched. The hold
/// computation lives in [`View::draw`] so it runs off a wall clock — that way the held peak
/// decays after `hold_time` even when the source signal stops updating (e.g. silent audio).
struct PeakMeterBar {
    level_dbfs: SyncSignal<f32>,
    hold_time: Option<Duration>,
    held_peak_dbfs: Cell<f32>,
    last_held_at: Cell<Option<Instant>>,
}

impl PeakMeter {
    /// Creates a new [`PeakMeter`] reading from the given dBFS signal. If `hold_time` is set,
    /// the peak value is latched for that duration before decaying.
    ///
    /// Typical setup: an `Arc<AtomicF32>` (or similar) updated by the audio thread, mirrored
    /// into a `SyncSignal<f32>` from a periodic UI-side tick. See the `gain_gui` example.
    pub fn new(
        cx: &mut Context,
        level_dbfs: SyncSignal<f32>,
        hold_time: Option<Duration>,
    ) -> Handle<'_, Self> {
        Self.build(cx, move |cx| {
            // `PeakMeterBar` paints directly from the `level_dbfs` signal (read inside
            // `draw()`). A signal read from `draw()` does *not* register the view for
            // redraws, so we explicitly bind to `level_dbfs` for the "level changed →
            // repaint" path.
            let bar = PeakMeterBar {
                level_dbfs,
                hold_time,
                held_peak_dbfs: Cell::new(f32::MIN),
                last_held_at: Cell::new(None),
            }
            .build(cx, |_| {})
            .class("bar")
            .bind(level_dbfs, |mut handle| handle.needs_redraw());

            let bar_entity = bar.entity();

            // When `hold_time` is set, the held-peak value decays on wall-clock time. If the
            // source goes silent, `level_dbfs` stops changing and the `bind` above stops
            // triggering repaints — the held peak would stay stuck forever without this
            // timer nudging a repaint periodically. `draw()` reads `Instant::now()` and
            // expires the hold on its own.
            if hold_time.is_some() {
                let decay_timer = cx.add_timer(DECAY_TICK_INTERVAL, None, move |cx, action| {
                    if matches!(action, TimerAction::Tick(_)) {
                        cx.with_current(bar_entity, |cx| cx.needs_redraw());
                    }
                });
                cx.start_timer(decay_timer);
            }

            HStack::new(cx, |cx| {
                for tick_db in TEXT_TICKS {
                    let first_tick = tick_db == TEXT_TICKS[0];
                    let last_tick = tick_db == TEXT_TICKS[TEXT_TICKS.len() - 1];
                    VStack::new(cx, |cx| {
                        if !last_tick {
                            Element::new(cx).class("ticks__tick");
                        }

                        if first_tick {
                            Label::new(cx, "-inf")
                                .class("ticks__label")
                                .class("ticks__label--inf")
                        } else if last_tick {
                            // Only in the array to make positioning easier.
                            Label::new(cx, "dBFS")
                                .class("ticks__label")
                                .class("ticks__label--dbfs")
                        } else {
                            Label::new(cx, tick_db.to_string()).class("ticks__label")
                        };
                    })
                    .width(Auto)
                    .alignment(Alignment::TopCenter);

                    if !last_tick {
                        Spacer::new(cx);
                    }
                }
            })
            .class("ticks");
        })
    }
}

impl View for PeakMeter {
    fn element(&self) -> Option<&'static str> {
        Some("peak-meter")
    }
}

impl PeakMeterBar {
    /// Compute the current hold-peak dBFS value. Called from `draw()` — runs on the UI
    /// thread, uses wall-clock time, mutates the `Cell` state in place.
    fn update_held_peak(&self, current_level_dbfs: f32) -> f32 {
        let Some(hold_time) = self.hold_time else {
            // No hold configured — the display shows `MINUS_INFINITY_DB` as a sentinel so
            // the draw path can skip drawing the hold tick.
            return util::MINUS_INFINITY_DB;
        };

        let now = Instant::now();
        let mut held = self.held_peak_dbfs.get();
        let held_at = self.last_held_at.get();

        if current_level_dbfs >= held
            || held_at.is_none()
            || now > held_at.unwrap() + hold_time
        {
            held = current_level_dbfs;
            self.held_peak_dbfs.set(held);
            self.last_held_at.set(Some(now));
        }

        held
    }
}

impl View for PeakMeterBar {
    fn element(&self) -> Option<&'static str> {
        Some("peak-meter-bar")
    }

    fn draw(&self, cx: &mut DrawContext, canvas: &Canvas) {
        let level_dbfs = self.level_dbfs.get();
        let peak_dbfs = self.update_held_peak(level_dbfs);

        // These basics are taken directly from the default implementation of this function.
        let bounds = cx.bounds();
        if bounds.w == 0.0 || bounds.h == 0.0 {
            return;
        }

        // TODO: It would be nice to let the text colour property drive the gradient here. For
        //       now we only support basic background colours and borders.
        let background_color = cx.background_color();
        let border_color = cx.border_color();
        let border_width = cx.border_width();

        let mut path = vg::PathBuilder::new();
        {
            let x = bounds.x + border_width / 2.0;
            let y = bounds.y + border_width / 2.0;
            let w = bounds.w - border_width;
            let h = bounds.h - border_width;
            path.move_to((x, y));
            path.line_to((x, y + h));
            path.line_to((x + w, y + h));
            path.line_to((x + w, y));
            path.line_to((x, y));
            path.close();
        }

        // Fill with background colour.
        let mut paint = vg::Paint::default();
        paint.set_color(background_color);
        canvas.draw_path(&path.snapshot(), &paint);

        // Now the fun stuff. Try not to overlap the border, but draw that last just in case.
        let bar_bounds = bounds.shrink(border_width / 2.0);
        let bar_ticks_start_x = bar_bounds.left().floor() as i32;
        let bar_ticks_end_x = bar_bounds.right().ceil() as i32;

        // NOTE: We scale this with the nearest integer DPI ratio. That way it still looks good
        //       at 2× scaling and isn't blurry at 1.x× scaling.
        let dpi_scale = cx.logical_to_physical(1.0).floor().max(1.0);
        let bar_tick_coordinates = (bar_ticks_start_x..bar_ticks_end_x)
            .step_by(((TICK_WIDTH + TICK_GAP) * dpi_scale).round() as usize);
        for tick_x in bar_tick_coordinates {
            let tick_fraction = (tick_x - bar_ticks_start_x) as f32
                / (bar_ticks_end_x - bar_ticks_start_x) as f32;
            let tick_db = (tick_fraction * (MAX_TICK - MIN_TICK)) + MIN_TICK;
            if tick_db > level_dbfs {
                break;
            }

            // femtovg draws paths centred on these coordinates, so for pixel-perfect rendering
            // we need to account for that — otherwise the ticks will be 2px wide instead of 1px.
            let mut path = vg::PathBuilder::new();
            path.move_to((tick_x as f32 + (dpi_scale / 2.0), bar_bounds.top()));
            path.line_to((tick_x as f32 + (dpi_scale / 2.0), bar_bounds.bottom()));

            let grayscale_color = 0.3 + ((1.0 - tick_fraction) * 0.5);
            let mut paint = vg::Paint::default();
            paint.set_color4f(
                vg::Color4f::new(grayscale_color, grayscale_color, grayscale_color, 1.0),
                None,
            );
            paint.set_stroke_width(TICK_WIDTH * dpi_scale);
            paint.set_style(vg::PaintStyle::Stroke);
            canvas.draw_path(&path.snapshot(), &paint);
        }

        // Draw the hold peak value if the hold time option was set.
        let db_to_x_coord = |db: f32| {
            let tick_fraction = (db - MIN_TICK) / (MAX_TICK - MIN_TICK);
            bar_ticks_start_x as f32
                + ((bar_ticks_end_x - bar_ticks_start_x) as f32 * tick_fraction).round()
        };
        if (MIN_TICK..MAX_TICK).contains(&peak_dbfs) {
            let peak_x = db_to_x_coord(peak_dbfs);
            let mut path = vg::PathBuilder::new();
            path.move_to((peak_x + (dpi_scale / 2.0), bar_bounds.top()));
            path.line_to((peak_x + (dpi_scale / 2.0), bar_bounds.bottom()));

            let mut paint = vg::Paint::default();
            paint.set_color4f(vg::Color4f::new(0.3, 0.3, 0.3, 1.0), None);
            paint.set_stroke_width(TICK_WIDTH * dpi_scale);
            paint.set_style(vg::PaintStyle::Stroke);
            canvas.draw_path(&path.snapshot(), &paint);
        }

        // Draw border last.
        let mut paint = vg::Paint::default();
        paint.set_color(border_color);
        paint.set_stroke_width(border_width);
        paint.set_style(vg::PaintStyle::Stroke);
        canvas.draw_path(&path.snapshot(), &paint);
    }
}
