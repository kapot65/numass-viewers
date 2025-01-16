use std::{sync::Arc, path::PathBuf, collections::BTreeMap};

use egui_plot::{Legend, Points};
use egui::mutex::Mutex;
use processing::{
    numass::{protos::rsb_event, NumassMeta}, postprocess::{post_process, PostProcessParams}, process::{extract_events, ProcessParams}, storage::{load_meta, load_point}, types::FrameEvent, utils::color_for_index, widgets::UserInput
};

#[cfg(not(target_arch = "wasm32"))]
use tokio::spawn;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen_futures::spawn_local as spawn;

type Chunk = Vec<(u8, i64, f32)>;

pub struct BundleViewer {
    point: Arc<Mutex<Option<rsb_event::Point>>>, // TODO: redownload point instead of storing?
    meta: Arc<Mutex<Option<NumassMeta>>>, // TODO: redownload meta instead of storing?

    process:ProcessParams,
    post_process: PostProcessParams,
    limit_ms: u64,

    chunks: Arc<Mutex<Option<Vec<Chunk>>>>,
    current_chunk: usize,
}

fn point_to_chunks(meta: Option<NumassMeta>, point: rsb_event::Point, process: ProcessParams, postprocess: PostProcessParams, limit_ns: u64) -> Vec<Chunk> {

    let events = post_process(extract_events(meta, point, &process), &postprocess);

    let mut chunks = vec![];
    chunks.push(vec![]);

    for (time, timed_event) in events {
        for (offset, event) in timed_event {

            if let FrameEvent::Event { channel, amplitude, .. } = event {
                let time = time + offset as u64;
                let chunk_num = (time / limit_ns) as usize;
                    
                while chunks.len() < chunk_num + 1 {
                    chunks.push(vec![])
                }

                chunks[chunk_num].push((
                    channel,
                    (time % limit_ns) as i64,
                    amplitude
                ));
            }
        }
    }

    chunks
}

impl BundleViewer {
    pub fn init_with_point(filepath: PathBuf, process: ProcessParams, post_process: PostProcessParams) -> Self {

        let viewer = BundleViewer {
            point: Arc::new(Mutex::new(None)),
            meta: Arc::new(Mutex::new(None)),
            process,
            post_process,
            limit_ms: 100,
            chunks: Arc::new(Mutex::new(None)),
            current_chunk: 0,
        };

        let point = Arc::clone(&viewer.point);
        let meta = Arc::clone(&viewer.meta);
        let chunks = Arc::clone(&viewer.chunks);
        let limit_ns = viewer.limit_ms * 1_000_000;
        let process = viewer.process.to_owned();
        let post_process = viewer.post_process.to_owned();
        
        spawn(async move {
            let point_local = load_point(&filepath).await;
            *point.lock() = Some(point_local);

            let meta_local = load_meta(&filepath).await;
            *meta.lock() = meta_local;

            BundleViewer::recalculate_chunks(meta, point, chunks, process, post_process, limit_ns);
        });

        viewer
    }

    fn recalculate_chunks(
        meta: Arc<Mutex<Option<NumassMeta>>>,
        point: Arc<Mutex<Option<rsb_event::Point>>>,
        chunks: Arc<Mutex<Option<Vec<Chunk>>>>, 
        process: ProcessParams, 
        post_process: PostProcessParams, 
        limit_ns: u64)  {

        *chunks.lock() = None;

        if let Some(point) = &*point.lock() { 
            let chunks_local = Some(point_to_chunks(
                meta.lock().clone(),
                point.clone(), 
                process, post_process, 
                limit_ns
            ));
            *chunks.lock() = chunks_local;
        }
    }
}

// TODO: add visualization for resets, overflows
impl eframe::App for BundleViewer {
    #[allow(unused_variables)]
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {

        if let Some(chunks) = &*self.chunks.lock() {
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
        }

        eframe::egui::SidePanel::left("parameters").show(ctx, |ui| {
            self.process = self.process.input(ui, ctx);

            ui.separator();

            self.post_process = self.post_process.input(ui, ctx);

            ui.separator();

            ui.add(egui::Slider::new(&mut self.limit_ms, 1..=1000).text("bin size (ms)"));

            if ui.button("apply").clicked() {

                self.current_chunk = 0; // Reset to the first chunk when applying changes.

                let meta = Arc::clone(&self.meta);
                let point = Arc::clone(&self.point);
                let chunks = Arc::clone(&self.chunks);
                let limit_ns = self.limit_ms * 1_000_000;
                let process = self.process.to_owned();
                let post_process = self.post_process.to_owned();

                spawn(async move {
                    BundleViewer::recalculate_chunks(meta, point, chunks, process, post_process, limit_ns);
                });
            }
        });
        
        egui::CentralPanel::default().show(ctx, |ui| {

            if let Some(chunks) = &*self.chunks.lock() {
                
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

                    egui_plot::Plot::new("waveforms").legend(Legend::default())
                        .x_axis_formatter(|mark, _, _| format!("{:.3} ms", mark.value))
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
                                    .name(format!("ch #{}", ch_num + 1))
                                )
                            }
                        });
                
            
            } else {
                ui.spinner();
            }
        });
    }
}