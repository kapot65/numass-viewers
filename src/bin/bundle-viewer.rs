#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#[cfg(target_arch = "wasm32")]
fn main() {
    panic!("this binary is not meant to be run in browser")
}

#[cfg(target_family = "unix")]
use tikv_jemallocator::Jemalloc;
#[cfg(target_family = "unix")]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

#[cfg(not(target_arch = "wasm32"))]
#[tokio::main]
async fn main() {
    use clap::Parser;
    use viewers::bundle_viewer::BundleViewer;

    #[derive(Parser, Debug)]
    #[clap(author, version, about, long_about = None)]
    struct Opt {
        /// point file location
        filepath: Option<std::path::PathBuf>,
        /// process params serialized to json
        #[clap(long)]
        process: Option<String>,
        /// postprocess params serialized to json
        #[clap(long)]
        postprocess: Option<String>,
    }

    let args = Opt::parse();

    let filepath = args
        .filepath
        .unwrap_or_else(|| rfd::FileDialog::new().pick_file().expect("no file choosen"));

    let native_options = eframe::NativeOptions::default();

    eframe::run_native(
        std::fs::canonicalize(&filepath).unwrap().to_str().unwrap(),
        native_options,
        Box::new(|ctx| {
            
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

            ctx.egui_ctx.set_visuals(egui::Visuals::dark());
            Box::new(BundleViewer::init_with_point(
                filepath,
                process,
                postprocess,
            ))
        }),
    )
    .unwrap();
}
