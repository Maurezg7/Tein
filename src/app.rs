use crate::art;
use crate::download::{self, Job, Update};
use crate::library::{self, Track};
use crate::online::{self, OnlineTrack};
use crate::player::{self, Cmd, Event, PlayerHandle, Snapshot};
use crate::store::{self, FAVORITES, Playlist};
use crate::{AppWindow, DownloadItem, GroupItem, OnlineItem, Palette, State, TrackItem};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;

static UI_ATTACHED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy)]
enum MenuAction {
    PlayNow,
    PlayNext,
    Enqueue,
    Favorite,
    AddToList,
    RemoveFromList(usize),
    RemoveFromQueue(usize),
    Delete,
    Edit,
    ConfirmDelete,
    Cancel,
    PlayGroup,
    ShuffleGroup,
    DeleteList,
    FillMeta,
    FillGroupMeta,
    FolderDefault,
    FolderSd,
    FolderPick,
    PickCover,
    RemoveCover,
    EditGroup,
    PickGroupCover,
    RemoveGroupCover,
}

/// What the image being picked will be used for.
enum CoverTarget {
    /// Embedded into these files (a song or a whole album).
    Tracks(Vec<Track>),
    /// Shown for an artist (view 1) or a playlist (view 3).
    Group(i32, String),
}

struct DownloadEntry {
    id: u64,
    title: String,
    status: String,
    progress: f32,
    state: i32,
    online: Option<(u64, usize)>,
    playlist: Option<String>,
}

#[derive(Default)]
struct Latest<T> {
    busy: bool,
    pending: Option<T>,
}

impl<T> Latest<T> {
    fn submit(&mut self, work: T) -> Option<T> {
        if self.busy {
            self.pending = Some(work);
            return None;
        }
        self.busy = true;
        Some(work)
    }
    fn finish(&mut self) -> Option<T> {
        let next = self.pending.take();
        self.busy = next.is_some();
        next
    }
}

type SearchJob = (u64, String, i32);

#[cfg(test)]
#[test]
fn active_work_keeps_only_the_latest_pending_item() {
    let mut slot = Latest::default();
    assert_eq!(slot.submit(1), Some(1));
    assert_eq!(slot.submit(2), None);
    assert_eq!(slot.submit(3), None);
    assert_eq!(slot.finish(), Some(3));
    assert_eq!(slot.finish(), None);
    assert_eq!(slot.submit(4), Some(4));
}
use crate::download::QUEUE_LIMIT as MAX_DOWNLOADS;

#[derive(Default)]
struct Core {
    library: Vec<Track>,
    playlists: Vec<Playlist>,
    filter: String,
    view: i32,
    group: Option<String>,
    visible: Vec<Track>,
    group_keys: Vec<String>,
    online: Vec<(OnlineTrack, i32)>,
    online_collection: Option<String>,
    search_gen: u64,
    searches: Latest<SearchJob>,
    last_search: Option<std::time::Instant>,
    web_images: Latest<u64>,
    metadata_busy: bool,
    thumb_generation: std::sync::Arc<std::sync::atomic::AtomicU64>,
    downloads: Vec<DownloadEntry>,
    next_id: u64,
    menu: Vec<MenuAction>,
    menu_track: Option<Track>,
    menu_group: Option<String>,
    snapshot: Option<Snapshot>,
    toast_gen: u64,
    scanning: bool,
    scan_again: bool,
    thumbs: std::collections::HashMap<String, (slint::Image, slint::Color)>,
    thumb_tried: std::collections::HashSet<String>,
    thumbs_busy: bool,
    thumbs_again: bool,
    web_thumbs: std::collections::HashMap<String, slint::Image>,
    prefs: store::Prefs,
    /// Downloads waiting for the user to choose a folder: (search generation, result indexes).
    pending_downloads: Option<(u64, Vec<usize>)>,
    cover_target: Option<CoverTarget>,
    edit_group: Option<(i32, String)>,
    custom_art: std::collections::HashMap<String, Option<slint::Image>>,
}

thread_local! {
    static CORE: RefCell<Core> = RefCell::new(Core::default());
    static UI: RefCell<Option<slint::Weak<AppWindow>>> = const { RefCell::new(None) };
    static PLAYER: RefCell<Option<PlayerHandle>> = const { RefCell::new(None) };
    static DOWNLOADER: RefCell<Option<SyncSender<Job>>> = const { RefCell::new(None) };
}

fn with(f: impl FnOnce(&AppWindow, &mut Core)) {
    let Some(ui) = UI.with(|u| u.borrow().as_ref().and_then(|w| w.upgrade())) else {
        return;
    };
    CORE.with(|c| f(&ui, &mut c.borrow_mut()));
}

/// Runs `f` on the UI thread from any worker thread.
fn post(f: impl FnOnce(&AppWindow, &mut Core) + Send + 'static) {
    let _ = slint::invoke_from_event_loop(move || with(f));
}

fn send(cmd: Cmd) {
    PLAYER.with(|p| {
        if let Some(p) = p.borrow().as_ref() {
            p.send(cmd)
        }
    });
}

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(v))
}

fn strings(v: &[&str]) -> ModelRc<SharedString> {
    model(v.iter().map(|s| SharedString::from(*s)).collect())
}

pub fn run() -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;
    ui.global::<State>()
        .set_desktop(!cfg!(target_os = "android"));
    UI_ATTACHED.store(true, Ordering::Release);
    UI.with(|u| *u.borrow_mut() = Some(ui.as_weak()));
    let settings = store::load_settings();
    let st = ui.global::<State>();
    st.set_volume(settings.volume);
    st.set_shuffle(settings.shuffle);
    st.set_repeat_mode(settings.repeat as i32);

    static ENGINE: std::sync::OnceLock<PlayerHandle> = std::sync::OnceLock::new();
    PLAYER.with(|p| {
        *p.borrow_mut() = Some(
            ENGINE
                .get_or_init(|| {
                    player::spawn(settings, |e| {
                        if UI_ATTACHED.load(Ordering::Acquire) {
                            post(move |ui, core| on_player(ui, core, e));
                        }
                    })
                })
                .clone(),
        )
    });
    DOWNLOADER.with(|d| {
        *d.borrow_mut() = Some(download::spawn(|u| {
            post(move |ui, core| on_download(ui, core, u))
        }))
    });

    CORE.with(|c| {
        let mut core = c.borrow_mut();
        core.library = library::load_cache();
        core.playlists = store::load_playlists();
        core.prefs = store::load_prefs();
        if let Some(dir) = &core.prefs.download_dir {
            crate::paths::set_download_dir(dir.into());
        }
    });
    with(|ui, core| {
        crate::eq::set_gains(core.prefs.eq);
        ui.global::<State>().set_eq_presets(model(
            crate::eq::PRESETS
                .iter()
                .map(|p| p.0.into())
                .collect::<Vec<slint::SharedString>>(),
        ));
        show_eq(ui, core);
        ui.global::<Palette>().set_theme(core.prefs.theme);
        ui.global::<State>()
            .set_download_dir(folder_label(&crate::paths::download_dir()).into());
    });
    with(|ui, core| refresh_library(ui, core));
    rescan();
    bind(&ui);
    send(Cmd::Snapshot);
    let result = ui.run();
    UI_ATTACHED.store(false, Ordering::Release);
    result
}

