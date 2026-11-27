use super::{ACTION_FOLDER, ACTION_IMAGE};
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Default)]
enum Dialog {
    #[default]
    Idle,
    Open,
    Ready {
        action: i32,
        path: Option<PathBuf>,
        notified: bool,
    },
}

static DIALOG: Mutex<Dialog> = Mutex::new(Dialog::Idle);

#[cfg(not(any(windows, target_os = "macos")))]
fn dialog(image: bool) -> std::io::Result<impl FnOnce() -> Option<PathBuf>> {
    use std::os::unix::ffi::OsStringExt;
    use std::process::{Command, Stdio};
    let mut cmd = Command::new("zenity");
    cmd.arg("--file-selection");
    if image {
        cmd.args([
            "--title=Elegir portada",
            "--file-filter=Imágenes | *.jpg *.jpeg *.png *.webp *.gif",
            "--file-filter=*",
        ]);
    } else {
        cmd.args(["--directory", "--title=Carpeta de descargas"]);
    }
    let child = cmd.stdout(Stdio::piped()).spawn()?;
    Ok(move || {
        let result = child.wait_with_output().ok()?;
        if !result.status.success() {
            return None;
        }
        let mut bytes = result.stdout;
        if bytes.last() == Some(&b'\n') {
            bytes.pop();
        }
        Some(std::ffi::OsString::from_vec(bytes).into())
    })
}

#[cfg(any(windows, target_os = "macos"))]
fn dialog(image: bool) -> std::io::Result<impl FnOnce() -> Option<PathBuf>> {
    Ok(move || {
        let picker = rfd::FileDialog::new();
        if image {
            picker
                .set_title("Elegir portada")
                .add_filter("Imágenes", &["jpg", "jpeg", "png", "webp", "gif"])
                .pick_file()
        } else {
            picker.set_title("Carpeta de descargas").pick_folder()
        }
    })
}

fn pick(image: bool) -> bool {
    let mut state = DIALOG.lock().unwrap_or_else(|e| e.into_inner());
    if !matches!(*state, Dialog::Idle) {
        return false;
    }
    let Ok(run) = dialog(image) else {
        return false;
    };
    *state = Dialog::Open;
    std::thread::spawn(move || {
        let path = run();
        *DIALOG.lock().unwrap_or_else(|e| e.into_inner()) = Dialog::Ready {
            action: if image { ACTION_IMAGE } else { ACTION_FOLDER },
            path,
            notified: false,
        };
    });
    true
}

pub fn pick_folder() -> bool {
    pick(false)
}
pub fn pick_image() -> bool {
    pick(true)
}

pub fn poll_action() -> Option<i64> {
    let mut state = DIALOG.lock().unwrap_or_else(|e| e.into_inner());
    if let Dialog::Ready {
        action, notified, ..
    } = &mut *state
    {
        if !*notified {
            *notified = true;
            return Some((*action).into());
        }
    }
    None
}

pub fn take_picked(image: bool) -> String {
    let mut state = DIALOG.lock().unwrap_or_else(|e| e.into_inner());
    let wanted = if image { ACTION_IMAGE } else { ACTION_FOLDER };
    if matches!(&*state, Dialog::Ready {action, ..} if *action == wanted) {
        if let Dialog::Ready { path: Some(p), .. } = std::mem::take(&mut *state) {
            return p.into_os_string().into_string().unwrap_or_default();
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn completed_dialog_notifies_once_and_cancel_releases_slot() {
        for path in [Some(PathBuf::from("portada con espacios.png")), None] {
            *DIALOG.lock().unwrap() = Dialog::Ready {
                action: ACTION_IMAGE,
                path: path.clone(),
                notified: false,
            };
            assert!(take_picked(false).is_empty());
            assert_eq!(poll_action(), Some(ACTION_IMAGE.into()));
            assert_eq!(poll_action(), None);
            assert_eq!(
                take_picked(true),
                path.unwrap_or_default().to_string_lossy()
            );
            assert!(matches!(*DIALOG.lock().unwrap(), Dialog::Idle));
        }
    }
}
