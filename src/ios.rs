use super::{ACTION_FOLDER, ACTION_IMAGE};
use std::collections::VecDeque;
use std::ffi::{CStr, CString, c_char};
use std::sync::Mutex;

unsafe extern "C" {
    fn tein_ios_init();
    fn tein_ios_pick(image: bool) -> bool;
    fn tein_ios_media(
        title: *const c_char,
        artist: *const c_char,
        playing: bool,
        pos: i64,
        dur: i64,
    );
}

static ACTIONS: Mutex<VecDeque<i64>> = Mutex::new(VecDeque::new());
static PICKED: Mutex<Option<(bool, String)>> = Mutex::new(None);

#[unsafe(no_mangle)]
extern "C" fn tein_command(action: i64) {
    let mut actions = ACTIONS.lock().unwrap_or_else(|e| e.into_inner());
    if actions.len() < 64 {
        actions.push_back(action);
    }
}

#[unsafe(no_mangle)]
extern "C" fn tein_selected(image: bool, path: *const c_char) {
    // UIKit's file-system representation is borrowed for this callback only.
    let path = if path.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(path) }
            .to_str()
            .unwrap_or_default()
            .to_owned()
    };
    *PICKED.lock().unwrap_or_else(|e| e.into_inner()) = Some((image, path));
    let mut q = ACTIONS.lock().unwrap_or_else(|e| e.into_inner());
    q.retain(|a| ![i64::from(ACTION_FOLDER), i64::from(ACTION_IMAGE)].contains(a));
    q.push_front(i64::from(if image { ACTION_IMAGE } else { ACTION_FOLDER }));
}

pub fn init_ios() {
    unsafe { tein_ios_init() }
}
pub fn pick_folder() -> bool {
    unsafe { tein_ios_pick(false) }
}
pub fn pick_image() -> bool {
    unsafe { tein_ios_pick(true) }
}
pub fn poll_action() -> Option<i64> {
    ACTIONS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .pop_front()
}
pub fn take_picked(image: bool) -> String {
    let mut picked = PICKED.lock().unwrap_or_else(|e| e.into_inner());
    picked
        .take_if(|(kind, _)| *kind == image)
        .map(|(_, p)| p)
        .unwrap_or_default()
}
pub fn now_playing(title: &str, artist: &str, playing: bool, pos: i64, dur: i64) {
    let title = CString::new(title.replace('\0', "")).unwrap_or_default();
    let artist = CString::new(artist.replace('\0', "")).unwrap_or_default();
    unsafe { tein_ios_media(title.as_ptr(), artist.as_ptr(), playing, pos, dur) }
}
