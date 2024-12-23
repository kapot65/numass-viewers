use std::{collections::BTreeSet, path::PathBuf, sync::Arc};

use egui::mutex::Mutex;
use egui_plot::{GridMark, Legend};
use processing::{numass::protos::rsb_event, storage::load_point};

#[cfg(not(target_arch = "wasm32"))]
use tokio::spawn;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen_futures::spawn_local as spawn;

type Chunk = BTreeSet<i64>;

pub struct TriggerViewer {
    point: Arc<Mutex<Option<rsb_event::Point>>>,
    chunks: Arc<Mutex<Option<Vec<Chunk>>>>,

    limit_ns: u64,
}

impl TriggerViewer {
    pub fn init_with_point(filepath: PathBuf) -> Self {
        let viewer = TriggerViewer {
            point: Arc::new(Mutex::new(None)),
            chunks: Arc::new(Mutex::new(None)),
            limit_ns: 10_000_000,
        };

        let point = Arc::clone(&viewer.point);
        let chunks = Arc::clone(&viewer.chunks);
        let limit_ns = viewer.limit_ns;

        spawn(async move {
            let point_local = load_point(&filepath).await;

            let chunks_local = TriggerViewer::point_to_chunks(&point_local, limit_ns);

            *point.lock() = Some(point_local);
            *chunks.lock() = Some(chunks_local);
        });

        viewer
    }

    fn point_to_chunks(point: &rsb_event::Point, limit_ns: u64) -> Vec<Chunk> {
        let mut chunks = vec![];
        chunks.push(BTreeSet::new());

        for channel in &point.channels {
            for block in &channel.blocks {
                for frame in &block.frames {
                    let chunk_num = (frame.time / limit_ns) as usize;

                    while chunks.len() < chunk_num + 1 {
                        chunks.push(BTreeSet::new())
                    }

                    chunks[chunk_num].insert(
                        (frame.time % limit_ns) as i64, // channel.id as u8,
                                                        // (frame.time % limit_ns) as i64,
                                                        // waveform,
                    );
                }
            }
        }

        chunks
    }
}

impl eframe::App for TriggerViewer {
    #[allow(unused_variables)]
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if let Some(chunks) = self.chunks.lock().as_ref() {
            egui::CentralPanel::default().show(ctx, |ui| {
                egui_plot::Plot::new("waveforms")
                    .legend(Legend::default())
                    .x_axis_formatter(|GridMark { value, .. }, _, _| {
                        format!("{:.3} μs", value * 1e-3)
                    })
                    .show(ui, |plot_ui| {
                        let cr = chunks
                            .iter()
                            .enumerate()
                            .flat_map(|(idx, chunk)| {
                                vec![
                                    [(idx as u64 * self.limit_ns) as f64, chunk.len() as f64],
                                    [
                                        ((idx + 1) as u64 * self.limit_ns) as f64,
                                        chunk.len() as f64,
                                    ],
                                ]
                            })
                            .collect::<Vec<_>>();

                        plot_ui.line(egui_plot::Line::new(cr));
                    });
            });
        } else {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.spinner();
            });
        }
    }
}