fn bind(ui: &AppWindow) {
    let st = ui.global::<State>();
    st.on_lib_filter(|text| {
        with(|ui, core| {
            core.filter = text.to_lowercase();
            refresh_library(ui, core);
        })
    });
    st.on_lib_view_changed(|v| {
        with(|ui, core| {
            core.view = v;
            core.group = None;
            refresh_library(ui, core);
        })
    });
    st.on_group_clicked(|i| {
        with(|ui, core| {
            core.group = core.group_keys.get(i as usize).cloned();
            refresh_library(ui, core);
        })
    });
    st.on_group_back(|| {
        with(|ui, core| {
            core.group = None;
            refresh_library(ui, core);
        })
    });
    st.on_rescan(rescan);
    st.on_track_clicked(|i| {
        with(|_, core| {
            if !core.visible.is_empty() {
                send(Cmd::Play {
                    queue: core.visible.clone(),
                    index: i as usize,
                });
            }
        })
    });
    st.on_play_all(|shuffle| {
        with(|ui, core| {
            if core.visible.is_empty() {
                return;
            }
            let is_shuffled = core
                .snapshot
                .as_ref()
                .map(|s| s.shuffle)
                .unwrap_or(ui.global::<State>().get_shuffle());
            if shuffle != is_shuffled {
                send(Cmd::Shuffle);
            }
            let index = if shuffle {
                fastrand::usize(..core.visible.len())
            } else {
                0
            };
            send(Cmd::Play {
                queue: core.visible.clone(),
                index,
            });
        })
    });
    st.on_track_menu(|i| {
        with(|ui, core| {
            if let Some(t) = core.visible.get(i as usize).cloned() {
                open_track_menu(ui, core, t, None);
            }
        })
    });
    st.on_current_menu(|| {
        with(|ui, core| {
            if let Some(t) = core.snapshot.as_ref().and_then(|s| s.current.clone()) {
                open_track_menu(ui, core, t, None);
            }
        })
    });
    st.on_group_menu(|i| {
        with(|ui, core| {
            if let Some(key) = core.group_keys.get(i as usize).cloned() {
                open_group_menu(ui, core, key);
            }
        })
    });
    st.on_current_group_menu(|| {
        with(|ui, core| {
            if let Some(key) = core.group.clone() {
                open_group_menu(ui, core, key);
            }
        })
    });
    st.on_menu_action(|i| with(|ui, core| menu_action(ui, core, i as usize)));
    st.on_save_edit(|| {
        with(|ui, core| {
            let st = ui.global::<State>();
            if st.get_edit_mode() != 0 {
                save_group_edit(ui, core);
                return;
            }
            let Some(track) = core.menu_track.clone() else {
                return;
            };
            let (title, artist, album) = (
                st.get_edit_title(),
                st.get_edit_artist(),
                st.get_edit_album(),
            );
            if title.trim().is_empty() {
                toast(ui, core, "Escribí un título".into());
                return;
            }
            // Check write access without modifying the file; leave the draft open on denial.
            if let Err(e) = std::fs::OpenOptions::new().write(true).open(&track.path) {
                write_error(ui, core, e);
                return;
            }
            if st.get_edit_saving() {
                return;
            }
            st.set_edit_saving(true);
            std::thread::spawn(move || {
                let path = std::path::Path::new(&track.path);
                let result = library::edit_tags(path, &title, &artist, &album).and_then(|_| {
                    library::read_track(path).ok_or("No se pudo releer la canción".into())
                });
                post(move |ui, core| {
                    let st = ui.global::<State>();
                    st.set_edit_saving(false);
                    match result {
                        Ok(new) => {
                            apply_track_update(ui, core, new);
                            st.set_edit_open(false);
                            toast(ui, core, "Cambios guardados".into());
                        }
                        Err(e) => toast(ui, core, format!("No se pudo guardar: {e}")),
                    }
                });
            });
        })
    });
    st.on_choose_playlist(|i| {
        with(|ui, core| {
            let (Some(track), Some(list)) =
                (core.menu_track.clone(), core.playlists.get_mut(i as usize))
            else {
                return;
            };
            if !list.paths.contains(&track.path) {
                list.paths.push(track.path.clone());
            }
            let name = list.name.clone();
            store::save_playlists(&core.playlists);
            refresh_library(ui, core);
            toast(ui, core, format!("Añadida a {name}"));
        })
    });
    st.on_create_playlist(|name| {
        with(|ui, core| {
            let name = name.trim().to_string();
            if name.is_empty() || core.playlists.iter().any(|p| p.name == name) {
                toast(ui, core, "Nombre vacío o repetido".into());
                return;
            }
            core.playlists.push(Playlist {
                name,
                paths: vec![],
            });
            store::save_playlists(&core.playlists);
            refresh_library(ui, core);
        })
    });

    st.on_toggle_play(|| send(Cmd::Toggle));
    st.on_next(|| send(Cmd::Next));
    st.on_prev(|| send(Cmd::Prev));
    st.on_seek(|v| send(Cmd::Seek(v)));
    st.on_set_volume(|v| send(Cmd::Volume(v)));
    st.on_toggle_shuffle(|| send(Cmd::Shuffle));
    st.on_cycle_repeat(|| send(Cmd::Repeat));
    st.on_queue_jump(|i| send(Cmd::Jump(i as usize)));
    st.on_queue_menu(|i| {
        with(|ui, core| {
            if let Some(t) = core
                .snapshot
                .as_ref()
                .and_then(|s| s.queue.get(i as usize))
                .cloned()
            {
                open_track_menu(ui, core, t, Some(i as usize));
            }
        })
    });
    st.on_toggle_fav(|| {
        with(|ui, core| {
            if let Some(t) = core.snapshot.as_ref().and_then(|s| s.current.clone()) {
                toggle_favorite(ui, core, &t);
            }
        })
    });

    st.on_online_search(|q| search(q.to_string()));
    st.on_open_source(|i| {
        with(|ui, core| {
            if let Some((
                OnlineTrack {
                    source: online::Source::SoundCloud { url, .. },
                    ..
                },
                _,
            )) = core.online.get(i as usize)
            {
                if !crate::platform::open_soundcloud(url) {
                    toast(ui, core, "No se pudo abrir SoundCloud".into());
                }
            }
        })
    });
    st.on_online_download(|i| with(|ui, core| start_downloads(ui, core, vec![i as usize])));
    st.on_online_download_all(|| {
        with(|ui, core| {
            let wanted = (0..core.online.len())
                .filter(|i| matches!(core.online[*i].1, 0 | 4))
                .collect();
            start_downloads(ui, core, wanted);
        })
    });
    st.on_change_download_dir(|| {
        with(|ui, core| {
            core.pending_downloads = None;
            choose_folder(ui, core);
        })
    });
    st.on_set_eq(|band, gain| {
        with(|ui, core| {
            if let Some(slot) = core.prefs.eq.get_mut(band as usize) {
                *slot = gain.round().clamp(-crate::eq::MAX_DB, crate::eq::MAX_DB);
                apply_eq(ui, core);
            }
        })
    });
    st.on_set_eq_preset(|i| {
        with(|ui, core| {
            if let Some(preset) = crate::eq::PRESETS.get(i as usize) {
                core.prefs.eq = preset.1;
                apply_eq(ui, core);
            }
        })
    });
    st.on_set_theme(|i| {
        with(|ui, core| {
            ui.global::<Palette>().set_theme(i);
            core.prefs.theme = i;
            store::save_prefs(&core.prefs);
            refresh_now_art(ui, core);
        })
    });
    st.on_clear_downloads(|| {
        with(|ui, core| {
            core.downloads.retain(|d| d.state < 2);
            refresh_downloads(ui, core);
        })
    });
}

