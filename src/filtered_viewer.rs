use std::{collections::BTreeMap, ops::Range, path::PathBuf, vec};

use egui::Color32;
use egui_plot::{Legend, MarkerShape, PlotUi, Points, VLine};

use processing::{
    postprocess::{post_process_frame, PostProcessParams},
    process::{
        convert_to_kev, extract_waveforms, frame_to_events, ProcessParams, StaticProcessParams,
    },
    storage::load_point,
    types::{FrameEvent, NumassFrame, NumassWaveforms, ProcessedWaveform},
    utils::{color_for_index, EguiLine},
};

use processing::widgets::UserInput;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
extern "C" {
    fn download(filename: &str, text: &str);
}

pub struct FilteredViewer<'a> {
    process: ProcessParams,
    postprocess: PostProcessParams,
    range: Range<f32>,
    waveforms: NumassWaveforms<'a>,
    static_params: StaticProcessParams,
    indexes: Option<Vec<u64>>,
    current: usize,
}

impl<'a> FilteredViewer<'a> {
    pub async fn init_with_point(
        filepath: PathBuf,
        process: ProcessParams,
        postprocess: PostProcessParams,
        range: Range<f32>,
    ) -> Self {
        let (waveforms, static_params) = {
            let point = load_point(&filepath).await;
            let point = Box::leak::<'a>(Box::new(point)); // TODO: set lifetime properly
            let static_params = StaticProcessParams::from_point(point, &process.algorithm);
            (extract_waveforms(point), static_params)
        };

        let mut viewer = Self {
            process,
            postprocess,
            range,
            waveforms,
            indexes: None,
            static_params,
            current: 0,
        };

        viewer.update_indexes();
        viewer
    }

    fn update_indexes(&mut self) {
        self.current = 0;

        let mut new_indexes = vec![];

        for (time, frame) in &self.waveforms {
            let events = post_process_frame(
                frame_to_events(
                    frame,
                    &self.process.algorithm,
                    &self.static_params,
                    &mut None,
                ),
                &self.postprocess,
                None,
            );

            for (_, event) in events {
                if let FrameEvent::Event {
                    channel, amplitude, ..
                } = event
                {
                    let amplitude = if self.process.convert_to_kev {
                        convert_to_kev(&amplitude, channel, &self.process.algorithm)
                    } else {
                        amplitude
                    };
                    if self.range.contains(&amplitude) {
                        new_indexes.push(*time);
                        break;
                    }
                }
            }
        }

        self.indexes = Some(new_indexes);
    }

    fn plot_processed_frame(
        current: usize,
        process: &ProcessParams,
        postprocess: &PostProcessParams,
        plot_ui: &mut PlotUi,
        indexes: &[u64],
        static_params: &StaticProcessParams,
        waveforms: &BTreeMap<u64, NumassFrame<'a>>,
    ) {
        let frame = {
            let current_time = indexes[current];
            waveforms.get(&current_time).unwrap().clone()
        };

        let mut events = frame_to_events(
            &frame,
            &process.algorithm,
            static_params,
            &mut Some(plot_ui),
        );

        events
            .iter()
            .enumerate()
            .for_each(|(idx, (pos, event))| match event {
                FrameEvent::Event {
                    channel,
                    amplitude,
                    size,
                } => {
                    let ch = channel + 1;
                    let name = format!("ev#{idx} ch# {ch}");

                    plot_ui.vline(
                        VLine::new((*pos as f64) / 8.0)
                            .color(color_for_index(*channel as usize))
                            .name(name.clone()),
                    );
                    plot_ui.vline(
                        VLine::new((*pos + *size * 8) as f64 / 8.0)
                            .color(color_for_index(*channel as usize))
                            .name(name.clone()),
                    );
                    plot_ui.points(
                        Points::new(vec![[*pos as f64 / 8.0, *amplitude as f64]])
                            .color(color_for_index(*channel as usize))
                            .shape(MarkerShape::Diamond)
                            .filled(false)
                            .radius(10.0)
                            .name(name),
                    );
                }
                FrameEvent::Reset { size } => {
                    plot_ui.vline(
                        VLine::new(*pos as f64 / 8.0)
                            .color(Color32::WHITE)
                            .name("RESET"),
                    );
                    plot_ui.vline(
                        VLine::new((*pos + *size * 8) as f64 / 8.0)
                            .color(Color32::WHITE)
                            .name("RESET"),
                    );
                }
                FrameEvent::Overflow { channel, size } => {
                    plot_ui.vline(
                        VLine::new(*pos as f64 / 8.0)
                            .color(color_for_index(*channel as usize))
                            .style(egui_plot::LineStyle::Dashed { length: 10.0 })
                            .name(format!("OVERFLOW ch# {}", channel)),
                    );
                    plot_ui.vline(
                        VLine::new((*pos + *size * 8) as f64 / 8.0)
                            .color(color_for_index(*channel as usize))
                            .style(egui_plot::LineStyle::Dashed { length: 10.0 })
                            .name(format!("OVERFLOW ch# {}", channel)),
                    );
                }
                FrameEvent::Frame { .. } => {}
            });

        events = post_process_frame(events, postprocess, Some(plot_ui));

        // TODO: dont need conversion since we dont plot it
        if process.convert_to_kev {
            events.iter_mut().for_each(|(_, event)| {
                if let FrameEvent::Event {
                    channel: ch_id,
                    amplitude,
                    ..
                } = event
                {
                    *amplitude = convert_to_kev(amplitude, *ch_id, &process.algorithm)
                }
            });
        }

        for (ch_id, waveform) in frame {
            ProcessedWaveform::from(waveform).draw_egui(
                plot_ui,
                Some(&format!("ch# {}", ch_id + 1)),
                Some(color_for_index(ch_id as usize)),
                Some(1.0),
                Some(0),
            );
        }
    }

