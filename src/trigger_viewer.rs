use std::{path::PathBuf, sync::Arc};

use egui::mutex::Mutex;
use egui_plot::{GridMark, Legend};
use processing::{histogram::PointHistogram, numass::protos::rsb_event, storage::load_point};

#[cfg(not(target_arch = "wasm32"))]
use tokio::spawn;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen_futures::spawn_local as spawn;

pub struct TriggerViewer {
    point: Arc<Mutex<Option<rsb_event::Point>>>,
    trigger_density: Arc<Mutex<Option<PointHistogram>>>,

    /// bin size in ms
    bin_size: u64, 
    per_channel: bool
}

impl TriggerViewer {
    pub fn init_with_point(filepath: PathBuf) -> Self {
        let viewer = TriggerViewer {
            point: Arc::new(Mutex::new(None)),
            trigger_density: Arc::new(Mutex::new(None)),
            bin_size: 10,
            per_channel: false
        };

        let point = Arc::clone(&viewer.point);
        let trigger_density = Arc::clone(&viewer.trigger_density);
        let limit_ms = viewer.bin_size;

        spawn(async move {
            let point_local = load_point(&filepath).await;
            *point.lock() = Some(point_local);

            TriggerViewer::calc_density(point, trigger_density, limit_ms).await   
        });

        viewer
    }

    async fn calc_density(point: Arc<Mutex<Option<rsb_event::Point>>>, trigger_density: Arc<Mutex<Option<PointHistogram>>>, limit_ms: u64) {
        
        if let Some(point) = point.lock().as_ref() {
        
            let end_time = {
                let mut end_time = 0;
                for channel in &point.channels {
                    for block in &channel.blocks {
                        for frame in &block.frames {
                            if end_time < frame.time {
                                end_time = frame.time;
                            }
                        }
                    }
                }
                end_time
            };

            let mut trigger_density_local = PointHistogram::new_step(0.0..(end_time as f32), (limit_ms as f32) * 1e6);

            for channel in &point.channels {
                for block in &channel.blocks {
                    for frame in &block.frames {
                        trigger_density_local.add(channel.id as u8, frame.time as f32);
                    }
                }
            }

            *trigger_density.lock() = Some(trigger_density_local);
        }

    }
}

impl eframe::App for TriggerViewer {
    #[allow(unused_variables)]
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {

        egui::SidePanel::left("left").show(ctx, |ui| {
            ui.add(egui::Slider::new(&mut self.bin_size, 1..=1_000).text("bin size (ms)"));
            ui.checkbox(&mut self.per_channel, "show each channel");
            if ui.button("apply").clicked() {

                *self.trigger_density.lock() = None;

                let point = Arc::clone(&self.point);
                let trigger_density = Arc::clone(&self.trigger_density);
                let limit_ms = self.bin_size;
                spawn(async move {
                    TriggerViewer::calc_density(point, trigger_density, limit_ms).await;
                });
            }
        });

        if let Some(trigger_density) = self.trigger_density.lock().as_ref() {
            egui::CentralPanel::default().show(ctx, |ui| {
                egui_plot::Plot::new("triggers")
                    .legend(Legend::default())
                    .x_axis_formatter(|GridMark { value, .. }, _, _| {
                        format!("{:.3} s", value * 1e-9)
                    })
                    .show(ui, |plot_ui| {
                        if self.per_channel {
                            trigger_density.draw_egui_each_channel(plot_ui, None);
                        } else {
                            trigger_density.draw_egui(plot_ui, None, None, None)
                        }
                    });
            });
        } else {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.spinner();
            });
        }
    }
}