fn rescan() {
    with(|ui, core| {
        if core.scanning {
            core.scan_again = true;
            return;
        }
        core.scanning = true;
        let previous = core.library.clone();
        ui.global::<State>()
            .set_library_status("Buscando música…".into());
        std::thread::spawn(move || {
            let tracks = library::scan(&previous);
            library::save_cache(&tracks);
            post(move |ui, core| {
                core.scanning = false;
                core.thumb_tried.clear();
                let live: std::collections::HashSet<_> = tracks.iter().map(art::key).collect();
                core.thumbs.retain(|k, _| live.contains(k));
                core.library = tracks;
                refresh_library(ui, core);
                if std::mem::take(&mut core.scan_again) {
                    let _ = slint::invoke_from_event_loop(rescan);
                }
            });
        });
    });
}

fn refresh_library(ui: &AppWindow, core: &mut Core) {
    let st = ui.global::<State>();
    let filter = core.filter.clone();
    let matches = |t: &Track| {
        filter.is_empty()
            || t.title.to_lowercase().contains(&filter)
            || t.artist.to_lowercase().contains(&filter)
            || t.album.to_lowercase().contains(&filter)
    };
    let current = core
        .snapshot
        .as_ref()
        .and_then(|s| s.current.as_ref())
        .map(|t| t.path.clone());

    let tracks: Vec<Track> = match (core.view, &core.group) {
        (0, _) => core
            .library
            .iter()
            .filter(|t| matches(t))
            .cloned()
            .collect(),
        (1, Some(g)) => core
            .library
            .iter()
            .filter(|t| &t.artist == g && matches(t))
            .cloned()
            .collect(),
        (2, Some(g)) => {
            let mut v: Vec<Track> = core
                .library
                .iter()
                .filter(|t| &t.album == g && matches(t))
                .cloned()
                .collect();
            v.sort_by_key(|t| t.path.clone());
            v
        }
        (3, Some(g)) => core
            .playlists
            .iter()
            .find(|p| &p.name == g)
            .map(|p| {
                p.paths
                    .iter()
                    .filter_map(|path| core.library.iter().find(|t| &t.path == path))
                    .filter(|t| matches(t))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default(),
        _ => vec![],
    };

    let mut groups: Vec<(String, String, Option<String>)> = vec![];
    if core.group.is_none() && core.view > 0 {
        if core.view == 3 {
            groups = core
                .playlists
                .iter()
                .filter(|p| filter.is_empty() || p.name.to_lowercase().contains(&filter))
                .map(|p| {
                    let art = p
                        .paths
                        .iter()
                        .filter_map(|path| core.library.iter().find(|t| &t.path == path))
                        .map(art::key)
                        .find(|k| core.thumbs.contains_key(k));
                    (p.name.clone(), format!("{} canciones", p.paths.len()), art)
                })
                .collect();
        } else {
            let mut map: std::collections::BTreeMap<String, (String, usize, Option<String>)> =
                Default::default();
            for t in core.library.iter().filter(|t| matches(t)) {
                let key = if core.view == 1 { &t.artist } else { &t.album };
                let entry = map
                    .entry(key.to_lowercase())
                    .or_insert_with(|| (key.clone(), 0, None));
                entry.1 += 1;
                if entry.2.is_none() {
                    let k = art::key(t);
                    if core.thumbs.contains_key(&k) {
                        entry.2 = Some(k);
                    }
                }
            }
            groups = map
                .into_values()
                .map(|(k, n, art)| {
                    (
                        k,
                        format!("{n} {}", if n == 1 { "canción" } else { "canciones" }),
                        art,
                    )
                })
                .collect();
        }
    }

    st.set_tracks(model(
        tracks
            .iter()
            .map(|t| TrackItem {
                title: t.title.clone().into(),
                subtitle: format!("{} · {}", t.artist, t.album).into(),
                trailing: online::format_duration(t.duration_secs).into(),
                playing: current.as_deref() == Some(t.path.as_str()),
                art: art_of(core, t).0,
                has_art: core.thumbs.contains_key(&art::key(t)),
            })
            .collect(),
    ));
    let view = core.view;
    let custom: Vec<Option<slint::Image>> = groups
        .iter()
        .map(|(n, _, _)| custom_art(core, view, n))
        .collect();
    st.set_groups(model(
        groups
            .iter()
            .zip(custom)
            .map(|((n, i, k), custom)| {
                let art = custom.or_else(|| {
                    k.as_ref()
                        .and_then(|k| core.thumbs.get(k))
                        .map(|a| a.0.clone())
                });
                GroupItem {
                    name: n.clone().into(),
                    info: i.clone().into(),
                    has_art: art.is_some(),
                    art: art.unwrap_or_default(),
                }
            })
            .collect(),
    ));
    st.set_group_title(core.group.clone().unwrap_or_default().into());
    let hero = core
        .group
        .clone()
        .and_then(|g| custom_art(core, view, &g))
        .map(|img| (img, true))
        .or_else(|| tracks.iter().map(|t| art_of(core, t)).find(|a| a.1));
    st.set_has_group_art(hero.is_some());
    st.set_group_art(hero.map(|a| a.0).unwrap_or_default());
    let total: u32 = tracks.iter().map(|t| t.duration_secs).sum();
    let status = if core.scanning {
        "Buscando música…".to_string()
    } else if tracks.is_empty() {
        String::new()
    } else {
        let n = tracks.len();
        format!(
            "{n} {} · {} min",
            if n == 1 { "canción" } else { "canciones" },
            total.div_ceil(60)
        )
    };
    st.set_library_status(status.into());
    core.visible = tracks;
    core.group_keys = groups.into_iter().map(|(n, _, _)| n).collect();
    refresh_fav(ui, core);
    request_thumbs(core);
}

fn open_group_menu(ui: &AppWindow, core: &mut Core, key: String) {
    let mut actions = vec![
        (MenuAction::PlayGroup, "Reproducir"),
        (MenuAction::ShuffleGroup, "Reproducir aleatorio"),
        (MenuAction::FillGroupMeta, "Completar datos y portadas"),
    ];
    match core.view {
        1 => {
            actions.push((MenuAction::EditGroup, "Renombrar artista…"));
            actions.push((MenuAction::PickGroupCover, "Cambiar foto…"));
            actions.push((MenuAction::RemoveGroupCover, "Quitar foto"));
        }
        2 => {
            actions.push((MenuAction::EditGroup, "Editar álbum…"));
            actions.push((MenuAction::PickGroupCover, "Cambiar portada…"));
            actions.push((MenuAction::RemoveGroupCover, "Quitar portada"));
        }
        _ => {
            if key != FAVORITES {
                actions.push((MenuAction::EditGroup, "Renombrar lista…"));
            }
            actions.push((MenuAction::PickGroupCover, "Cambiar portada…"));
            actions.push((MenuAction::RemoveGroupCover, "Quitar portada"));
            if key != FAVORITES {
                actions.push((MenuAction::DeleteList, "Eliminar lista"));
            }
        }
    }
    core.menu_group = Some(key.clone());
    core.menu_track = None;
    show_menu(ui, core, &key, actions);
}

fn art_of(core: &Core, t: &Track) -> (slint::Image, bool) {
    match core.thumbs.get(&art::key(t)) {
        Some((img, _)) => (img.clone(), true),
        None => (slint::Image::default(), false),
    }
}

fn refresh_now_art(ui: &AppWindow, core: &Core) {
    let tint = core
        .snapshot
        .as_ref()
        .and_then(|s| s.current.as_ref())
        .and_then(|t| core.thumbs.get(&art::key(t)))
        .map(|a| a.1)
        .unwrap_or_else(|| ui.global::<Palette>().get_accent().darker(1.1));
    ui.global::<State>().set_now_tint(tint);
}

/// Builds missing cover thumbnails in the background, a few at a time.
fn request_thumbs(core: &mut Core) {
    if core.thumbs_busy {
        core.thumbs_again = true;
        return;
    }
    let mut want = Vec::new();
    for t in &core.library {
        let k = art::key(t);
        if !core.thumbs.contains_key(&k) && !core.thumb_tried.contains(&k) {
            core.thumb_tried.insert(k.clone());
            want.push((k, t.path.clone()));
        }
    }
    if want.is_empty() {
        return;
    }
    core.thumbs_busy = true;
    std::thread::spawn(move || {
        let total = want.len();
        let mut batch = vec![];
        let mut last = std::time::Instant::now();
        for (i, (k, path)) in want.into_iter().enumerate() {
            if let Some(t) = art::load(&k, &path) {
                let tint = art::tint(&t);
                batch.push((k, t, tint));
            }
            if !batch.is_empty()
                && (batch.len() >= 16 || i + 1 == total || last.elapsed().as_millis() >= 200)
            {
                last = std::time::Instant::now();
                let ready = std::mem::take(&mut batch);
                post(move |ui, core| {
                    for (k, t, tint) in ready {
                        core.thumbs.insert(k, (slint::Image::from_rgba8(t), tint));
                    }
                    refresh_now_art(ui, core);
                    refresh_queue(ui, core);
                    refresh_library(ui, core);
                });
            }
        }
        post(|_, core| {
            core.thumbs_busy = false;
            if std::mem::take(&mut core.thumbs_again) {
                request_thumbs(core);
            }
        });
    });
}

fn forget_art(core: &mut Core, key: &str) {
    art::forget(key);
    core.thumbs.remove(key);
    core.thumb_tried.remove(key);
}

/// Fetches small previews for the online results.
fn request_web_thumbs(core: &mut Core, generation: u64) {
    if core.web_images.submit(generation).is_some() {
        load_web_thumbs(core, generation);
    }
}

fn load_web_thumbs(core: &mut Core, generation: u64) {
    let current = core.thumb_generation.clone();
    let ids: Vec<(String, String)> = core
        .online
        .iter()
        .filter_map(|(t, _)| t.thumb())
        .filter(|(k, _)| !core.web_thumbs.contains_key(k))
        .take(60)
        .collect();
    if ids.is_empty() {
        core.web_images.finish();
        return;
    }
    std::thread::spawn(move || {
        for chunk in ids.chunks(6) {
            if current.load(Ordering::Relaxed) != generation {
                break;
            }
            let got: Vec<_> = chunk
                .iter()
                .take_while(|_| current.load(std::sync::atomic::Ordering::Relaxed) == generation)
                .filter_map(|(id, url)| {
                    let data = crate::net::get_bytes(url, "Mozilla/5.0").ok()?;
                    Some((id.clone(), art::from_bytes(&data)?))
                })
                .collect();
            post(move |ui, core| {
                if core.search_gen != generation {
                    return;
                }
                for (id, t) in got {
                    core.web_thumbs.insert(id, slint::Image::from_rgba8(t));
                }
                refresh_online(ui, core);
            });
        }
        post(|_, core| {
            if let Some(next) = core.web_images.finish() {
                load_web_thumbs(core, next);
            }
        });
    });
}

fn refresh_queue(ui: &AppWindow, core: &Core) {
    let Some(snap) = &core.snapshot else {
        return;
    };
    let items = snap
        .queue
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let (art, has_art) = art_of(core, t);
            TrackItem {
                title: t.title.clone().into(),
                subtitle: t.artist.clone().into(),
                trailing: online::format_duration(t.duration_secs).into(),
                playing: i == snap.index,
                art,
                has_art,
            }
        })
        .collect();
    ui.global::<State>().set_queue(model(items));
}

