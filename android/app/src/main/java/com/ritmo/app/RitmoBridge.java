package com.ritmo.app;

import android.content.Context;
import android.content.Intent;
import android.os.Handler;
import android.os.Looper;

import java.util.concurrent.ConcurrentLinkedQueue;

/** Static entry points called from Rust through JNI. */
public final class RitmoBridge {
    public static final int ACTION_TOGGLE = 1;
    public static final int ACTION_NEXT = 2;
    public static final int ACTION_PREV = 3;
    public static final int ACTION_PAUSE = 4;
    public static final int ACTION_PLAY = 5;
    public static final int ACTION_RESCAN = 10;

    private static final ConcurrentLinkedQueue<Integer> ACTIONS = new ConcurrentLinkedQueue<>();
    private static final Handler MAIN = new Handler(Looper.getMainLooper());
    private static Context app;

    static volatile String title = "";
    static volatile String artist = "";
    static volatile boolean playing;

    private RitmoBridge() {}

    static void init(Context context) {
        app = context.getApplicationContext();
    }

    static void push(int action) {
        ACTIONS.add(action);
    }

    public static int poll() {
        Integer a = ACTIONS.poll();
        return a == null ? 0 : a;
    }

    public static void update(String newTitle, String newArtist, boolean isPlaying) {
        title = newTitle;
        artist = newArtist;
        playing = isPlaying;
        MAIN.post(() -> {
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
                    android.util.Log.w("ritmo", "No se pudo iniciar el servicio", e);
                }
            }
        });
    }

    static void shutdown(Context c) {
        c.stopService(new Intent(c, PlaybackService.class));
        android.os.Process.killProcess(android.os.Process.myPid());
    }
}
