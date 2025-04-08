use std::{path::PathBuf, sync::Arc};

use egui::{mutex::Mutex, Color32, Visuals};
use egui_plot::{GridMark, Legend, VLine};
use processing::{
    histogram::PointHistogram,
    numass::{protos::rsb_event, NumassMeta, Reply},
    preprocess::{Preprocess, CUTOFF_BIN_SIZE},
    process::TRAPEZOID_DEFAULT,
    storage::{load_meta, load_point},
    utils::correct_frame_time,
};

#[cfg(not(target_arch = "wasm32"))]
use tokio::spawn;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen_futures::spawn_local as spawn;

pub struct TriggerViewer {
    meta: Arc<Mutex<Option<NumassMeta>>>,
    point: Arc<Mutex<Option<rsb_event::Point>>>,
    trigger_density: Arc<Mutex<Option<PointHistogram>>>,
    preprocess: Arc<Mutex<Option<Preprocess>>>,

    /// bin size in ms
    bin_size: u64,
    per_channel: bool,
}

impl TriggerViewer {
    pub fn init_with_point(filepath: PathBuf) -> Self {
        let viewer = TriggerViewer {
            meta: Arc::new(Mutex::new(None)),
            point: Arc::new(Mutex::new(None)),
            preprocess: Arc::new(Mutex::new(None)),

            trigger_density: Arc::new(Mutex::new(None)),
            bin_size: 10,
            per_channel: false,
        };

        let meta = Arc::clone(&viewer.meta);
        let point = Arc::clone(&viewer.point);
        let trigger_density = Arc::clone(&viewer.trigger_density);
        let static_params = Arc::clone(&viewer.preprocess);
        let limit_ms = viewer.bin_size;

        spawn(async move {
            let meta_local = load_meta(&filepath).await;
            meta.lock().clone_from(&meta_local);

            let point_local = load_point(&filepath).await;

            // TODO: optimize to prevent double processing of point data
            let static_params_local =
                Preprocess::from_point(meta_local.clone(), &point_local, &TRAPEZOID_DEFAULT);
            *static_params.lock() = Some(static_params_local);

            *point.lock() = Some(point_local);

            if let Some(NumassMeta::Reply(Reply::AcquirePoint {
                acquisition_time, ..
            })) = meta_local
            {
                TriggerViewer::calc_density(
                    point,
                    trigger_density,
                    limit_ms,
                    (acquisition_time * 1e9) as u64,
                )
                .await;
            } else {
                panic!("Unexpected meta data type")
            }
        });

        viewer
    }

    /// Calculates the density of triggers over time.
    ///
    /// # Arguments
    /// * `bin_size` - A size of each time bin in nanoseconds.
    /// * `acquisition_time` - Total acquisition time in nanoseconds.
    ///
    async fn calc_density(
        point: Arc<Mutex<Option<rsb_event::Point>>>,
        trigger_density: Arc<Mutex<Option<PointHistogram>>>,
        bin_size: u64,
        acquisition_time: u64,
    ) {
        if let Some(point) = point.lock().as_ref() {
            let mut trigger_density_local =
                PointHistogram::new_step(0.0..(acquisition_time as f32), (bin_size as f32) * 1e6);

            for channel in &point.channels {
                for block in &channel.blocks {
                    for frame in &block.frames {
                        trigger_density_local
                            .add(channel.id as u8, correct_frame_time(frame.time) as f32);
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
        ctx.set_visuals(Visuals::dark());

        egui::SidePanel::left("left").show(ctx, |ui| {
            ui.add(egui::Slider::new(&mut self.bin_size, 1..=1_000).text("bin size (ms)"));
            ui.checkbox(&mut self.per_channel, "show each channel");
            if ui.button("apply").clicked() {
                *self.trigger_density.lock() = None;

                let point = Arc::clone(&self.point);
                let trigger_density = Arc::clone(&self.trigger_density);
                let limit_ms = self.bin_size;

                let meta = self.meta.lock().clone();
                spawn(async move {
                    if let Some(NumassMeta::Reply(Reply::AcquirePoint {
                        acquisition_time, ..
                    })) = meta
                    {
                        TriggerViewer::calc_density(
                            point,
                            trigger_density,
                            limit_ms,
                            (acquisition_time * 1e9) as u64,
                        )
                        .await;
                    } else {
                        panic!("Unexpected meta data type")
                    }
                });
            }
            ui.separator();

            if let Some(NumassMeta::Reply(Reply::AcquirePoint {
                acquisition_time, ..
            })) = self.meta.lock().as_ref()
            {
                ui.label(format!("acquisition_time: {acquisition_time}"));
            }
        });

        if let Some(trigger_density) = self.trigger_density.lock().as_ref() {
            egui::CentralPanel::default().show(ctx, |ui| {
                egui_plot::Plot::new("triggers")
                    .legend(Legend::default())
                    .x_axis_formatter(|GridMark { value, .. }, _| {
                        format!("{:.3} s", value * 1e-9)
                    })
                    .show(ui, |plot_ui| {
                        if self.per_channel {
                            trigger_density.draw_egui_each_channel(plot_ui, None);
                        } else {
                            trigger_density.draw_egui(plot_ui, None, None, None)
                        }

                        if let Some(Preprocess { bad_blocks, .. }) =
                            &self.preprocess.lock().as_ref()
                        {
                            bad_blocks.iter().for_each(|idx| {
                                plot_ui.vline(
                                    VLine::new("BAD", CUTOFF_BIN_SIZE as f64 * (*idx as f64))
                                        .color(Color32::WHITE)
                                );
                                plot_ui.vline(
                                    VLine::new("BAD",CUTOFF_BIN_SIZE as f64 * ((*idx + 1) as f64))
                                        .color(Color32::WHITE)
                                );
                            });
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
