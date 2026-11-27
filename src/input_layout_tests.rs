use crate::{AppWindow, State};
use slint::platform::software_renderer::MinimalSoftwareWindow;
use slint::platform::{Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, PhysicalSize, SharedString};
use std::cell::RefCell;
use std::rc::Rc;

struct Headless(Rc<MinimalSoftwareWindow>);

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

fn click(ui: &AppWindow, x: f32, y: f32) {
    let position = LogicalPosition::new(x, y);
    let button = PointerEventButton::Left;
    ui.window()
        .dispatch_event(WindowEvent::PointerPressed { position, button });
    ui.window()
        .dispatch_event(WindowEvent::PointerReleased { position, button });
}

#[test]
fn long_input_preserves_text_and_keeps_actions_in_bounds() {
    let window = MinimalSoftwareWindow::new(Default::default());
    slint::platform::set_platform(Box::new(Headless(window.clone()))).unwrap();
    let ui = AppWindow::new().unwrap();
    ui.show().unwrap();
    let state = ui.global::<State>();
    let submitted = Rc::new(RefCell::new(None::<SharedString>));
    state.on_online_search({
        let submitted = submitted.clone();
        move |query| *submitted.borrow_mut() = Some(query)
    });
    let created = Rc::new(RefCell::new(None::<SharedString>));
    state.on_create_playlist({
        let created = created.clone();
        move |name| *created.borrow_mut() = Some(name)
    });
    let saved = Rc::new(std::cell::Cell::new(false));
    state.on_save_edit({
        let saved = saved.clone();
        move || saved.set(true)
    });
    let link = format!(
        "https://example.com/music?playlist={}",
        "abc123".repeat(100)
    );
    let name = "Artista y nombre de la canción ".repeat(25);
    for width in [320, 360, 411, 800] {
        window.set_size(PhysicalSize::new(width, 720));
        let w = width as f32;
        let pad = if width < 600 { 16. } else { 24. };
        ui.set_current_tab(1);
        for query in ["", "Tein", &link, &name] {
            state.set_online_query(query.into());
            click(&ui, w - pad - 23., 87.);
            assert_eq!(submitted.take().as_deref(), Some(query), "width: {width}");
        }
        ui.set_current_tab(0);
        state.set_lib_query(name.as_str().into());
        click(&ui, w - pad - 26., 87.);
        assert!(
            state.get_lib_query().is_empty(),
            "library clear, width: {width}"
        );

        state.set_name_dialog_open(true);
        click(&ui, w / 2., 252.);
        ui.window().dispatch_event(WindowEvent::KeyPressed {
            text: name.as_str().into(),
        });
        ui.window().dispatch_event(WindowEvent::KeyReleased {
            text: name.as_str().into(),
        });
        let dialog_right = (w + (w - 48.).min(400.)) / 2.;
        click(&ui, dialog_right - 48., 307.);
        assert_eq!(
            created.take().as_deref(),
            Some(name.as_str()),
            "create, width: {width}"
        );

        state.set_edit_title(name.as_str().into());
        state.set_edit_artist(name.as_str().into());
        state.set_edit_album(name.as_str().into());
        state.set_edit_open(true);
        let dialog_right = (w + (w - 24.).min(440.)) / 2.;
        click(&ui, dialog_right - 50., 310.);
        assert!(saved.replace(false), "save, width: {width}");
        state.set_edit_open(false);
    }
}
