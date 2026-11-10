package io.linkrs;

import java.lang.ref.Cleaner;
import java.nio.charset.StandardCharsets;
import java.util.List;
import java.util.Map;

/**
 * Owner of one native result handle; keeps its session alive.
 *
 * <p>Rows are read through the JSON shortcut. Repeating close is safe; a
 * cleaner closes handles the caller forgot.
 */
public final class Result implements AutoCloseable {
    private static final Cleaner CLEANER = Cleaner.create();

    private final Object lock = new Object();
    private long handle;
    private final Session session;
    private boolean closed;
    private final Cleaner.Cleanable cleanable;

    Result(long handle, Session session) {
        this.handle = handle;
        this.session = session;
        this.cleanable = CLEANER.register(this, new Releaser(handle));
    }

    private static final class Releaser implements Runnable {
        private final long handle;

        Releaser(long handle) {
            this.handle = handle;
        }

        @Override
        public void run() {
            NativeLib.nResultFree(handle);
        }
    }

    /** Repeating close is safe. */
    @Override
    public void close() {
        synchronized (lock) {
            if (closed) {
                return;
            }
            closed = true;
        }
        cleanable.clean();
        synchronized (lock) {
            handle = 0;
        }
    }

    public boolean isClosed() {
        synchronized (lock) {
            return closed;
        }
    }

    long requireOpen() {
        synchronized (lock) {
            if (closed || handle == 0) {
                throw new MisuseError("result is closed", 20, 0, null);
            }
            return handle;
        }
    }

    /** Number of columns. */
    public int columnCount() {
        synchronized (lock) {
            int count = NativeLib.nColumnCount(requireOpen());
            if (count < 0) {
                throw Json.failure(1, 0);
            }
            return count;
        }
    }

    /** Number of rows. */
    public int rowCount() {
        synchronized (lock) {
            int count = NativeLib.nRowCount(requireOpen());
            if (count < 0) {
                throw Json.failure(1, 0);
            }
            return count;
        }
    }

    /** Column names in order. */
    public List<String> columnNames() {
        synchronized (lock) {
            long handle = requireOpen();
            int count = NativeLib.nColumnCount(handle);
            if (count < 0) {
                throw Json.failure(1, 0);
            }
            java.util.ArrayList<String> names = new java.util.ArrayList<>(count);
            for (int i = 0; i < count; i++) {
                byte[] name = NativeLib.nColumnName(handle, i);
                if (name == null) {
                    throw Json.failure(1, session.nativeHandle());
                }
                names.add(new String(name, StandardCharsets.UTF_8));
            }
            return names;
        }
    }

    /**
     * All rows as maps via the JSON shortcut.
     *
     * <p>The engine wraps the payload as a {columns, rows, metadata}
     * envelope; the row list is returned. Cell values keep their tagged
     * serde shape.
     */
    public List<Map<String, Object>> rows() {
        synchronized (lock) {
            byte[][] out = new byte[1][];
            Json.check(NativeLib.nResultToJson(requireOpen(), out), session.nativeHandle());
            return Json.envelopeRows(out[0]);
        }
    }

    /** One row as a map via the JSON shortcut. */
    public Map<String, Object> row(int index) {
        synchronized (lock) {
            byte[][] out = new byte[1][];
            Json.check(NativeLib.nRowToJson(requireOpen(), index, out), session.nativeHandle());
            return Json.row(out[0]);
        }
    }
}