fn refresh_fav(ui: &AppWindow, core: &Core) {
    let fav = core
        .snapshot
        .as_ref()
        .and_then(|s| s.current.as_ref())
        .is_some_and(|t| {
            core.playlists
                .iter()
                .any(|p| p.name == FAVORITES && p.paths.contains(&t.path))
        });
    ui.global::<State>().set_current_fav(fav);
}

fn toggle_favorite(ui: &AppWindow, core: &mut Core, track: &Track) {
    let Some(fav) = core.playlists.iter_mut().find(|p| p.name == FAVORITES) else {
        return;
    };
    let msg = if let Some(pos) = fav.paths.iter().position(|p| p == &track.path) {
        fav.paths.remove(pos);
        "Quitada de Favoritas"
    } else {
        fav.paths.push(track.path.clone());
        "Añadida a Favoritas"
    };
    store::save_playlists(&core.playlists);
    refresh_library(ui, core);
    toast(ui, core, msg.into());
}

fn open_track_menu(ui: &AppWindow, core: &mut Core, track: Track, queue_index: Option<usize>) {
    let fav = core
        .playlists
        .iter()
        .any(|p| p.name == FAVORITES && p.paths.contains(&track.path));
    let mut actions = vec![
        (MenuAction::PlayNow, "Reproducir ahora"),
        (MenuAction::PlayNext, "Reproducir a continuación"),
        (MenuAction::Enqueue, "Añadir a la cola"),
        (
            MenuAction::Favorite,
            if fav {
                "Quitar de Favoritas"
            } else {
                "Añadir a Favoritas"
            },
        ),
        (MenuAction::AddToList, "Añadir a una lista…"),
    ];
    if let Some(i) = queue_index {
        if core.snapshot.as_ref().is_some_and(|s| s.index != i) {
            actions.push((MenuAction::RemoveFromQueue(i), "Quitar de la cola"));
        }
    } else if let (3, Some(g)) = (core.view, &core.group) {
        if let Some(idx) = core.playlists.iter().position(|p| &p.name == g) {
            actions.push((MenuAction::RemoveFromList(idx), "Quitar de esta lista"));
        }
    }
    actions.push((MenuAction::FillMeta, "Completar datos y portada"));
    actions.push((MenuAction::Edit, "Editar datos…"));
    actions.push((MenuAction::PickCover, "Cambiar portada…"));
    actions.push((MenuAction::RemoveCover, "Quitar portada"));
    actions.push((MenuAction::Delete, "Eliminar archivo…"));
    let title = format!("{} — {}", track.title, track.artist);
    core.menu_track = Some(track);
    core.menu_group = None;
    show_menu(ui, core, &title, actions);
}

fn show_menu(ui: &AppWindow, core: &mut Core, title: &str, actions: Vec<(MenuAction, &str)>) {
    let st = ui.global::<State>();
    st.set_menu_title(title.into());
    st.set_menu_options(strings(
        &actions.iter().map(|(_, l)| *l).collect::<Vec<_>>(),
    ));
    core.menu = actions.into_iter().map(|(a, _)| a).collect();
    st.set_menu_open(true);
}

