package io.linkrs;

import java.lang.ref.Cleaner;
import java.util.List;
import java.util.Map;

/**
 * Owner of one native database handle.
 *
 * <p>Sessions keep their database alive; close sessions before the
 * database. Repeating close is safe; a cleaner closes handles the caller
 * forgot. A session must not be shared across threads concurrently.
 */
public final class Database implements AutoCloseable {
    private static final Cleaner CLEANER = Cleaner.create();

    private final Object lock = new Object();
    private long handle;
    private boolean closed;
    private final Cleaner.Cleanable cleanable;

    private Database(long handle) {
        this.handle = handle;
        this.cleanable = CLEANER.register(this, new Releaser(handle));
    }

    private static final class Releaser implements Runnable {
        private final long handle;

        Releaser(long handle) {
            this.handle = handle;
        }

        @Override
        public void run() {
            NativeLib.nClose(handle);
        }
    }

    private static Database checked(long[] out, int code) {
        if (code != 0) {
            throw Json.failure(code, 0);
        }
        return new Database(out[0]);
    }

    /** Open the database at a path. */
    public static Database open(String path) {
        NativeLib.ensureLoaded();
        long[] out = new long[1];
        return checked(out, NativeLib.nOpen(NativeLib.utf8(path), out));
    }

    /** Open an in-memory database. */
    public static Database openMemory() {
        NativeLib.ensureLoaded();
        long[] out = new long[1];
        return checked(out, NativeLib.nOpenMemory(out));
    }

    /** Open with a configuration handle; the config stays usable. */
    public static Database openWithConfig(Config config) {
        NativeLib.ensureLoaded();
        long[] out = new long[1];
        return checked(out, NativeLib.nOpenWithConfig(config.requireOpen(), out));
    }

    /** Library version string. */
    public static String libversion() {
        NativeLib.ensureLoaded();
        return Json.text(NativeLib.nLibversion());
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
                throw new MisuseError("database is closed", 20, 0, null);
            }
            return handle;
        }
    }

    /** Database-level status code. */
    public int lastCode() {
        synchronized (lock) {
            if (closed || handle == 0) {
                return 20;
            }
            return NativeLib.nErrcode(handle);
        }
    }

    /** Thread-local error text without raising. */
    public String lastMessage() {
        return Json.text(NativeLib.nLastErrorText());
    }

    /** Open a new session on this database. */
    public Session session() {
        long[] out = new long[1];
        synchronized (lock) {
            Json.check(NativeLib.nSessionCreate(requireOpen(), out), 0);
        }
        return new Session(out[0], this);
    }

    /** Create a graph space; null vidType/comment select defaults. */
    public void createSpace(String name, int partitions, int replicas, String vidType,
            String comment) {
        synchronized (lock) {
            Json.check(NativeLib.nCreateSpace(requireOpen(), NativeLib.utf8(name), partitions,
                    replicas, NativeLib.utf8(vidType), NativeLib.utf8(comment)), 0);
        }
    }

    public void createSpace(String name) {
        createSpace(name, 1, 1, null, null);
    }

    /** Drop a graph space by name. */
    public void dropSpace(String name) {
        synchronized (lock) {
            Json.check(NativeLib.nDropSpace(requireOpen(), NativeLib.utf8(name)), 0);
        }
    }

    /** List graph space names. */
    public List<String> listSpaces() {
        synchronized (lock) {
            byte[] json = NativeLib.nListSpaces(requireOpen());
            if (json == null) {
                throw Json.failure(1, 0);
            }
            return Json.stringList(json);
        }
    }

    /** Describe a graph space, or null when unknown. */
    public Map<String, Object> getSpace(String name) {
        synchronized (lock) {
            byte[] json = NativeLib.nGetSpace(requireOpen(), NativeLib.utf8(name));
            return json == null ? null : Json.row(json);
        }
    }

    /** Create a tag in a space; null properties store no fields. */
    public void createTag(String space, String name, Object properties) {
        synchronized (lock) {
            Json.check(NativeLib.nCreateTag(requireOpen(), NativeLib.utf8(space),
                    NativeLib.utf8(name), Json.encode(properties)), 0);
        }
    }

    /** Drop a tag from a space. */
    public void dropTag(String space, String name) {
        synchronized (lock) {
            Json.check(NativeLib.nDropTag(requireOpen(), NativeLib.utf8(space),
                    NativeLib.utf8(name)), 0);
        }
    }

    /** List tag names in a space. */
    public List<String> listTags(String space) {
        synchronized (lock) {
            byte[] json = NativeLib.nListTags(requireOpen(), NativeLib.utf8(space));
            if (json == null) {
                throw Json.failure(1, 0);
            }
            return Json.stringList(json);
        }
    }

    /** Describe a tag, or null when unknown. */
    public Map<String, Object> getTag(String space, String name) {
        synchronized (lock) {
            byte[] json = NativeLib.nGetTag(requireOpen(), NativeLib.utf8(space),
                    NativeLib.utf8(name));
            return json == null ? null : Json.row(json);
        }
    }

    /** Create an edge type in a space. */
    public void createEdgeType(String space, String name, Object properties) {
        synchronized (lock) {
            Json.check(NativeLib.nCreateEdgeType(requireOpen(), NativeLib.utf8(space),
                    NativeLib.utf8(name), Json.encode(properties)), 0);
        }
    }

    /** Drop an edge type from a space. */
    public void dropEdgeType(String space, String name) {
        synchronized (lock) {
            Json.check(NativeLib.nDropEdgeType(requireOpen(), NativeLib.utf8(space),
                    NativeLib.utf8(name)), 0);
        }
    }

    /** List edge type names in a space. */
    public List<String> listEdgeTypes(String space) {
        synchronized (lock) {
            byte[] json = NativeLib.nListEdgeTypes(requireOpen(), NativeLib.utf8(space));
            if (json == null) {
                throw Json.failure(1, 0);
            }
            return Json.stringList(json);
        }
    }

    /** Describe an edge type, or null when unknown. */
    public Map<String, Object> getEdgeType(String space, String name) {
        synchronized (lock) {
            byte[] json = NativeLib.nGetEdgeType(requireOpen(), NativeLib.utf8(space),
                    NativeLib.utf8(name));
            return json == null ? null : Json.row(json);
        }
    }
}
