use std::{ops::Range, path::PathBuf, sync::Arc, collections::BTreeMap};

use egui::{mutex::Mutex, plot::{PlotUi, Points, MarkerShape}};
use serde::Serialize;
use serde_json::json;

use processing::{
    process::{convert_to_kev, process_waveform, waveform_to_events, ProcessParams}, 
    types::ProcessedWaveform, utils::{color_for_index, EguiLine}, 
    storage::load_point 
};
use crate::widgets::process_editor;

#[cfg(not(target_arch = "wasm32"))]
use {
    tokio::spawn,
    home::home_dir,
};

#[cfg(target_arch = "wasm32")]
use {
    wasm_bindgen::prelude::*, wasm_bindgen_futures::spawn_local as spawn,
};

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
extern "C" {
    fn download(filename: &str, text: &str);
}

#[derive(Debug, Clone, Serialize)]
struct ProcessedChannel {
    waveform: ProcessedWaveform,
    peaks: Option<Vec<[f64; 2]>>
}

#[derive(Debug, Clone, Serialize)]
struct ProcessedDeviceFrame {
    pub time: u64,
    pub channels: BTreeMap<u8, ProcessedChannel>
}

impl ProcessedDeviceFrame {
    pub fn merge(&self) -> Self {
        let mut merged = ProcessedWaveform(vec![0.0; self.channels.first_key_value().unwrap().1.waveform.0.len()]);
        
        self.channels.iter().for_each(|(_, channel)| {
            channel.waveform.0.iter().enumerate().for_each(|(j, value)| {
                merged.0[j] += value;
            })
        });

        let mut channels = BTreeMap::new();
        channels.insert(5u8, ProcessedChannel { waveform: merged, peaks: None});

        Self { time: self.time, channels }
    }
}

#[derive(Debug, Clone)]
enum AppState {
    Initializing,
    FirstLoad,
    Interactive
}

pub struct FilteredViewer {
    filepath: PathBuf,
    processing: ProcessParams,
    range: Range<f32>,
    neighborhood: usize,
    events: Arc<Mutex<Option<Vec<ProcessedDeviceFrame>>>>,
    indexes: Arc<Mutex<Option<Vec<usize>>>>,
    state: Arc<Mutex<AppState>>,
    current: usize,
    merge: bool
}

impl FilteredViewer {
    pub fn init_with_point(
        filepath: PathBuf,
        processing: ProcessParams,
        range: Range<f32>,
        neighborhood: usize
    ) -> Self {

        let viewer = Self {
            filepath: filepath.clone(),
            processing,
            range,
            neighborhood,
            events: Arc::new(Mutex::new(None)),
            indexes: Arc::new(Mutex::new(None)),
            state: Arc::new(Mutex::new(AppState::Initializing)),
            current: 0,
            merge: false
        };

        let events = Arc::clone(&viewer.events);
        let state = Arc::clone(&viewer.state);

        spawn(async move {

            let mut frames: BTreeMap<u64, BTreeMap<u8, ProcessedChannel>> = BTreeMap::new();

            let point = load_point(&filepath).await;

            for channel in &point.channels {
                for block in &channel.blocks {
                    for frame in &block.frames {
                        let entry = frames.entry(frame.time).or_default();
                        entry.insert(channel.id as u8, ProcessedChannel { 
                            waveform: process_waveform(frame), 
                            peaks: None
                        });
                    }
                }
            }

            {
                *events.lock() = Some(frames.iter().map(|(time, frame)| {
                    ProcessedDeviceFrame {
                        time: *time,
                        channels: frame.clone()
                    }
                }).collect::<Vec<_>>());

                *state.lock() = AppState::FirstLoad;
            }
        });

        viewer
    }

    fn update_indexes(&mut self) {

        let indexes = Arc::clone(&self.indexes);
        let events: Arc<Mutex<Option<Vec<ProcessedDeviceFrame>>>> = Arc::clone(&self.events);
        let processing = self.processing.clone();
        let range = self.range.clone();

        self.current = 0;

        spawn(async move {
            if let Some(events) = events.lock().as_mut() { 
                events.iter_mut().for_each(|ProcessedDeviceFrame { channels, .. }| {
                    channels.iter_mut().for_each(|(ch_id, processed)| {
                        processed.peaks =  Some(waveform_to_events(&processed.waveform, &processing.algorithm).iter().map(|(time, pos)|{
                            let pos = if processing.convert_to_kev {
                                convert_to_kev(pos, *ch_id, &processing.algorithm)
                            } else {
                                *pos
                            };
                            [*time as f64 / 8.0, pos as f64]
                        }).collect::<Vec<_>>());
                    })
                });
            }

            if let Some(events) = events.lock().as_ref() {
                *indexes.lock() = Some(events.iter().enumerate().filter_map(|(idx, frame)| {
                    let ProcessedDeviceFrame { channels, .. } = frame;
                    if channels.iter().any(|(_ , event)| {
                        if let Some(peaks) = event.peaks.as_ref() {
                            peaks.iter().any(|[_, amp]| {
                                range.contains(&(*amp as f32))
                            })
                        } else {
                            false
                        }
                    }) {
                        Some(idx)
                    } else {
                        None
                    }
                }).collect::<Vec<_>>());
            }
        });
    }