fn menu_action(ui: &AppWindow, core: &mut Core, i: usize) {
    let Some(action) = core.menu.get(i).copied() else {
        return;
    };
    let track = core.menu_track.clone();
    match action {
        MenuAction::PlayNow => {
            if let Some(t) = track {
                send(Cmd::PlayNext(t));
                send(Cmd::Next);
            }
        }
        MenuAction::PlayNext => {
            if let Some(t) = track {
                send(Cmd::PlayNext(t));
                toast(ui, core, "Sonará a continuación".into());
            }
        }
        MenuAction::Enqueue => {
            if let Some(t) = track {
                send(Cmd::Enqueue(t));
                toast(ui, core, "Añadida a la cola".into());
            }
        }
        MenuAction::Favorite => {
            if let Some(t) = track {
                toggle_favorite(ui, core, &t);
            }
        }
        MenuAction::AddToList => {
            let names: Vec<SharedString> = core
                .playlists
                .iter()
                .map(|p| p.name.clone().into())
                .collect();
            let st = ui.global::<State>();
            st.set_playlist_names(model(names));
            st.set_chooser_open(true);
        }
        MenuAction::RemoveFromList(idx) => {
            if let (Some(t), Some(list)) = (track, core.playlists.get_mut(idx)) {
                list.paths.retain(|p| p != &t.path);
                store::save_playlists(&core.playlists);
                refresh_library(ui, core);
            }
        }
        MenuAction::Delete => {
            if let Some(t) = track {
                show_menu(
                    ui,
                    core,
                    &format!("¿Eliminar «{}» del dispositivo?", t.title),
                    vec![
                        (MenuAction::Cancel, "Cancelar"),
                        (MenuAction::ConfirmDelete, "Eliminar definitivamente"),
                    ],
                );
            }
        }
        MenuAction::Cancel => {}
        MenuAction::RemoveFromQueue(i) => {
            if track
                .as_ref()
                .is_some_and(|t| core.snapshot.as_ref().and_then(|s| s.queue.get(i)) == Some(t))
            {
                send(Cmd::Remove(i));
            }
        }
        MenuAction::Edit => {
            if let Some(t) = track {
                let st = ui.global::<State>();
                st.set_edit_title(t.title.into());
                st.set_edit_artist(if t.artist == "Artista desconocido" {
                    "".into()
                } else {
                    t.artist.into()
                });
                st.set_edit_album(if t.album == "Sin álbum" {
                    "".into()
                } else {
                    t.album.into()
                });
                st.set_edit_mode(0);
                st.set_edit_open(true);
            }
        }
        MenuAction::FolderDefault => {
            apply_download_dir(ui, core, crate::paths::get().download_dir.clone())
        }
        MenuAction::FolderSd => {
            if let Some((root, app)) = crate::platform::sd_card() {
                // Android 10 and older only let apps write to their own folder on the card.
                let shared = std::path::Path::new(&root).join("Music/Tein");
                let dir = if std::fs::create_dir_all(&shared).is_ok()
                    && crate::paths::is_writable(&shared)
                {
                    shared
                } else {
                    std::path::Path::new(&app).join("Music")
                };
                apply_download_dir(ui, core, dir);
            } else {
                toast(ui, core, "No se encontró una tarjeta SD".into());
            }
        }
        MenuAction::FolderPick => {
            if !crate::platform::pick_folder() {
                toast(ui, core, "No se pudo abrir el selector de carpetas".into());
            }
        }
        MenuAction::PickCover => {
            if let Some(t) = track {
                pick_cover(ui, core, CoverTarget::Tracks(vec![t]));
            }
        }
        MenuAction::RemoveCover => {
            if let Some(t) = track {
                write_covers(ui, core, vec![t], None);
            }
        }
        MenuAction::PickGroupCover | MenuAction::RemoveGroupCover => {
            let Some(key) = core.menu_group.clone() else {
                return;
            };
            let pick = matches!(action, MenuAction::PickGroupCover);
            if core.view == 2 {
                let tracks = group_tracks(core, 2, &key);
                if pick {
                    pick_cover(ui, core, CoverTarget::Tracks(tracks));
                } else {
                    write_covers(ui, core, tracks, None);
                }
            } else if pick {
                pick_cover(ui, core, CoverTarget::Group(core.view, key));
            } else {
                let _ = std::fs::remove_file(group_art_file(core.view, &key));
                core.custom_art.clear();
                refresh_library(ui, core);
            }
        }
        MenuAction::EditGroup => {
            let Some(key) = core.menu_group.clone() else {
                return;
            };
            let st = ui.global::<State>();
            st.set_edit_title(key.clone().into());
            let artist = if core.view == 2 {
                let tracks = group_tracks(core, 2, &key);
                match tracks.first() {
                    Some(first) if tracks.iter().all(|t| t.artist == first.artist) => {
                        first.artist.clone()
                    }
                    _ => String::new(),
                }
            } else {
                String::new()
            };
            st.set_edit_artist(artist.into());
            st.set_edit_mode(match core.view {
                2 => 1,
                1 => 2,
                _ => 3,
            });
            core.edit_group = Some((core.view, key));
            st.set_edit_open(true);
        }
        MenuAction::ConfirmDelete => {
            if let Some(t) = track {
                match std::fs::remove_file(&t.path) {
                    Ok(()) => {
                        forget_art(core, &art::key(&t));
                        player::forget_cover(&t.path);
                        send(Cmd::Forget(t.path.clone()));
                        core.library.retain(|x| x.path != t.path);
                        for p in &mut core.playlists {
                            p.paths.retain(|x| x != &t.path);
                        }
                        library::save_cache(&core.library);
                        store::save_playlists(&core.playlists);
                        refresh_library(ui, core);
                        toast(ui, core, "Archivo eliminado".into());
                    }
                    Err(e) => write_error(ui, core, e),
                }
            }
        }
        MenuAction::PlayGroup | MenuAction::ShuffleGroup => {
            let saved = core
                .group
                .replace(core.menu_group.clone().unwrap_or_default());
            refresh_library(ui, core);
            let queue = core.visible.clone();
            core.group = saved;
            refresh_library(ui, core);
            if !queue.is_empty() {
                let shuffle = matches!(action, MenuAction::ShuffleGroup);
                if shuffle != core.snapshot.as_ref().is_some_and(|s| s.shuffle) {
                    send(Cmd::Shuffle);
                }
                let index = if shuffle {
                    fastrand::usize(..queue.len())
                } else {
                    0
                };
                send(Cmd::Play { queue, index });
            }
        }
        MenuAction::FillMeta => {
            if let Some(t) = track {
                fill_meta(ui, core, vec![t]);
            }
        }
        MenuAction::FillGroupMeta => {
            let saved = core
                .group
                .replace(core.menu_group.clone().unwrap_or_default());
            refresh_library(ui, core);
            let tracks = core.visible.clone();
            core.group = saved;
            refresh_library(ui, core);
            fill_meta(ui, core, tracks);
        }
        MenuAction::DeleteList => {
            if let Some(name) = &core.menu_group {
                core.playlists
                    .retain(|p| &p.name != name || p.name == FAVORITES);
                store::save_playlists(&core.playlists);
                refresh_library(ui, core);
            }
        }
    }
}

fn write_error(ui: &AppWindow, core: &mut Core, e: std::io::Error) {
    if e.kind() == std::io::ErrorKind::PermissionDenied && !crate::platform::can_write_all() {
        toast(
            ui,
            core,
            "Activá el permiso de archivos y volvé a intentarlo".into(),
        );
        crate::platform::ask_write_all();
    } else {
        toast(ui, core, format!("No se pudo modificar el archivo: {e}"));
    }
}

fn apply_track_update(ui: &AppWindow, core: &mut Core, new: Track) {
    if let Some(i) = core.library.iter().position(|t| t.path == new.path) {
        let old = art::key(&core.library[i]);
        forget_art(core, &old);
        forget_art(core, &art::key(&new));
        player::forget_cover(&new.path);
        send(Cmd::Refresh(new.clone()));
        core.library[i] = new;
        library::sort(&mut core.library);
        library::save_cache(&core.library);
        refresh_library(ui, core);
    }
}

