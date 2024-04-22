use std::{sync::Arc, path::PathBuf};

use egui::mutex::Mutex;
use egui_plot::Legend;
use processing::{
    utils::{color_for_index, EguiLine}, 
    types::ProcessedWaveform, 
    numass::protos::rsb_event, 
    process::process_waveform,
    storage::load_point 
};

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

type Chunk = Vec<(u8, i64, ProcessedWaveform)>;

pub struct PointViewer {
    chunks: Arc<Mutex<Option<Vec<Chunk>>>>,
    current_chunk: usize,
    state: Arc<Mutex<AppState>>,
}

fn point_to_chunks(point: rsb_event::Point, limit_ns: u64) -> Vec<Chunk> {

    let mut chunks = vec![];
    chunks.push(vec![]);

    for channel in point.channels {
        for block in channel.blocks {
            for frame in block.frames {
                let chunk_num = (frame.time / limit_ns) as usize;
                
                while chunks.len() < chunk_num + 1 {
                    chunks.push(vec![])
                }

                let waveform = process_waveform(&frame);

                chunks[chunk_num].push((
                    channel.id as u8,
                    (frame.time % limit_ns) as i64,
                    waveform
                ));
            }
        }
    }

    chunks
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
            let point = load_point(&filepath).await;
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
            
                        egui_plot::Plot::new("waveforms").legend(Legend::default())
                            // TODO: fix
                            // .x_axis_formatter(|value, _| format!("{value:.3} μs"))
                            .show(ui, |plot_ui| {

                                for (ch_num, offset, waveform) in chunks[self.current_chunk].clone() {

                                    waveform.draw_egui(
                                        plot_ui, 
                                        Some(&format!("ch #{}", ch_num + 1)), 
                                        Some(color_for_index((ch_num) as usize)),
                                         None, Some(offset)
                                    );
                                }
                            });
                    });
                }
            }
        }
    }
}