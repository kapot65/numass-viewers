#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(target_family = "unix")]
use tikv_jemallocator::Jemalloc;
#[cfg(target_family = "unix")]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

#[cfg(target_arch = "wasm32")]
fn main() {
    panic!("this binary is not meant to be run in browser")
}
#[cfg(not(target_arch = "wasm32"))]
#[tokio::main]
async fn main() {
    use clap::Parser;
    use egui_extras::install_image_loaders;
    use viewers::filtered_viewer::FilteredViewer;

    #[derive(Parser, Debug)]
    #[clap(author, version, about, long_about = None)]
    struct Opt {
        /// point file location
        filepath: std::path::PathBuf,
        /// minimal amplitude in range
        #[clap(long, default_value_t = 0.0)]
        min: f32,
        /// maximal amplitude in range
        #[clap(long, default_value_t = 27.0)]
        max: f32,
        /// process params serialized to json
        #[clap(long)]
        process: Option<String>,
        /// postprocess params serialized to json
        #[clap(long)]
        postprocess: Option<String>,
    }

    let args = Opt::parse();
    let filepath = args.filepath;
    let range = args.min..args.max;

    let process = if let Some(process) = args.process {
        serde_json::from_str(&process).expect("cant parse algorithm param")
    } else {
        processing::process::ProcessParams::default()
    };

    let postprocess = if let Some(postprocess) = args.postprocess {
        serde_json::from_str(&postprocess).expect("cant parse postprocess param")
    } else {
        processing::postprocess::PostProcessParams::default()
    };

    let viewer =
        FilteredViewer::init_with_point(filepath.clone(), process, postprocess, range).await;

    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        format!("filtered {filepath:?}").as_str(),
        native_options,
        Box::new(move |ctx| {
            install_image_loaders(&ctx.egui_ctx);
            ctx.egui_ctx.set_visuals(egui::Visuals::dark());
            Box::new(viewer)
        }),
    )
    .unwrap();
}