/// Looks up tags and covers online in the background and rewrites the files.
fn fill_meta(ui: &AppWindow, core: &mut Core, tracks: Vec<Track>) {
    if tracks.is_empty() {
        return;
    }
    if core.metadata_busy {
        toast(ui, core, "Ya se están completando datos".into());
        return;
    }
    core.metadata_busy = true;
    toast(ui, core, "Buscando datos…".into());
    std::thread::spawn(move || {
        let total = tracks.len();
        let (mut ok, mut last_err) = (0, String::new());
        for (i, t) in tracks.into_iter().enumerate() {
            match crate::meta::enrich(&t) {
                Ok(_) => {
                    ok += 1;
                    let path = t.path.clone();
                    post(move |ui, core| {
                        if let Some(new) = library::read_track(std::path::Path::new(&path)) {
                            apply_track_update(ui, core, new);
                        }
                    });
                }
                Err(e) => last_err = e,
            }
            if total > 1 {
                post(move |ui, core| {
                    toast(ui, core, format!("Completando datos {}/{total}…", i + 1))
                });
            }
        }
        let blocked = ok < total
            && !crate::platform::can_write_all()
            && last_err.to_ascii_lowercase().contains("permission denied");
        post(move |ui, core| {
            core.metadata_busy = false;
            if blocked {
                toast(
                    ui,
                    core,
                    "Activá \"Acceso a todos los archivos\" para Tein y probá de nuevo".into(),
                );
                crate::platform::ask_write_all();
                return;
            }
            let msg = match (ok, total) {
                (0, _) => last_err,
                (1, 1) => "Datos y portada actualizados".into(),
                _ => format!("Actualizadas {ok} de {total} canciones"),
            };
            toast(ui, core, msg);
        });
    });
}

fn toast(ui: &AppWindow, core: &mut Core, msg: String) {
    core.toast_gen += 1;
    let generation = core.toast_gen;
    ui.global::<State>().set_toast(msg.into());
    slint::Timer::single_shot(std::time::Duration::from_millis(2500), move || {
        with(|ui, core| {
            if core.toast_gen == generation {
                ui.global::<State>().set_toast("".into());
            }
        })
    });
}

fn on_player(ui: &AppWindow, core: &mut Core, event: Event) {
    let st = ui.global::<State>();
    match event {
        Event::Tick { pos, dur } => {
            st.set_position(pos);
            st.set_duration(dur);
            st.set_position_text(online::format_duration(pos as u32).into());
            st.set_duration_text(online::format_duration(dur as u32).into());
        }
        Event::Error(e) => toast(ui, core, e),
        // Core is borrowed here; rescan borrows it again, so defer it.
        Event::Rescan => {
            let _ = slint::invoke_from_event_loop(rescan);
        }
        Event::FolderPicked => {
            let path = crate::platform::take_picked(false);
            if path.is_empty() {
                core.pending_downloads = None;
                toast(ui, core, "No se seleccionó una carpeta".into());
            } else {
                apply_download_dir(ui, core, path.into());
            }
        }
        Event::ImagePicked => {
            let path = crate::platform::take_picked(true);
            match core.cover_target.take() {
                Some(target) if !path.is_empty() => use_picked_image(ui, core, path, target),
                _ => toast(ui, core, "No se seleccionó una imagen".into()),
            }
        }
        Event::Changed(snap) => {
            let changed_track =
                core.snapshot.as_ref().and_then(|s| s.current.as_ref()) != snap.current.as_ref();
            st.set_has_current(snap.current.is_some());
            if let Some(t) = &snap.current {
                st.set_now_title(t.title.clone().into());
                st.set_now_artist(t.artist.clone().into());
                st.set_now_album(t.album.clone().into());
            }
            if changed_track
                || core.snapshot.as_ref().and_then(|s| s.cover.as_ref()) != snap.cover.as_ref()
            {
                let image = snap
                    .cover
                    .as_ref()
                    .and_then(|p| slint::Image::load_from_path(std::path::Path::new(p)).ok());
                st.set_has_cover(image.is_some());
                st.set_now_cover(image.unwrap_or_default());
            }
            st.set_playing(snap.playing);
            st.set_shuffle(snap.shuffle);
            st.set_repeat_mode(snap.repeat as i32);
            st.set_volume(snap.volume);
            core.snapshot = Some(snap);
            refresh_queue(ui, core);
            if changed_track {
                refresh_now_art(ui, core);
                refresh_library(ui, core);
            }
        }
    }
}

fn search(query: String) {
    let query = query.trim();
    if query.is_empty() {
        return;
    }
    with(|ui, core| {
        if query.len() > 4096 {
            toast(
                ui,
                core,
                "El enlace o la búsqueda es demasiado largo".into(),
            );
            return;
        }
        core.search_gen += 1;
        let generation = core.search_gen;
        core.thumb_generation
            .store(generation, std::sync::atomic::Ordering::Relaxed);
        core.web_thumbs.clear();
        core.online.clear();
        core.online_collection = None;
        refresh_online(ui, core);
        let st = ui.global::<State>();
        let provider = st.get_online_source();
        st.set_online_busy(true);
        st.set_online_status(
            if query.contains("://") || query.starts_with("spotify:") {
                "Leyendo link…"
            } else {
                "Buscando…"
            }
            .into(),
        );
        if let Some(job) = core.searches.submit((generation, query.into(), provider)) {
            run_search(core, job);
        }
    });
}

fn run_search(core: &mut Core, (generation, query, provider): SearchJob) {
    let now = std::time::Instant::now();
    let start = core
        .last_search
        .map_or(now, |t| now.max(t + std::time::Duration::from_secs(1)));
    core.last_search = Some(start);
    std::thread::spawn(move || {
        std::thread::sleep(start.saturating_duration_since(std::time::Instant::now()));
        let result = online::resolve_in(&query, provider);
        post(move |ui, core| {
            if let Some(job) = core.searches.finish() {
                run_search(core, job);
                return;
            }
            if core.search_gen != generation {
                return;
            }
            let st = ui.global::<State>();
            st.set_online_busy(false);
            match result {
                Ok(res) => {
                    let n = res.tracks.len();
                    st.set_online_status(if n == 0 {
                        if provider == 2 {
                            "Sin descargas públicas compatibles en Audius"
                        } else {
                            "Sin resultados"
                        }
                        .into()
                    } else if res.collection.is_some() {
                        format!(
                            "{} · {n} {}",
                            res.label,
                            if n == 1 { "canción" } else { "canciones" }
                        )
                        .into()
                    } else {
                        res.label.into()
                    });
                    core.online_collection = res.collection;
                    core.online = res.tracks.into_iter().map(|t| (t, 0)).collect();
                    request_web_thumbs(core, generation);
                }
                Err(e) => {
                    st.set_online_status(format!("Error: {e}").into());
                    core.online.clear();
                    core.online_collection = None;
                }
            }
            refresh_online(ui, core);
        });
    });
}

fn refresh_online(ui: &AppWindow, core: &Core) {
    let items = core
        .online
        .iter()
        .map(|(t, state)| {
            let thumb = t.thumb_key().and_then(|key| core.web_thumbs.get(key));
            OnlineItem {
                art: thumb.cloned().unwrap_or_default(),
                has_art: thumb.is_some(),
                soundcloud: matches!(&t.source, online::Source::SoundCloud { .. }),
                title: t.title.clone().into(),
                subtitle: if t.duration_secs == 0 {
                    format!("{} · {}", t.artist, t.origin)
                } else {
                    format!(
                        "{} · {} · {}",
                        t.artist,
                        online::format_duration(t.duration_secs),
                        t.origin
                    )
                }
                .into(),
                state: *state,
            }
        })
        .collect();
    ui.global::<State>().set_online(model(items));
}

