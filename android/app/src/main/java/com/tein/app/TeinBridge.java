package com.tein.app;

import android.content.Context;
import android.content.Intent;
import android.os.Handler;
import android.os.Looper;

import java.util.concurrent.ConcurrentLinkedQueue;

/** Static entry points called from Rust through JNI. */
public final class TeinBridge {
    public static final int ACTION_TOGGLE = 1;
    public static final int ACTION_NEXT = 2;
    public static final int ACTION_PREV = 3;
    public static final int ACTION_PAUSE = 4;
    public static final int ACTION_PLAY = 5;
    public static final int ACTION_PARK = 6;
    public static final int ACTION_RESCAN = 10;
    public static final int ACTION_FOLDER = 11;
    public static final int ACTION_IMAGE = 12;

    private static final ConcurrentLinkedQueue<Long> ACTIONS = new ConcurrentLinkedQueue<>();
    private static final Handler MAIN = new Handler(Looper.getMainLooper());
    private static Context app;
    static java.lang.ref.WeakReference<MainActivity> activity = new java.lang.ref.WeakReference<>(null);
    static volatile String pickedFolder = "";
    static volatile String pickedImage = "";

    static volatile String title = "";
    static volatile String artist = "";
    static volatile boolean playing;
    static long positionMs;
    static long durationMs;

    private TeinBridge() {}

    static void init(Context context) {
        app = context.getApplicationContext();
    }

    static void push(int action) {
        ACTIONS.add((long) action);
    }

    static void seek(long ms) {
        ACTIONS.add(-1L - Math.max(0L, Math.min(ms, durationMs)));
    }

    public static long poll() {
        Long a = ACTIONS.poll();
        return a == null ? 0 : a;
    }

    public static void update(String newTitle, String newArtist, boolean isPlaying, long pos, long duration) {
        MAIN.post(() -> {
            title = newTitle;
            artist = newArtist;
            playing = isPlaying;
            positionMs = pos;
            durationMs = duration;
            Context c = app;
            if (c == null) return;
            PlaybackService service = PlaybackService.instance;
            if (title.isEmpty()) {
                if (service != null) service.stopSelf();
            } else if (service != null) {
                service.refresh();
            } else if (playing) {
                try {
                    c.startForegroundService(new Intent(c, PlaybackService.class));
                } catch (RuntimeException e) {
                    android.util.Log.w("tein", "No se pudo iniciar el servicio", e);
                }
            }
        });
    }

    /** Whether files of other apps can be rewritten (tags/covers) on Android 11+. */
    public static boolean canWriteAll() {
        return android.os.Build.VERSION.SDK_INT < 30
                || android.os.Environment.isExternalStorageManager();
    }

    /** Opens the "All files access" settings page for Tein. */
    public static void askWriteAll() {
        MAIN.post(() -> {
            Context c = app;
            if (c == null || android.os.Build.VERSION.SDK_INT < 30) return;
            Intent i = new Intent(android.provider.Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION,
                    android.net.Uri.parse("package:" + c.getPackageName()));
            i.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
            try {
                c.startActivity(i);
            } catch (RuntimeException e) {
                Intent all = new Intent(android.provider.Settings.ACTION_MANAGE_ALL_FILES_ACCESS_PERMISSION);
                all.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
                c.startActivity(all);
            }
        });
    }

    /** Opens the system folder picker; the result arrives as ACTION_FOLDER. */
    public static void pickFolder() {
        MAIN.post(() -> {
            MainActivity a = activity.get();
            if (a == null) return;
            try {
                a.startActivityForResult(new Intent(Intent.ACTION_OPEN_DOCUMENT_TREE), MainActivity.PICK_FOLDER);
            } catch (RuntimeException e) {
                android.util.Log.w("tein", "Sin selector de carpetas", e);
                pickedFolder = "";
                push(ACTION_FOLDER);
            }
        });
    }

    /** Opens the system image picker; the result arrives as ACTION_IMAGE. */
    public static void pickImage() {
        MAIN.post(() -> {
            MainActivity a = activity.get();
            if (a == null) return;
            try {
                Intent i = new Intent(Intent.ACTION_GET_CONTENT).setType("image/*")
                        .addCategory(Intent.CATEGORY_OPENABLE);
                a.startActivityForResult(i, MainActivity.PICK_IMAGE);
            } catch (RuntimeException e) {
                android.util.Log.w("tein", "Sin selector de imágenes", e);
                pickedImage = "";
                push(ACTION_IMAGE);
            }
        });
    }

    public static String takeImage() {
        String f = pickedImage;
        pickedImage = "";
        return f;
    }

    public static String takeFolder() {
        String f = pickedFolder;
        pickedFolder = "";
        return f;
    }

    /** "root\nappDir" of the removable card, or "" when there is none. */
    public static String sdCard() {
        Context c = app;
        if (c == null) return "";
        try {
            java.io.File[] dirs = c.getExternalFilesDirs(null);
            for (int i = 1; i < dirs.length; i++) {
                java.io.File d = dirs[i];
                if (d == null || !android.os.Environment.isExternalStorageRemovable(d)) continue;
                if (!android.os.Environment.MEDIA_MOUNTED.equals(android.os.Environment.getExternalStorageState(d))) continue;
                String path = d.getAbsolutePath();
                int at = path.indexOf("/Android/");
                if (at > 0) return path.substring(0, at) + "\n" + path;
            }
        } catch (RuntimeException e) {
            android.util.Log.w("tein", "No se pudo leer la tarjeta SD", e);
        }
        return "";
    }
}
