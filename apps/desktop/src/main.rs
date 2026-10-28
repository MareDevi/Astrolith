mod spikes {
    pub mod editor;
    pub mod layout;
}

use gpui_kit::component::ActiveTheme;
use gpui_kit::{AppContext, Context, IntoElement, Render, Window, WindowOptions, div, prelude::*};

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
    let editor_spike = std::env::args().any(|argument| argument == "--spike=editor");
    let layout_spike = std::env::args().any(|argument| argument == "--spike=layout");
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            if layout_spike {
                if let Err(error) =
                    gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                        window.set_window_title("Astrolith · Layout spike");
                        cx.new(|cx| spikes::layout::LayoutSpike::new(window, cx))
                    })
                {
                    eprintln!("Could not open the layout spike: {error:#}");
                    std::process::exit(1);
                }
                return;
            }

            if editor_spike {
                spikes::editor::EditorSpike::bind_keys(cx);
                if let Err(error) =
                    gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                        window.set_window_title("Astrolith · Editor spike");
                        cx.new(|cx| spikes::editor::EditorSpike::new(window, cx))
                    })
                {
                    eprintln!("Could not open the editor spike: {error:#}");
                    std::process::exit(1);
                }
                return;
            }

            if let Err(error) = gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                window.set_window_title("Astrolith");
                cx.new(|_| Astrolith)
            }) {
                eprintln!("Could not open the Astrolith window: {error:#}");
                std::process::exit(1);
            }
        });
}