fn apply_eq(ui: &AppWindow, core: &mut Core) {
    crate::eq::set_gains(core.prefs.eq);
    store::save_prefs(&core.prefs);
    show_eq(ui, core);
}

fn show_eq(ui: &AppWindow, core: &Core) {
    let st = ui.global::<State>();
    st.set_eq(model(core.prefs.eq.to_vec()));
    let preset = crate::eq::PRESETS.iter().position(|p| p.1 == core.prefs.eq);
    st.set_eq_preset(preset.map_or(-1, |p| p as i32));
}

/// Shows internal storage as people know it instead of the raw mount point.
fn folder_label(dir: &std::path::Path) -> String {
    let text = dir.to_string_lossy();
    match text.strip_prefix("/storage/emulated/0/") {
        Some(rest) => format!("Memoria interna/{rest}"),
        None => match text.strip_prefix("/storage/") {
            Some(rest) => format!("Tarjeta SD/{}", rest.split_once('/').map_or("", |r| r.1)),
            None => text.into_owned(),
        },
    }
}

fn choose_folder(ui: &AppWindow, core: &mut Core) {
    let mut actions = vec![(
        MenuAction::FolderDefault,
        "Carpeta predeterminada · Música/Tein",
    )];
    if crate::platform::sd_card().is_some() {
        actions.push((MenuAction::FolderSd, "Tarjeta SD"));
    }
    actions.push((MenuAction::FolderPick, "Elegir otra carpeta…"));
    actions.push((MenuAction::Cancel, "Cancelar"));
    core.menu_track = None;
    core.menu_group = None;
    show_menu(ui, core, "¿Dónde guardar las descargas?", actions);
}

fn apply_download_dir(ui: &AppWindow, core: &mut Core, dir: std::path::PathBuf) {
    if std::fs::create_dir_all(&dir).is_err() || !crate::paths::is_writable(&dir) {
        if crate::platform::can_write_all() {
            toast(
                ui,
                core,
                "No se puede escribir ahí. Probá con otra carpeta".into(),
            );
        } else {
            toast(
                ui,
                core,
                "Activá «Acceso a todos los archivos» y elegí la carpeta otra vez".into(),
            );
            crate::platform::ask_write_all();
        }
        return;
    }
    let label = folder_label(&dir);
    core.prefs.download_dir = Some(dir.to_string_lossy().into_owned());
    store::save_prefs(&core.prefs);
    crate::paths::set_download_dir(dir);
    ui.global::<State>().set_download_dir(label.clone().into());
    toast(ui, core, format!("Descargas en {label}"));
    if let Some((generation, wanted)) = core.pending_downloads.take() {
        if generation == core.search_gen {
            start_downloads(ui, core, wanted);
        }
    }
    let _ = slint::invoke_from_event_loop(rescan);
}

/// Queues the downloads, asking first where to save them if that was never chosen.
fn start_downloads(ui: &AppWindow, core: &mut Core, wanted: Vec<usize>) {
    let slots = MAX_DOWNLOADS.saturating_sub(core.downloads.iter().filter(|d| d.state < 2).count());
    if wanted.len() > slots {
        toast(
            ui,
            core,
            format!("Hay lugar para {slots} descargas más. El resto queda disponible en Buscar."),
        );
    }
    let wanted: Vec<_> = wanted.into_iter().take(slots).collect();
    if wanted.is_empty() {
        return;
    }
    if core.prefs.download_dir.is_none() {
        core.pending_downloads = Some((core.search_gen, wanted));
        choose_folder(ui, core);
        return;
    }
    for i in wanted {
        enqueue_download(core, i);
    }
    let mut history = 0;
    core.downloads.retain(|d| {
        if d.state < 2 {
            return true;
        }
        history += 1;
        history <= MAX_DOWNLOADS
    });
    refresh_online(ui, core);
    refresh_downloads(ui, core);
}

