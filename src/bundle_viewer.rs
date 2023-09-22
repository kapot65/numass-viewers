use std::{sync::Arc, path::PathBuf, collections::BTreeMap};

use egui::{mutex::Mutex, plot::Points};
use processing::{color_for_index, numass::protos::rsb_event, ProcessParams, PostProcessParams, post_process_events, extract_events};

use crate::load_point;

#[cfg(not(target_arch = "wasm32"))]
use tokio::spawn;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen_futures::spawn_local as spawn;

#[derive(Debug, Clone)]
enum AppState {
    Initializing,
    FirstLoad,
    Interactive
}

type Chunk = Vec<(u8, i64, f32)>;

pub struct BundleViewer {
    chunks: Arc<Mutex<Option<Vec<Chunk>>>>,
    current_chunk: usize,
    state: Arc<Mutex<AppState>>,
}

fn point_to_chunks(point: rsb_event::Point, limit_ns: u64) -> Vec<Chunk> {

    let frames = post_process_events(
        extract_events(&point, &ProcessParams::default()), 
        &PostProcessParams::default()
    );

    let mut chunks = vec![];
    chunks.push(vec![]);

    for (time, frame) in frames {
        for (ch_num, (offset, amp)) in frame {

            let time = time + offset as u64;
            let chunk_num = (time / limit_ns) as usize;
                
            while chunks.len() < chunk_num + 1 {
                chunks.push(vec![])
            }

            chunks[chunk_num].push((
                ch_num as u8,
                (time % limit_ns) as i64,
                amp
            ));
        }
    }

    chunks
}

impl BundleViewer {
    pub fn init_with_point(filepath: PathBuf) -> Self {

        let viewer = BundleViewer {
            chunks: Arc::new(Mutex::new(None)),
            current_chunk: 0,
            state: Arc::new(Mutex::new(AppState::Initializing)),
        };

        let chunks = Arc::clone(&viewer.chunks);
        let state = Arc::clone(&viewer.state);
        
        spawn(async move {
            let point = load_point(&filepath).await;
            *chunks.lock() = Some(point_to_chunks(point, 100_000_000));
            *state.lock() = AppState::FirstLoad;
        });

        viewer
    }
}

impl eframe::App for BundleViewer {
    #[allow(unused_variables)]
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {

        let state = self.state.lock().clone();

        match state {
            AppState::Initializing => {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.spinner();
                });
            }
            AppState::FirstLoad => {
                *self.state.lock() = AppState::Interactive;
            }
            AppState::Interactive => {
                if let Some(chunks) = self.chunks.lock().as_ref() {
                    ctx.input(|i| {
                        if i.key_pressed(eframe::egui::Key::ArrowRight)
                            && self.current_chunk < chunks.len() - 1
                        {
                            self.current_chunk += 1;
                        }
                        if i.key_pressed(eframe::egui::Key::ArrowLeft) && self.current_chunk > 0 {
                            self.current_chunk -= 1;
                        }
                    });

                    egui::CentralPanel::default().show(ctx, |ui| {
                        #[cfg(not(target_arch = "wasm32"))]
                        let width = frame.info().window_info.size.x;
                        #[cfg(target_arch = "wasm32")]
                        let width = eframe::web_sys::window()
                            .unwrap()
                            .inner_width()
                            .unwrap()
                            .as_f64()
                            .unwrap() as f32;
            
                        ui.style_mut().spacing.slider_width = width - 150.0;
            
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::Slider::new(&mut self.current_chunk, 0..=chunks.len() - 1)
                                    .suffix("00 ms") // TODO: change to custom formatter
                                    .step_by(1.0),
                            );
                            if ui.button("<").clicked() && self.current_chunk > 0 {
                                self.current_chunk -= 1;
                            }
                            if ui.button(">").clicked() && self.current_chunk < chunks.len() - 1 {
                                self.current_chunk += 1;
                            }
                        });

                        egui::plot::Plot::new("waveforms")
                            .legend(egui::plot::Legend {
                                text_style: egui::TextStyle::Body,
                                background_alpha: 1.0,
                                position: egui::plot::Corner::RightTop,
                            })
                            .x_axis_formatter(|value, _| format!("{:.3} ms", value))
                            .show(ui, |plot_ui| {

                                let mut channel_points = BTreeMap::new();

                                for (ch_num, offset, amp) in chunks[self.current_chunk].clone() {                     
                                    channel_points.entry(ch_num).or_insert(vec![]).push([offset as f64 / 1_000_000.0, amp as f64]);
                                }

                                for (ch_num, points) in channel_points {
                                    plot_ui.points(
                                        Points::new(points)
                                        .color(color_for_index((ch_num) as usize))
                                        .radius(3.0)
                                        .name(&format!("ch #{}", ch_num + 1))
                                    )
                                }
                            });
                    });
                }
            }
        }
    }
}