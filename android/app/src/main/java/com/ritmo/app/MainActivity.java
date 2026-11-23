package com.ritmo.app;

import android.Manifest;
import android.app.NativeActivity;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.os.Build;
import android.os.Bundle;

import java.util.ArrayList;
import java.util.List;

public class MainActivity extends NativeActivity {
    @Override
    protected void onCreate(Bundle savedInstanceState) {
        RitmoBridge.init(this);
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
            if (Build.VERSION.SDK_INT <= 28) wanted.add(Manifest.permission.WRITE_EXTERNAL_STORAGE);
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
        RitmoBridge.push(RitmoBridge.ACTION_RESCAN);
    }

    @Override
    public void onBackPressed() {
        // Keep the native UI alive (and the music playing) instead of destroying it.
        moveTaskToBack(true);
    }

    @Override
    protected void onDestroy() {
        super.onDestroy();
        if (isFinishing()) RitmoBridge.shutdown(this);
    }

    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        setIntent(intent);
    }
}
