#[cfg(not(target_arch = "wasm32"))]
fn main() {
    panic!("this binary is not meant to be run on desktop")
}
#[cfg(target_arch = "wasm32")]
fn main() {
    use gloo::worker::Registrable;
    use viewers::PointProcessor;
    console_error_panic_hook::set_once();
    PointProcessor::registrar().register();
}
