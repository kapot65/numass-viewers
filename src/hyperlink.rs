use egui::{Link, Response, Ui, Widget, WidgetText};

// modified version of egui::Hyperlink that will always open in a new window
// TODO: change to standart egui::Hyperlink when it will be possible to open links in new window
pub struct HyperlinkNewWindow {
    url: String,
    text: WidgetText,
}

impl HyperlinkNewWindow {
    pub fn new(text: impl Into<WidgetText>, url: impl ToString) -> Self {
        Self {
            url: url.to_string(),
            text: text.into(),
        }
    }
}

impl Widget for HyperlinkNewWindow {
    fn ui(self, ui: &mut Ui) -> Response {
        let Self { url, text } = self;
        let response = ui.add(Link::new(text));
        if response.clicked() {
            ui.ctx().output_mut(|o| {
                o.open_url = Some(egui::output::OpenUrl {
                    url: url.clone(),
                    new_tab: true,
                });
            });
        }
        response.on_hover_text(url)
    }
}
