use std::{collections::BTreeMap, ops::Range, path::PathBuf, sync::Arc, vec};

use egui_plot::{Legend, PlotUi};
use egui::mutex::Mutex;

use processing::{
    process::{convert_to_kev, extract_waveforms, frame_to_events, ProcessParams, StaticProcessParams}, storage::load_point, types::{FrameEvent, NumassWaveforms, ProcessedWaveform}, utils::{color_for_index, EguiLine} 
};

use processing::widgets::UserInput;

#[cfg(not(target_arch = "wasm32"))]
use tokio::spawn;

#[cfg(target_arch = "wasm32")]
use {
    wasm_bindgen::prelude::*, wasm_bindgen_futures::spawn_local as spawn,
};

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
extern "C" {
    fn download(filename: &str, text: &str);
}

#[derive(Debug, Clone)]
enum AppState {
    Initializing,
    FirstLoad,
    Interactive
}

pub struct FilteredViewer {
    processing: ProcessParams,
    range: Range<f32>,
    waveforms: Arc<Mutex<Option<NumassWaveforms>>>,
    indexes: Arc<Mutex<Option<Vec<u64>>>>,
    state: Arc<Mutex<AppState>>,
    static_params: Arc<Mutex<StaticProcessParams>>,
    current: usize,
}

impl FilteredViewer {
    pub fn init_with_point(
        filepath: PathBuf,
        processing: ProcessParams,
        range: Range<f32>,
    ) -> Self {

        let viewer = Self {
            processing,
            range,
            waveforms: Arc::new(Mutex::new(None)),
            indexes: Arc::new(Mutex::new(None)),
            state: Arc::new(Mutex::new(AppState::Initializing)),
            static_params: Arc::new(Mutex::new(StaticProcessParams { baseline: None })),
            current: 0,
        };

        let waveforms = Arc::clone(&viewer.waveforms);
        let state = Arc::clone(&viewer.state);
        let static_params = Arc::clone(&viewer.static_params);

        spawn(async move {
            let point = load_point(&filepath).await;
            let loaded_waveforms = extract_waveforms(&point);

            *waveforms.lock() = Some(loaded_waveforms);
            *state.lock() = AppState::FirstLoad;
            *static_params.lock() = StaticProcessParams::from_point(&point);
        });

        viewer
    }

    fn update_indexes(&mut self) {
        // TODO: avoid cloning

        let indexes = Arc::clone(&self.indexes);
        let waveforms = Arc::clone(&self.waveforms);
        let processing = self.processing.clone();
        let static_params = Arc::clone(&self.static_params);
        let range = self.range.clone();

        self.current = 0;

        spawn(async move {

            let waveforms = waveforms.lock().clone().unwrap();
            let static_params = static_params.lock().clone();
            let mut new_indexes = vec![];

            for (time, frame) in waveforms {

                let events = frame_to_events(
                    &frame,
                    &processing.algorithm, 
                    &static_params,
                    None
                );

                for (_, event) in events {
                    if let FrameEvent::Event {channel, amplitude, .. } = event {
                        let amplitude = if processing.convert_to_kev {
                            convert_to_kev(&amplitude, channel, &processing.algorithm)
                        } else {
                            amplitude
                        };
                        if range.contains(&amplitude) {
                            new_indexes.push(time);
                            break;
                        }
                    }
                }
            }

            *indexes.lock() = Some(new_indexes);
        });
    }

    fn plot_processed_frame(
        current: usize,
        processing: &ProcessParams,
        plot_ui: &mut PlotUi,
        indexes: &[u64],
        static_params: &StaticProcessParams,
        waveforms: &BTreeMap<u64, BTreeMap<u8, ProcessedWaveform>>) {

        let frame = {
            let current_time = indexes[current];
            waveforms.get(&current_time).unwrap().clone()
        };

        let mut events = frame_to_events(
            &frame, 
            &processing.algorithm,
            static_params,
            Some(plot_ui)
        );

        if processing.convert_to_kev {
            events.iter_mut().for_each(|(_, event)| {
                if let FrameEvent::Event { channel: ch_id, amplitude , ..} = event {
                    *amplitude = convert_to_kev(amplitude, *ch_id, &processing.algorithm)
                }
            });
        }

        for (ch_id, waveform) in frame {
            waveform.clone().draw_egui(
                plot_ui, 
                Some(&format!("ch# {}", ch_id + 1)), 
                Some(color_for_index(ch_id as usize)), 
                Some(1.0), 
                Some(0)
            );
        }
    }

    fn inc(&mut self) {
        if let Some(len) = self.indexes.lock().as_ref().map(|indexes| indexes.len()) {
            if self.current < len - 1 {
                self.current += 1
            }
        }
    }

    fn dec(&mut self) {
        if self.indexes.lock().is_some() && self.current != 0 {
            self.current -= 1
        }
    }

}

impl eframe::App for FilteredViewer {

    #[allow(unused_variables)]
    fn update(&mut self, ctx: &eframe::egui::Context, frame: &mut eframe::Frame) {
    
        {
            let state = self.state.lock().clone();
            if let AppState::FirstLoad = state {
                *self.state.lock() = AppState::Interactive;
                self.update_indexes();
            }
        }

        let indexes_len = self.indexes.lock().as_ref().map(|indexes| {
            indexes.len()
        });

        ctx.input(|i| {
            if i.key_pressed(eframe::egui::Key::ArrowLeft) {
                self.dec()
            }
            if i.key_pressed(eframe::egui::Key::ArrowRight) {
                self.inc()
            }
        });

        eframe::egui::SidePanel::left("parameters").show(ctx, |ui| {

            self.processing =  self.processing.input(ui, ctx);

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
                    ctx.input(|i| {x = i.viewport().inner_rect.unwrap().size().x});
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
                    ui.add(
                        eframe::egui::Slider::new(&mut self.current, 0..=len - 1)
                            .step_by(1.0),
                    );
                    if ui.button("<").clicked() 
                    {
                        self.dec();
                    }
                    if ui.button(">").clicked() 
                    {
                        self.inc();
                    }
                }

                if let Some(waveforms) = self.waveforms.lock().as_ref() {
                    if let Some(indexes) = self.indexes.lock().as_ref() {
                        ui.label(format!("{:.3} ms", indexes[self.current] as f64 / 1e6));    
                    }
                }
            })
        });

        eframe::egui::CentralPanel::default().show(ctx, |ui| {

            if let Some(indexes) = self.indexes.lock().as_ref() {
                if let Some(waveforms) = self.waveforms.lock().as_ref() {
                    egui_plot::Plot::new("waveforms").legend(Legend::default())
                    .x_axis_formatter(|mark, _, _| format!("{:.3} μs", (mark.value * 8.0) / 1000.0))
                    .show(ui, |plot_ui| {

                        if indexes.is_empty() {
                            return;
                        }

                        let position = indexes[self.current];

                        FilteredViewer::plot_processed_frame(
                            self.current,
                            &self.processing,
                            plot_ui,
                            indexes,
                            &self.static_params.lock(),
                            waveforms);
                    });

                } else {
                    ui.spinner();
                }
            } else {
                ui.spinner();
            }
        });
    }
}
