package dev.ribbit.emulator;

import android.app.Activity;
import android.app.Presentation;
import android.content.Context;
import android.hardware.display.DisplayManager;
import android.view.Display;
import android.view.MotionEvent;
import android.view.Surface;
import android.view.SurfaceHolder;
import android.view.SurfaceView;
import android.view.View;

/**
 * EXPERIMENTAL: a picture on the device's second display (a dual-screen
 * handheld such as the AYN Thor), for the Leapster L-MAX's second picture.
 * An Android Presentation holding one SurfaceView; the native code draws
 * into its Surface and reads touches on it. Not tried on such a device.
 *
 * The native side asks from its own thread: the Presentation is made and
 * dismissed on Android's UI thread. Whenever the surface comes, changes or
 * goes, `generation` changes; the native side then lets go of the old one
 * (`released`), which surfaceDestroyed waits for (briefly) before Android
 * takes the surface away.
 */
public class SecondScreen {
    private static final Object LOCK = new Object();
    private static Presentation presentation;
    private static Surface surface;
    private static int width, height;
    private static volatile int generation;
    /** The native side draws to the surface. */
    private static boolean inUse;
    private static volatile boolean touching;
    private static volatile float touchX, touchY;

    /** The first display for presentations (not the one the app is on). */
    private static Display secondDisplay(Activity activity) {
        DisplayManager dm = (DisplayManager) activity.getSystemService(Context.DISPLAY_SERVICE);
        if (dm == null) {
            return null;
        }
        Display own = activity.getWindowManager().getDefaultDisplay();
        for (Display d : dm.getDisplays(DisplayManager.DISPLAY_CATEGORY_PRESENTATION)) {
            if (own == null || d.getDisplayId() != own.getDisplayId()) {
                return d;
            }
        }
        return null;
    }

    /** Shows (or dismisses) the picture's window; false if there's no second display. */
    public static boolean show(final Activity activity, final boolean on) {
        if (on && secondDisplay(activity) == null) {
            return false;
        }
        activity.runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (!on) {
                    if (presentation != null) {
                        presentation.dismiss();
                        presentation = null;
                    }
                    return;
                }
                if (presentation != null) {
                    return;
                }
                Display display = secondDisplay(activity);
                if (display == null) {
                    return;
                }
                SurfaceView view = new SurfaceView(activity);
                view.getHolder().addCallback(new SurfaceHolder.Callback() {
                    @Override
                    public void surfaceCreated(SurfaceHolder holder) {}

                    @Override
                    public void surfaceChanged(SurfaceHolder holder, int format, int w, int h) {
                        synchronized (LOCK) {
                            surface = holder.getSurface();
                            width = w;
                            height = h;
                            generation++;
                        }
                    }

                    @Override
                    public void surfaceDestroyed(SurfaceHolder holder) {
                        synchronized (LOCK) {
                            surface = null;
                            generation++;
                            long until = System.currentTimeMillis() + 500;
                            while (inUse) {
                                long left = until - System.currentTimeMillis();
                                if (left <= 0) {
                                    break;
                                }
                                try {
                                    LOCK.wait(left);
                                } catch (InterruptedException e) {
                                    break;
                                }
                            }
                        }
                    }
                });
                view.setOnTouchListener(new View.OnTouchListener() {
                    @Override
                    public boolean onTouch(View v, MotionEvent e) {
                        int action = e.getActionMasked();
                        touchX = e.getX();
                        touchY = e.getY();
                        touching = action != MotionEvent.ACTION_UP && action != MotionEvent.ACTION_CANCEL;
                        return true;
                    }
                });
                presentation = new Presentation(activity, display);
                presentation.setContentView(view);
                try {
                    presentation.show();
                } catch (RuntimeException e) {
                    // The display went away meanwhile.
                    presentation = null;
                }
            }
        });
        return true;
    }

    public static int generation() {
        return generation;
    }

    /** The surface to draw to now (null: none), marked in use until `released`. */
    public static Surface acquire() {
        synchronized (LOCK) {
            if (surface == null || !surface.isValid()) {
                return null;
            }
            inUse = true;
            return surface;
        }
    }

    public static int width() {
        return width;
    }

    public static int height() {
        return height;
    }

    public static void released() {
        synchronized (LOCK) {
            inUse = false;
            LOCK.notifyAll();
        }
    }

    /** A finger's position on the surface, or null. */
    public static float[] touch() {
        return touching ? new float[] {touchX, touchY} : null;
    }
}
