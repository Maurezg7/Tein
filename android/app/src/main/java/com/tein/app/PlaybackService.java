package com.tein.app;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.content.pm.ServiceInfo;
import android.media.AudioAttributes;
import android.media.AudioFocusRequest;
import android.media.AudioPlaybackConfiguration;
import android.media.AudioManager;
import android.media.MediaMetadata;
import android.media.session.MediaSession;
import android.media.session.PlaybackState;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;
import android.os.IBinder;
import android.os.PowerManager;

/** Keeps the process alive while music plays and exposes media controls. */
public class PlaybackService extends Service implements AudioManager.OnAudioFocusChangeListener {
    static volatile PlaybackService instance;

    private static final String CHANNEL = "playback";
    private static final int NOTIFICATION_ID = 1;

    private MediaSession session;
    private AudioManager audio;
    private AudioFocusRequest focusRequest;
    private boolean hasFocus;
    private boolean pausedByFocusLoss;
    private boolean foreground;
    private PowerManager.WakeLock wakeLock;
    private final Handler handler = new Handler(Looper.getMainLooper());
    // Another app took the audio for good: wait until it goes quiet, then resume.
    private boolean waitingForOthers;
    private boolean sawOther;
    private boolean parkedSeen;
    private int quietChecks;
    private long waitingSince;
    private static final long WATCH_MS = 1500;
    private static final long MAX_WAIT_MS = 3 * 60 * 60 * 1000L;
    private static final long PAUSED_GRACE_MS = 10 * 60 * 1000L;

    private final Runnable watchOthers = new Runnable() {
        @Override
        public void run() {
            if (!waitingForOthers) return;
            if (TeinBridge.playing) {
                // Our stream is still closing; other players cannot be told apart yet.
                handler.postDelayed(this, WATCH_MS);
                return;
            }
            if (System.currentTimeMillis() - waitingSince > MAX_WAIT_MS) {
                stopWaiting();
                refresh();
                return;
            }
            if (othersPlaying()) {
                sawOther = true;
                quietChecks = 0;
            } else if (sawOther && ++quietChecks >= 2) {
                stopWaiting();
                hasFocus = audio.requestAudioFocus(focusRequest) == AudioManager.AUDIOFOCUS_REQUEST_GRANTED;
                if (hasFocus) TeinBridge.push(TeinBridge.ACTION_PLAY);
                else refresh();
                return;
            }
            handler.postDelayed(this, WATCH_MS);
        }
    };

    private final Runnable detach = new Runnable() {
        @Override
        public void run() {
            if (foreground && !TeinBridge.playing && !pausedByFocusLoss) {
                stopForeground(STOP_FOREGROUND_DETACH);
                foreground = false;
            }
        }
    };

    private boolean othersPlaying() {
        for (AudioPlaybackConfiguration c : audio.getActivePlaybackConfigurations()) {
            int usage = c.getAudioAttributes().getUsage();
            if (usage == AudioAttributes.USAGE_MEDIA || usage == AudioAttributes.USAGE_GAME
                    || usage == AudioAttributes.USAGE_UNKNOWN) return true;
        }
        return false;
    }

    private void stopWaiting() {
        waitingForOthers = false;
        pausedByFocusLoss = false;
        handler.removeCallbacks(watchOthers);
    }

    private void userPaused() {
        if (waitingForOthers || pausedByFocusLoss) {
            stopWaiting();
            refresh();
        }
        TeinBridge.push(TeinBridge.ACTION_PAUSE);
    }

    private final BroadcastReceiver noisy = new BroadcastReceiver() {
        @Override
        public void onReceive(Context context, Intent intent) {
            TeinBridge.push(TeinBridge.ACTION_PAUSE);
        }
    };

