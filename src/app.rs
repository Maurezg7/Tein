use crate::download::{self, Job, Update};
use crate::library::{self, Track};
use crate::online::{self, OnlineTrack};
use crate::player::{self, Cmd, Event, PlayerHandle, Snapshot};
use crate::store::{self, FAVORITES, Playlist};
use crate::{AppWindow, DownloadItem, GroupItem, OnlineItem, State, TrackItem};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::sync::mpsc::Sender;

#[derive(Clone, Copy)]
enum MenuAction {
    PlayNow,
    PlayNext,
    Enqueue,
    Favorite,
    AddToList,
    RemoveFromList(usize),
    Delete,
    PlayGroup,
    ShuffleGroup,
    DeleteList,
}

struct DownloadEntry {
    id: u64,
    title: String,
    status: String,
    progress: f32,
    state: i32,
    online: Option<(u64, usize)>,
}

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
    search_gen: u64,
    downloads: Vec<DownloadEntry>,
    next_id: u64,
    menu: Vec<MenuAction>,
    menu_track: Option<Track>,
    menu_group: Option<String>,
    snapshot: Option<Snapshot>,
    toast_gen: u64,
    scanning: bool,
}

thread_local! {
    static CORE: RefCell<Core> = RefCell::new(Core::default());
    static UI: RefCell<Option<slint::Weak<AppWindow>>> = const { RefCell::new(None) };
    static PLAYER: RefCell<Option<PlayerHandle>> = const { RefCell::new(None) };
    static DOWNLOADER: RefCell<Option<Sender<Job>>> = const { RefCell::new(None) };
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
    UI.with(|u| *u.borrow_mut() = Some(ui.as_weak()));
    let settings = store::load_settings();
    let st = ui.global::<State>();
    st.set_volume(settings.volume);
    st.set_shuffle(settings.shuffle);
    st.set_repeat_mode(settings.repeat as i32);

    PLAYER.with(|p| {
        *p.borrow_mut() = Some(player::spawn(settings, |e| {
            post(move |ui, core| on_player(ui, core, e))
        }))
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
    });
    with(|ui, core| refresh_library(ui, core));
    rescan();
    bind(&ui);
    ui.run()
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
                open_track_menu(ui, core, t);
            }
        })
    });
    st.on_current_menu(|| {
        with(|ui, core| {
            if let Some(t) = core.snapshot.as_ref().and_then(|s| s.current.clone()) {
                open_track_menu(ui, core, t);
            }
        })
    });
    st.on_group_menu(|i| {
        with(|ui, core| {
            let Some(key) = core.group_keys.get(i as usize).cloned() else {
                return;
            };
            let mut actions = vec![
                (MenuAction::PlayGroup, "Reproducir"),
                (MenuAction::ShuffleGroup, "Reproducir aleatorio"),
            ];
            if key != FAVORITES {
                actions.push((MenuAction::DeleteList, "Eliminar lista"));
            }
            core.menu_group = Some(key.clone());
            core.menu_track = None;
            show_menu(ui, core, &key, actions);
        })
    });
    st.on_menu_action(|i| with(|ui, core| menu_action(ui, core, i as usize)));
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
    st.on_queue_remove(|i| send(Cmd::Remove(i as usize)));
    st.on_toggle_fav(|| {
        with(|ui, core| {
            if let Some(t) = core.snapshot.as_ref().and_then(|s| s.current.clone()) {
                toggle_favorite(ui, core, &t);
            }
        })
    });

    st.on_online_search(|q| search(q.to_string()));
    st.on_online_download(|i| with(|ui, core| enqueue_download(ui, core, i as usize)));
    st.on_online_download_all(|| {
        with(|ui, core| {
            for i in 0..core.online.len() {
                if matches!(core.online[i].1, 0 | 4) {
                    enqueue_download(ui, core, i);
                }
            }
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
                core.library = tracks;
                refresh_library(ui, core);
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

    let mut groups: Vec<(String, String)> = vec![];
    if core.group.is_none() && core.view > 0 {
        if core.view == 3 {
            groups = core
                .playlists
                .iter()
                .filter(|p| filter.is_empty() || p.name.to_lowercase().contains(&filter))
                .map(|p| (p.name.clone(), format!("{} canciones", p.paths.len())))
                .collect();
        } else {
            let mut map: std::collections::BTreeMap<String, (String, usize)> = Default::default();
            for t in core.library.iter().filter(|t| matches(t)) {
                let key = if core.view == 1 { &t.artist } else { &t.album };
                let entry = map
                    .entry(key.to_lowercase())
                    .or_insert_with(|| (key.clone(), 0));
                entry.1 += 1;
            }
            groups = map
                .into_values()
                .map(|(k, n)| {
                    (
                        k,
                        format!("{n} {}", if n == 1 { "canción" } else { "canciones" }),
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
            })
            .collect(),
    ));
    st.set_groups(model(
        groups
            .iter()
            .map(|(n, i)| GroupItem {
                name: n.clone().into(),
                info: i.clone().into(),
            })
            .collect(),
    ));
    st.set_group_title(core.group.clone().unwrap_or_default().into());
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
    core.group_keys = groups.into_iter().map(|(n, _)| n).collect();
    refresh_fav(ui, core);
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

fn open_track_menu(ui: &AppWindow, core: &mut Core, track: Track) {
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
    if let (3, Some(g)) = (core.view, &core.group) {
        if let Some(idx) = core.playlists.iter().position(|p| &p.name == g) {
            actions.push((MenuAction::RemoveFromList(idx), "Quitar de esta lista"));
        }
    }
    actions.push((MenuAction::Delete, "Eliminar archivo"));
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
                match std::fs::remove_file(&t.path) {
                    Ok(()) => {
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
                    Err(e) => toast(ui, core, format!("No se pudo eliminar: {e}")),
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
        Event::Rescan => rescan(),
        Event::Changed(snap) => {
            let changed_track = core
                .snapshot
                .as_ref()
                .and_then(|s| s.current.as_ref())
                .map(|t| &t.path)
                != snap.current.as_ref().map(|t| &t.path);
            st.set_has_current(snap.current.is_some());
            if let Some(t) = &snap.current {
                st.set_now_title(t.title.clone().into());
                st.set_now_artist(t.artist.clone().into());
            }
            if changed_track {
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
            st.set_queue(model(
                snap.queue
                    .iter()
                    .enumerate()
                    .map(|(i, t)| TrackItem {
                        title: t.title.clone().into(),
                        subtitle: t.artist.clone().into(),
                        trailing: online::format_duration(t.duration_secs).into(),
                        playing: i == snap.index,
                    })
                    .collect(),
            ));
            core.snapshot = Some(snap);
            if changed_track {
                refresh_library(ui, core);
            }
        }
    }
}

fn search(query: String) {
    let query = query.trim().to_string();
    if query.is_empty() {
        return;
    }
    with(|ui, core| {
        core.search_gen += 1;
        let generation = core.search_gen;
        let st = ui.global::<State>();
        st.set_online_busy(true);
        st.set_online_status(
            if query.contains("://") || query.starts_with("spotify:") {
                "Leyendo link…"
            } else {
                "Buscando…"
            }
            .into(),
        );
        std::thread::spawn(move || {
            let result = online::resolve(&query);
            post(move |ui, core| {
                if core.search_gen != generation {
                    return;
                }
                let st = ui.global::<State>();
                st.set_online_busy(false);
                match result {
                    Ok(res) => {
                        st.set_online_status(if res.tracks.is_empty() {
                            "Sin resultados".into()
                        } else {
                            res.label.into()
                        });
                        core.online = res.tracks.into_iter().map(|t| (t, 0)).collect();
                    }
                    Err(e) => {
                        st.set_online_status(format!("Error: {e}").into());
                        core.online.clear();
                    }
                }
                refresh_online(ui, core);
            });
        });
    });
}

fn refresh_online(ui: &AppWindow, core: &Core) {
    let items = core
        .online
        .iter()
        .map(|(t, state)| OnlineItem {
            title: t.title.clone().into(),
            subtitle: format!(
                "{} · {} · {}",
                t.artist,
                online::format_duration(t.duration_secs),
                t.origin
            )
            .into(),
            state: *state,
        })
        .collect();
    ui.global::<State>().set_online(model(items));
}

fn enqueue_download(ui: &AppWindow, core: &mut Core, i: usize) {
    let Some((track, state)) = core.online.get_mut(i) else {
        return;
    };
    if matches!(*state, 1..=3) {
        return;
    }
    *state = 1;
    let track = track.clone();
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
        },
    );
    DOWNLOADER.with(|d| {
        if let Some(tx) = d.borrow().as_ref() {
            let _ = tx.send(Job { id, track });
        }
    });
    refresh_online(ui, core);
    refresh_downloads(ui, core);
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
