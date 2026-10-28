use gpui_kit::component::ActiveTheme;
use gpui_kit::*;

struct Astrolith;

impl Render for Astrolith {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child("Astrolith")
    }
}

fn main() {
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(|cx| {
            gpui_kit::init(cx);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            if let Err(error) = gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                window.set_window_title("Astrolith");
                cx.new(|_| Astrolith)
            }) {
                eprintln!("Could not open the Astrolith window: {error:#}");
                std::process::exit(1);
            }
        });
}
