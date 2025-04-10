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
    use viewers::trigger_viewer::TriggerViewer;

    #[derive(Parser, Debug)]
    #[clap(author, version, about, long_about = None)]
    struct Opt {
        filepath: Option<std::path::PathBuf>,
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
            Ok(Box::new(TriggerViewer::init_with_point(filepath)))
        }),
    )
    .unwrap();
}
