use std::{sync::Arc, path::PathBuf};

use egui::mutex::Mutex;
use processing::{point_to_chunks, color_for_index, ProcessedWaveform, EguiLine};

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

type Chunk = Vec<(u8, ProcessedWaveform)>;

pub struct PointViewer {
    chunks: Arc<Mutex<Option<Vec<Chunk>>>>,
    current_chunk: usize,
    state: Arc<Mutex<AppState>>,
}

impl PointViewer {
    pub fn init_with_point(filepath: PathBuf) -> Self {

        let viewer = PointViewer {
            chunks: Arc::new(Mutex::new(None)),
            current_chunk: 0,
            state: Arc::new(Mutex::new(AppState::Initializing)),
        };

        let chunks = Arc::clone(&viewer.chunks);
        let state = Arc::clone(&viewer.state);
        
        spawn(async move {
            let point = load_point(filepath).await;
            *chunks.lock() = Some(point_to_chunks(point, 1_000_000));
            *state.lock() = AppState::FirstLoad;
        });

        viewer
    }
}

impl eframe::App for PointViewer {
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
                                    .suffix(" ms")
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
                            .x_axis_formatter(|value, _| format!("{value:.3} μs"))
                            .show(ui, |plot_ui| {

                                for (ch_num, waveform) in chunks[self.current_chunk].clone() {

                                    waveform.draw_egui(
                                        plot_ui, 
                                        Some(&format!("ch #{}", ch_num + 1)), 
                                        Some(color_for_index((ch_num) as usize)),
                                         None, None
                                    );
                                }
                            });
                    });
                }
            }
        }
    }
}