    fn inc(&mut self) {
        if let Some(len) = self.indexes.as_mut().map(|indexes| indexes.len()) {
            if self.current < len - 1 {
                self.current += 1
            }
        }
    }

    fn dec(&mut self) {
        if self.indexes.is_some() && self.current != 0 {
            self.current -= 1
        }
    }
}

impl<'a> eframe::App for FilteredViewer<'a> {
    #[allow(unused_variables)]
    fn update(&mut self, ctx: &eframe::egui::Context, frame: &mut eframe::Frame) {
        let indexes_len = self.indexes.as_ref().map(|indexes| indexes.len());

        ctx.input(|i| {
            if i.key_pressed(eframe::egui::Key::ArrowLeft) {
                self.dec()
            }
            if i.key_pressed(eframe::egui::Key::ArrowRight) {
                self.inc()
            }
        });

        eframe::egui::SidePanel::left("parameters").show(ctx, |ui| {
            self.process = self.process.input(ui, ctx);

            ui.separator();

            self.postprocess = self.postprocess.input(ui, ctx);

            ui.separator();
            let mut min = self.range.start;
            ui.add(egui::Slider::new(&mut min, -10.0..=400.0).text("left"));
            let mut max = self.range.end;
            ui.add(egui::Slider::new(&mut max, -10.0..=400.0).text("right"));
            self.range = min..max;

            if ui.button("apply").clicked() {
                self.update_indexes();
            }
        });

        eframe::egui::TopBottomPanel::top("position").show(ctx, |ui| {
            ui.horizontal(|ui| {
                #[cfg(not(target_arch = "wasm32"))]
                let width = {
                    let mut x = 0.0;
                    ctx.input(|i| x = i.viewport().inner_rect.unwrap().size().x);
                    x
                };
                #[cfg(target_arch = "wasm32")]
                let width = eframe::web_sys::window()
                    .unwrap()
                    .inner_width()
                    .unwrap()
                    .as_f64()
                    .unwrap() as f32;

                ui.style_mut().spacing.slider_width = width - 450.0;

                if let Some(len) = indexes_len {
                    ui.add(eframe::egui::Slider::new(&mut self.current, 0..=len - 1).step_by(1.0));
                    if ui.button("<").clicked() {
                        self.dec();
                    }
                    if ui.button(">").clicked() {
                        self.inc();
                    }
                }

                if let Some(indexes) = self.indexes.as_ref() {
                    ui.label(format!("{:.3} ms", indexes[self.current] as f64 / 1e6));
                }
            })
        });

        eframe::egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(indexes) = self.indexes.as_ref() {
                egui_plot::Plot::new("waveforms")
                    .legend(Legend::default())
                    .x_axis_formatter(|mark, _, _| format!("{:.3} μs", (mark.value * 8.0) / 1000.0))
                    .show(ui, |plot_ui| {
                        if indexes.is_empty() {
                            return;
                        }

                        let position = indexes[self.current];

                        FilteredViewer::plot_processed_frame(
                            self.current,
                            &self.process,
                            &self.postprocess,
                            plot_ui,
                            indexes,
                            &self.static_params,
                            &self.waveforms,
                        );
                    });
            } else {
                ui.spinner();
            }
        });
    }
}
