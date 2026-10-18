package dev.ribbit.emulator;

import android.app.Activity;
import android.content.Intent;
import android.database.Cursor;
import android.net.Uri;
import android.os.Bundle;
import android.os.Environment;
import android.provider.DocumentsContract;
import android.provider.OpenableColumns;

import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.io.OutputStream;

/**
 * Android's own file picker for Ribbit (a NativeActivity, which can't take
 * an activity's answer itself). The native code starts it with start() and
 * asks poll() for the answer while it is open.
 *
 * Ribbit reads files by path (it has "All files access"), so the picker's
 * content:// answer is turned into one: phone storage and SD cards map to
 * /storage/...; a file from anywhere else (Downloads by number, Drive...) is
 * copied into Ribbit's own folder first.
 */
public class PickerActivity extends Activity {
    private static final Object LOCK = new Object();
    private static boolean answered;
    private static String answer;

    private boolean folder;

    /** Opens the picker for a file, or a folder. */
    public static void start(Activity from, boolean folder) {
        synchronized (LOCK) {
            answered = false;
            answer = null;
        }
        Intent intent = new Intent(from, PickerActivity.class);
        intent.putExtra("folder", folder);
        from.startActivity(intent);
    }

    /**
     * null while the picker is open; "" once cancelled; else the chosen path,
     * or a reason it can't be used starting with "!". Each answer is given once.
     */
    public static String poll() {
        synchronized (LOCK) {
            if (!answered) {
                return null;
            }
            answered = false;
            return answer == null ? "" : answer;
        }
    }

    @Override
    protected void onCreate(Bundle saved) {
        super.onCreate(saved);
        folder = getIntent().getBooleanExtra("folder", false);
        if (saved != null) {
            // Recreated while the picker was open: its answer still comes.
            return;
        }
        Intent pick;
        if (folder) {
            pick = new Intent(Intent.ACTION_OPEN_DOCUMENT_TREE);
        } else {
            pick = new Intent(Intent.ACTION_OPEN_DOCUMENT);
            pick.addCategory(Intent.CATEGORY_OPENABLE);
            pick.setType("*/*");
        }
        try {
            startActivityForResult(pick, 1);
        } catch (Exception e) {
            finishWith("!No file picker on this device: " + e.getMessage());
        }
    }

    @Override
    protected void onActivityResult(int request, int result, Intent data) {
        if (result != RESULT_OK || data == null || data.getData() == null) {
            finishWith("");
            return;
        }
        String path;
        try {
            path = folder ? folderPath(data.getData()) : filePath(data.getData());
        } catch (Exception e) {
            path = "!" + e.getMessage();
        }
        finishWith(path);
    }

    private void finishWith(String path) {
        synchronized (LOCK) {
            answer = path;
            answered = true;
        }
        finish();
        overridePendingTransition(0, 0);
    }

    private static final String STORAGE = "com.android.externalstorage.documents";
    private static final String DOWNLOADS = "com.android.providers.downloads.documents";

    /** "primary:Ribbit/games" -> /storage/emulated/0/Ribbit/games; "1A2B-3C4D:x" -> /storage/1A2B-3C4D/x. */
    private static String storagePath(String id) {
        int colon = id.indexOf(':');
        if (colon < 0) {
            return null;
        }
        String volume = id.substring(0, colon);
        String rest = id.substring(colon + 1);
        if (volume.equals("raw")) {
            return rest;
        }
        String root = volume.equalsIgnoreCase("primary")
                ? Environment.getExternalStorageDirectory().getPath()
                : "/storage/" + volume;
        return rest.isEmpty() ? root : root + "/" + rest;
    }

    private static String folderPath(Uri uri) throws Exception {
        String id = DocumentsContract.getTreeDocumentId(uri);
        String path = null;
        if (STORAGE.equals(uri.getAuthority())) {
            path = storagePath(id);
        } else if (DOWNLOADS.equals(uri.getAuthority())) {
            path = id.startsWith("raw:")
                    ? id.substring(4)
                    : Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOWNLOADS).getPath();
        }
        if (path == null || !new File(path).isDirectory()) {
            throw new Exception("Choose a folder on the phone or its SD card.");
        }
        return path;
    }

    private String filePath(Uri uri) throws Exception {
        String authority = uri.getAuthority();
        if (STORAGE.equals(authority) || DOWNLOADS.equals(authority)) {
            String path = storagePath(DocumentsContract.getDocumentId(uri));
            if (path != null && new File(path).isFile()) {
                return path;
            }
        }
        return copied(uri);
    }

    /** A file Ribbit can't reach by path, copied into its own folder. */
    private String copied(Uri uri) throws Exception {
        String name = "picked";
        try (Cursor c = getContentResolver().query(uri, new String[] {OpenableColumns.DISPLAY_NAME}, null, null, null)) {
            if (c != null && c.moveToFirst() && c.getString(0) != null) {
                name = c.getString(0).replace('/', '_');
            }
        }
        File dir = new File(getExternalFilesDir(null), "imported");
        if (!dir.isDirectory() && !dir.mkdirs()) {
            throw new Exception("Can't make " + dir);
        }
        File out = new File(dir, name);
        try (InputStream in = getContentResolver().openInputStream(uri); OutputStream o = new FileOutputStream(out)) {
            if (in == null) {
                throw new Exception("Can't read the chosen file.");
            }
            byte[] buf = new byte[1 << 16];
            for (int n; (n = in.read(buf)) > 0; ) {
                o.write(buf, 0, n);
            }
        }
        return out.getPath();
    }
}