    fn plot_processed_frame(plot_ui: &mut PlotUi, event: BTreeMap<u8, ProcessedChannel>, secondary: bool, offset: i64) {
        for (ch_id, processed) in event {
            let ProcessedChannel { waveform, peaks } = processed;

            waveform.draw_egui(
                plot_ui, 
                Some(&format!("ch# {}", ch_id + 1)), 
                Some(color_for_index(ch_id as usize)), 
                Some(if secondary {1.0} else {3.0}), 
                Some(offset)
            );
        
            if !secondary {
                if let Some(peaks) = peaks {
                    plot_ui.points(Points::new(
                        peaks
                        ).shape(MarkerShape::Diamond)
                        .filled(false)
                        .radius(10.0)
                        .color(color_for_index(ch_id as usize))
                    )
                }
            }
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

    fn find_neighbors(position: usize, neighborhood: usize, events: &[ProcessedDeviceFrame]) -> Vec<ProcessedDeviceFrame> {

        let time = events[position].time;

        let mut neighbors = vec![];
        {
            if position != 0 {
                let mut left_position = position - 1;
                while left_position != 0 && time.abs_diff(events[left_position].time) < neighborhood as u64  {
                    neighbors.push(events[left_position].clone());
                    left_position -= 1;
                }
            }
        }

        {
            let mut right_position = position + 1;
            while right_position < events.len() && time.abs_diff(events[right_position].time) < neighborhood as u64 {
                neighbors.push(events[right_position].clone());
                right_position += 1;
            }
        }

        neighbors
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

            let processing = process_editor(ui, &self.processing);

            ui.separator();
            let mut min = self.range.start;
            ui.add(egui::Slider::new(&mut min, -10.0..=400.0).text("left"));
            let mut max = self.range.end;
            ui.add(egui::Slider::new(&mut max, -10.0..=400.0).text("right"));
            self.range = min..max;

            ui.label("neighborhood");
            ui.add(egui::Slider::new(&mut self.neighborhood, 0..=10000).text("ns"));

            ui.checkbox(&mut self.merge, "merge waveforms");

            if ui.button("apply").clicked() {
                self.update_indexes();
            }
        });

        eframe::egui::TopBottomPanel::top("position").show(ctx, |ui| {
            ui.horizontal(|ui| {

                #[cfg(not(target_arch = "wasm32"))]
                let width = frame.info().window_info.size.x;
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

                if let Some(events) = self.events.lock().as_ref() {
                    
                    ui.label(format!("{:.3} ms", events[self.current].time as f64 / 1e6));

                    if ui.button("save").clicked() {

                        let trigger_event = events[self.current].clone();
                        let neighbors = FilteredViewer::find_neighbors(
                            self.current, self.neighborhood, events);

                        let output = json!({
                            "filepath": self.filepath.clone(),
                            "processing": self.processing,
                            "range": self.range,
                            "neighborhood": self.neighborhood,
                            "trigger_event": trigger_event,
                            "neighbors": neighbors,
                        });

                        let filename = self.filepath.clone().file_name().unwrap().to_str().unwrap().to_owned();
                        let time = trigger_event.time;

                        spawn(async move {

                            let filepath = format!("filtered-{filename}-{time}.json");
    
                            #[cfg(not(target_arch = "wasm32"))] {
                                if let Some(save_folder) = rfd::FileDialog::new().set_directory(home_dir().unwrap()).pick_folder() {
                                    tokio::fs::write(
                                        save_folder.join(filepath), 
                                        serde_json::to_string_pretty(&output).unwrap()).await.unwrap()
                                }       
                            }
                            #[cfg(target_arch = "wasm32")] {
                                download(filepath.as_str(), &serde_json::to_string_pretty(&output).unwrap());
                            }
                        });
                    }

                }   
            })
        });

        eframe::egui::CentralPanel::default().show(ctx, |ui| {

            let indexes = self.indexes.lock().clone();
            let events = self.events.lock(); 

            if let Some(indexes) = indexes.as_ref() {
                if let Some(events) = events.as_ref() {
                    eframe::egui::plot::Plot::new("waveforms")
                    .legend(eframe::egui::plot::Legend {
                        text_style: eframe::egui::TextStyle::Body,
                        background_alpha: 1.0,
                        position: eframe::egui::plot::Corner::RightTop,
                    })
                    .x_axis_formatter(|value, _| format!("{:.3} μs", (value * 8.0) / 1000.0))
                    .show(ui, |plot_ui| {

                        if indexes.is_empty() {
                            return;
                        }

                        let position = indexes[self.current];

                        let ProcessedDeviceFrame { time, channels } = if self.merge {
                            events[position].merge()
                        } else { events[position].to_owned() };

                        FilteredViewer::plot_processed_frame(plot_ui, channels, false, 0);

                        for ProcessedDeviceFrame { time: time_2, channels } in FilteredViewer::find_neighbors(position, self.neighborhood, events) {
                            let offset = (time_2 as i64 - time as i64) / 8;
                            FilteredViewer::plot_processed_frame(plot_ui, channels, true, offset);
                        }
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