fn group_tracks(core: &Core, view: i32, key: &str) -> Vec<Track> {
    match view {
        1 => core
            .library
            .iter()
            .filter(|t| t.artist == key)
            .cloned()
            .collect(),
        2 => core
            .library
            .iter()
            .filter(|t| t.album == key)
            .cloned()
            .collect(),
        _ => core
            .playlists
            .iter()
            .find(|p| p.name == key)
            .map(|p| {
                p.paths
                    .iter()
                    .filter_map(|path| core.library.iter().find(|t| &t.path == path))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default(),
    }
}

fn group_art_file(view: i32, name: &str) -> std::path::PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (view, name).hash(&mut h);
    crate::paths::get()
        .data_dir
        .join("group-art")
        .join(format!("{:x}.jpg", h.finish()))
}

/// Picture chosen by the user for an artist or a playlist, if any.
fn custom_art(core: &mut Core, view: i32, name: &str) -> Option<slint::Image> {
    if view != 1 && view != 3 {
        return None;
    }
    core.custom_art
        .entry(format!("{view}\0{name}"))
        .or_insert_with(|| {
            let file = group_art_file(view, name);
            file.exists()
                .then(|| slint::Image::load_from_path(&file).ok())
                .flatten()
        })
        .clone()
}

fn pick_cover(ui: &AppWindow, core: &mut Core, target: CoverTarget) {
    if crate::platform::pick_image() {
        core.cover_target = Some(target);
    } else {
        toast(ui, core, "No se pudo abrir la galería".into());
    }
}

/// Square JPEG, at most `side` pixels wide, from any picked picture.
fn square_jpeg(path: &str, side: u32) -> Result<Vec<u8>, String> {
    use std::io::Read;
    const LIMIT: u64 = 32 * 1024 * 1024;
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut data = Vec::new();
    file.take(LIMIT + 1)
        .read_to_end(&mut data)
        .map_err(|e| e.to_string())?;
    if data.len() as u64 > LIMIT {
        return Err("La imagen supera los 32 MiB".into());
    }
    let img = crate::art::decode(&data)
        .ok_or("Imagen demasiado grande o incompatible: probá con un JPG, PNG o WebP más chico")?;
    let edge = img.width().min(img.height());
    let img = img.crop_imm(
        (img.width() - edge) / 2,
        (img.height() - edge) / 2,
        edge,
        edge,
    );
    let img = if edge > side {
        img.resize_exact(side, side, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let mut out = std::io::Cursor::new(Vec::new());
    img.to_rgb8()
        .write_to(&mut out, image::ImageFormat::Jpeg)
        .map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

fn use_picked_image(ui: &AppWindow, core: &mut Core, path: String, target: CoverTarget) {
    match target {
        CoverTarget::Tracks(tracks) => match square_jpeg(&path, 800) {
            Ok(jpeg) => write_covers(ui, core, tracks, Some(jpeg)),
            Err(e) => toast(ui, core, e),
        },
        CoverTarget::Group(view, key) => {
            let file = group_art_file(view, &key);
            let saved = square_jpeg(&path, 600).and_then(|jpeg| {
                std::fs::create_dir_all(file.parent().unwrap_or(&file))
                    .and_then(|_| std::fs::write(&file, jpeg))
                    .map_err(|e| e.to_string())
            });
            match saved {
                Ok(()) => {
                    core.custom_art.clear();
                    refresh_library(ui, core);
                    toast(ui, core, "Imagen actualizada".into());
                }
                Err(e) => toast(ui, core, e),
            }
        }
    }
    // Only Android returns an app-owned temporary copy.
    #[cfg(target_os = "android")]
    let _ = std::fs::remove_file(path);
}

/// Embeds (or removes, with `None`) the cover of the given files in the background.
fn write_covers(ui: &AppWindow, core: &mut Core, tracks: Vec<Track>, jpeg: Option<Vec<u8>>) {
    if tracks.is_empty() {
        return;
    }
    toast(ui, core, "Guardando portada…".into());
    std::thread::spawn(move || {
        let total = tracks.len();
        let mut ok = 0;
        for t in tracks {
            let path = std::path::Path::new(&t.path);
            if library::set_cover(path, jpeg.as_deref()).is_ok() {
                ok += 1;
                if let Some(new) = library::read_track(path) {
                    post(move |ui, core| apply_track_update(ui, core, new));
                }
            }
        }
        post(move |ui, core| finish_batch(ui, core, ok, total, "Portada actualizada"));
    });
}

fn finish_batch(ui: &AppWindow, core: &mut Core, ok: usize, total: usize, done: &str) {
    if ok < total && !crate::platform::can_write_all() {
        toast(
            ui,
            core,
            "Activá «Acceso a todos los archivos» para Tein y probá de nuevo".into(),
        );
        crate::platform::ask_write_all();
    } else if ok == total {
        toast(ui, core, done.into());
    } else {
        toast(ui, core, format!("Se guardaron {ok} de {total} canciones"));
    }
}

/// Saves the edit dialog for an album, an artist or a playlist.
fn save_group_edit(ui: &AppWindow, core: &mut Core) {
    let st = ui.global::<State>();
    let Some((view, old)) = core.edit_group.clone() else {
        return;
    };
    if st.get_edit_saving() {
        return;
    }
    let name = st.get_edit_title().trim().to_string();
    let artist = st.get_edit_artist().trim().to_string();
    if name.is_empty() {
        toast(ui, core, "Escribí un nombre".into());
        return;
    }
    if view == 3 {
        if name != old {
            if old == FAVORITES || core.playlists.iter().any(|p| p.name == name) {
                toast(ui, core, "Ya hay una lista con ese nombre".into());
                return;
            }
            for p in core.playlists.iter_mut().filter(|p| p.name == old) {
                p.name = name.clone();
            }
            store::save_playlists(&core.playlists);
            rename_group(core, view, &old, &name);
        }
        st.set_edit_open(false);
        refresh_library(ui, core);
        toast(ui, core, "Cambios guardados".into());
        return;
    }
    let tracks = group_tracks(core, view, &old);
    let (album, artist) = if view == 2 {
        (Some(name.clone()), (!artist.is_empty()).then_some(artist))
    } else {
        (None, Some(name.clone()))
    };
    st.set_edit_saving(true);
    std::thread::spawn(move || {
        let total = tracks.len();
        let mut ok = 0;
        for t in tracks {
            let path = std::path::Path::new(&t.path);
            if library::retag(path, album.as_deref(), artist.as_deref()).is_ok() {
                ok += 1;
                if let Some(new) = library::read_track(path) {
                    post(move |ui, core| apply_track_update(ui, core, new));
                }
            }
        }
        post(move |ui, core| {
            let st = ui.global::<State>();
            st.set_edit_saving(false);
            if ok > 0 {
                rename_group(core, view, &old, &name);
                st.set_edit_open(false);
                refresh_library(ui, core);
            }
            finish_batch(ui, core, ok, total, "Cambios guardados");
        });
    });
}

/// Keeps the open group and its custom picture after a rename.
fn rename_group(core: &mut Core, view: i32, old: &str, new: &str) {
    if old == new {
        return;
    }
    if view != 2 {
        let _ = std::fs::rename(group_art_file(view, old), group_art_file(view, new));
        core.custom_art.clear();
    }
    if core.view == view && core.group.as_deref() == Some(old) {
        core.group = Some(new.to_string());
    }
}

fn enqueue_download(core: &mut Core, i: usize) {
    let Some((track, state)) = core.online.get_mut(i) else {
        return;
    };
    if matches!(*state, 1..=3) {
        return;
    }
    *state = 1;
    let track = track.clone();
    let folder = core.online_collection.clone();
    core.next_id += 1;
    let id = core.next_id;
    let (artist, title) = download::clean_names(&track);
    core.downloads
        .retain(|d| !(d.state == 3 && d.title == format!("{artist} - {title}")));
    core.downloads.insert(
        0,
        DownloadEntry {
            id,
            title: format!("{artist} - {title}"),
            status: "En espera".into(),
            progress: 0.0,
            state: 0,
            online: Some((core.search_gen, i)),
            playlist: folder.clone(),
        },
    );
    let sent = DOWNLOADER.with(|d| {
        d.borrow()
            .as_ref()
            .is_some_and(|tx| tx.try_send(Job { id, track, folder }).is_ok())
    });
    if !sent {
        *state = 4;
        core.downloads[0].state = 3;
        core.downloads[0].status = "Cola no disponible: reintentá más tarde".into();
    }
}

fn on_download(ui: &AppWindow, core: &mut Core, update: Update) {
    let id = match &update {
        Update::Progress { id, .. } | Update::Done { id, .. } | Update::Failed { id, .. } => *id,
    };
    let Some(entry) = core.downloads.iter_mut().find(|d| d.id == id) else {
        return;
    };
    let online_state = match update {
        Update::Progress {
            status, progress, ..
        } => {
            entry.state = 1;
            entry.status = status;
            entry.progress = progress;
            2
        }
        Update::Done { track, .. } => {
            entry.state = 2;
            entry.progress = 1.0;
            entry.status = format!("Listo · {}", track.path);
            if let Some(name) = &entry.playlist {
                add_to_playlist(&mut core.playlists, name, &track.path);
            }
            if !core.library.iter().any(|t| t.path == track.path) {
                core.library.push(track);
                library::sort(&mut core.library);
                library::save_cache(&core.library);
            }
            3
        }
        Update::Failed { error, .. } => {
            entry.state = 3;
            entry.status = error;
            4
        }
    };
    let finished = online_state != 2;
    if let Some((generation, idx)) = entry.online {
        if generation == core.search_gen {
            if let Some(o) = core.online.get_mut(idx) {
                o.1 = online_state;
            }
            refresh_online(ui, core);
        }
    }
    if finished {
        refresh_library(ui, core);
    }
    refresh_downloads(ui, core);
}

fn add_to_playlist(playlists: &mut Vec<Playlist>, name: &str, path: &str) {
    let idx = match playlists.iter().position(|p| p.name == name) {
        Some(i) => i,
        None => {
            playlists.push(Playlist {
                name: name.to_string(),
                paths: Vec::new(),
            });
            playlists.len() - 1
        }
    };
    if !playlists[idx].paths.iter().any(|p| p == path) {
        playlists[idx].paths.push(path.to_string());
        store::save_playlists(playlists);
    }
}

fn refresh_downloads(ui: &AppWindow, core: &Core) {
    let st = ui.global::<State>();
    st.set_active_downloads(core.downloads.iter().filter(|d| d.state < 2).count() as i32);
    st.set_downloads(model(
        core.downloads
            .iter()
            .map(|d| DownloadItem {
                title: d.title.clone().into(),
                status: d.status.clone().into(),
                progress: d.progress,
                state: d.state,
            })
            .collect(),
    ));
}