    @Override
    public void onCreate() {
        super.onCreate();
        instance = this;
        NotificationManager nm = getSystemService(NotificationManager.class);
        NotificationChannel ch = new NotificationChannel(CHANNEL, "Reproducción", NotificationManager.IMPORTANCE_LOW);
        ch.setShowBadge(false);
        nm.createNotificationChannel(ch);

        audio = getSystemService(AudioManager.class);
        wakeLock = getSystemService(PowerManager.class).newWakeLock(
                PowerManager.PARTIAL_WAKE_LOCK, "Tein:Playback");
        wakeLock.setReferenceCounted(false);
        focusRequest = new AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN)
                .setAudioAttributes(new AudioAttributes.Builder()
                        .setUsage(AudioAttributes.USAGE_MEDIA)
                        .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
                        .build())
                .setOnAudioFocusChangeListener(this)
                .build();

        session = new MediaSession(this, "Tein");
        session.setCallback(new MediaSession.Callback() {
            @Override public void onPlay() { TeinBridge.push(TeinBridge.ACTION_PLAY); }
            @Override public void onPause() { userPaused(); }
            @Override public void onSkipToNext() { TeinBridge.push(TeinBridge.ACTION_NEXT); }
            @Override public void onSkipToPrevious() { TeinBridge.push(TeinBridge.ACTION_PREV); }
            @Override public void onStop() { userPaused(); }
            @Override public void onSeekTo(long positionMs) { TeinBridge.seek(positionMs); }
        });
        session.setSessionActivity(openApp());
        session.setActive(true);
        registerReceiver(noisy, new IntentFilter(AudioManager.ACTION_AUDIO_BECOMING_NOISY));
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        String action = intent != null ? intent.getAction() : null;
        if ("toggle".equals(action)) TeinBridge.push(TeinBridge.ACTION_TOGGLE);
        else if ("next".equals(action)) TeinBridge.push(TeinBridge.ACTION_NEXT);
        else if ("prev".equals(action)) TeinBridge.push(TeinBridge.ACTION_PREV);
        refresh();
        return START_NOT_STICKY;
    }

    void refresh() {
        boolean playing = TeinBridge.playing;
        if (playing && !wakeLock.isHeld()) wakeLock.acquire();
        if (!playing && wakeLock.isHeld()) wakeLock.release();
        if (playing) {
            // Playing again after being parked means the user resumed by hand.
            if (waitingForOthers && parkedSeen) stopWaiting();
            if (!waitingForOthers) pausedByFocusLoss = false;
            if (!hasFocus && !waitingForOthers) hasFocus = audio.requestAudioFocus(focusRequest) == AudioManager.AUDIOFOCUS_REQUEST_GRANTED;
        } else if (waitingForOthers) {
            parkedSeen = true;
        } else if (hasFocus && !pausedByFocusLoss) {
            audio.abandonAudioFocusRequest(focusRequest);
            hasFocus = false;
        }

        session.setMetadata(new MediaMetadata.Builder()
                .putString(MediaMetadata.METADATA_KEY_TITLE, TeinBridge.title)
                .putString(MediaMetadata.METADATA_KEY_ARTIST, TeinBridge.artist)
                .putLong(MediaMetadata.METADATA_KEY_DURATION, TeinBridge.durationMs)
                .build());
        session.setPlaybackState(new PlaybackState.Builder()
                .setActions(PlaybackState.ACTION_PLAY | PlaybackState.ACTION_PAUSE | PlaybackState.ACTION_PLAY_PAUSE
                        | PlaybackState.ACTION_SKIP_TO_NEXT | PlaybackState.ACTION_SKIP_TO_PREVIOUS
                        | (TeinBridge.durationMs > 0 ? PlaybackState.ACTION_SEEK_TO : 0))
                .setState(playing ? PlaybackState.STATE_PLAYING : PlaybackState.STATE_PAUSED,
                        TeinBridge.positionMs, playing ? 1f : 0f)
                .build());

        Notification n = new Notification.Builder(this, CHANNEL)
                .setSmallIcon(R.drawable.ic_note)
                .setContentTitle(TeinBridge.title)
                .setContentText(TeinBridge.artist)
                .setContentIntent(openApp())
                .setVisibility(Notification.VISIBILITY_PUBLIC)
                .setOngoing(playing)
                .setShowWhen(false)
                .addAction(new Notification.Action.Builder(R.drawable.ic_prev, "Anterior", control("prev", 1)).build())
                .addAction(new Notification.Action.Builder(playing ? R.drawable.ic_pause : R.drawable.ic_play,
                        playing ? "Pausa" : "Reproducir", control("toggle", 2)).build())
                .addAction(new Notification.Action.Builder(R.drawable.ic_next, "Siguiente", control("next", 3)).build())
                .setStyle(new Notification.MediaStyle()
                        .setMediaSession(session.getSessionToken())
                        .setShowActionsInCompactView(0, 1, 2))
                .build();

        handler.removeCallbacks(detach);
        if (playing || foreground) {
            // Stay in the foreground while paused so Android keeps the queue alive.
            try {
                if (Build.VERSION.SDK_INT >= 29) {
                    startForeground(NOTIFICATION_ID, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK);
                } else {
                    startForeground(NOTIFICATION_ID, n);
                }
                foreground = true;
            } catch (RuntimeException e) {
                android.util.Log.w("tein", "No se pudo pasar a primer plano", e);
                getSystemService(NotificationManager.class).notify(NOTIFICATION_ID, n);
            }
            if (!playing && !pausedByFocusLoss) handler.postDelayed(detach, PAUSED_GRACE_MS);
        } else {
            getSystemService(NotificationManager.class).notify(NOTIFICATION_ID, n);
        }
    }

    private PendingIntent control(String action, int code) {
        Intent i = new Intent(this, PlaybackService.class).setAction(action);
        return PendingIntent.getService(this, code, i, PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
    }

    private PendingIntent openApp() {
        Intent i = new Intent(this, MainActivity.class).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP);
        return PendingIntent.getActivity(this, 0, i, PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
    }

    @Override
    public void onAudioFocusChange(int change) {
        switch (change) {
            case AudioManager.AUDIOFOCUS_LOSS:
                hasFocus = false;
                if (TeinBridge.playing || pausedByFocusLoss) {
                    pausedByFocusLoss = true;
                    waitingForOthers = true;
                    sawOther = false;
                    parkedSeen = false;
                    quietChecks = 0;
                    waitingSince = System.currentTimeMillis();
                    TeinBridge.push(TeinBridge.ACTION_PARK);
                    handler.removeCallbacks(watchOthers);
                    handler.postDelayed(watchOthers, WATCH_MS);
                }
                break;
            case AudioManager.AUDIOFOCUS_LOSS_TRANSIENT:
            case AudioManager.AUDIOFOCUS_LOSS_TRANSIENT_CAN_DUCK:
                if (TeinBridge.playing) {
                    pausedByFocusLoss = true;
                    TeinBridge.push(TeinBridge.ACTION_PAUSE);
                }
                break;
            case AudioManager.AUDIOFOCUS_GAIN:
                hasFocus = true;
                if (pausedByFocusLoss) {
                    stopWaiting();
                    TeinBridge.push(TeinBridge.ACTION_PLAY);
                }
                break;
            default:
                break;
        }
    }

    @Override
    public void onTaskRemoved(Intent rootIntent) {
        if (!TeinBridge.playing) stopSelf();
    }

    @Override
    public void onDestroy() {
        instance = null;
        handler.removeCallbacksAndMessages(null);
        if (wakeLock.isHeld()) wakeLock.release();
        unregisterReceiver(noisy);
        if (hasFocus) audio.abandonAudioFocusRequest(focusRequest);
        session.release();
        getSystemService(NotificationManager.class).cancel(NOTIFICATION_ID);
        super.onDestroy();
    }

    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }
}
