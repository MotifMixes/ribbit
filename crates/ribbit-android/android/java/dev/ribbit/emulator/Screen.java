package dev.ribbit.emulator;

import android.app.Activity;
import android.view.WindowManager;

/**
 * Keeps the screen on while a game plays. The window's flags are changed on
 * Android's UI thread (the native code asks from its own thread).
 */
public class Screen {
    public static void keepOn(final Activity activity, final boolean on) {
        activity.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (on) {
                    activity.getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
                } else {
                    activity.getWindow().clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
                }
            }
        });
    }
}
