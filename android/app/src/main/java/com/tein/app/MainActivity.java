package com.tein.app;

import android.Manifest;
import android.app.NativeActivity;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.os.Build;
import android.os.Bundle;

import java.util.ArrayList;
import java.util.List;

public class MainActivity extends NativeActivity {
    static final int PICK_FOLDER = 2;
    static final int PICK_IMAGE = 3;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        TeinBridge.init(this);
        TeinBridge.activity = new java.lang.ref.WeakReference<>(this);
        super.onCreate(savedInstanceState);
        requestNeededPermissions();
    }

    private void requestNeededPermissions() {
        List<String> wanted = new ArrayList<>();
        if (Build.VERSION.SDK_INT >= 33) {
            wanted.add(Manifest.permission.READ_MEDIA_AUDIO);
            wanted.add(Manifest.permission.POST_NOTIFICATIONS);
        } else {
            wanted.add(Manifest.permission.READ_EXTERNAL_STORAGE);
            if (Build.VERSION.SDK_INT <= 29) wanted.add(Manifest.permission.WRITE_EXTERNAL_STORAGE);
        }
        List<String> missing = new ArrayList<>();
        for (String p : wanted) {
            if (checkSelfPermission(p) != PackageManager.PERMISSION_GRANTED) missing.add(p);
        }
        if (!missing.isEmpty()) requestPermissions(missing.toArray(new String[0]), 1);
    }

    @Override
    public void onRequestPermissionsResult(int requestCode, String[] permissions, int[] grantResults) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults);
        TeinBridge.push(TeinBridge.ACTION_RESCAN);
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (resultCode != RESULT_OK || data == null || data.getData() == null) return;
        if (requestCode == PICK_FOLDER) {
            TeinBridge.pickedFolder = folderPath(data.getData());
            TeinBridge.push(TeinBridge.ACTION_FOLDER);
        } else if (requestCode == PICK_IMAGE) {
            final android.net.Uri uri = data.getData();
            new Thread(() -> {
                TeinBridge.pickedImage = copyToCache(uri);
                TeinBridge.push(TeinBridge.ACTION_IMAGE);
            }).start();
        }
    }

    /** Copies a picked image into the cache so native code can read it by path. */
    private String copyToCache(android.net.Uri uri) {
        java.io.File out = new java.io.File(getCacheDir(), "picked-image");
        try (java.io.InputStream in = getContentResolver().openInputStream(uri);
             java.io.OutputStream os = new java.io.FileOutputStream(out)) {
            if (in == null) return "";
            byte[] buf = new byte[65536];
            long total = 0;
            int n;
            while ((n = in.read(buf)) > 0) {
                total += n;
                if (total > 40L * 1024 * 1024) return "";
                os.write(buf, 0, n);
            }
            return out.getAbsolutePath();
        } catch (java.io.IOException | RuntimeException e) {
            android.util.Log.w("tein", "No se pudo leer la imagen", e);
            return "";
        }
    }

    /** Real path of a folder picked in internal storage or an SD card ("" if it has none). */
    private static String folderPath(android.net.Uri tree) {
        try {
            if (!"com.android.externalstorage.documents".equals(tree.getAuthority())) return "";
            String id = android.provider.DocumentsContract.getTreeDocumentId(tree);
            int colon = id.indexOf(':');
            if (colon < 0) return "";
            String volume = id.substring(0, colon);
            String rest = id.substring(colon + 1);
            String root = "primary".equals(volume)
                    ? android.os.Environment.getExternalStorageDirectory().getAbsolutePath()
                    : "/storage/" + volume;
            return rest.isEmpty() ? root : root + "/" + rest;
        } catch (RuntimeException e) {
            return "";
        }
    }

    @Override
    public void onBackPressed() {
        // Keep the native UI alive (and the music playing) instead of destroying it.
        moveTaskToBack(true);
    }

    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        setIntent(intent);
    }
}
