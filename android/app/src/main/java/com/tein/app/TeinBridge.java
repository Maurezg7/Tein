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
    public static final int ACTION_RESCAN = 10;

    private static final ConcurrentLinkedQueue<Long> ACTIONS = new ConcurrentLinkedQueue<>();
    private static final Handler MAIN = new Handler(Looper.getMainLooper());
    private static Context app;

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

}
