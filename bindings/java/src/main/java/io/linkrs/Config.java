package io.linkrs;

import java.lang.ref.Cleaner;

/**
 * Owner of one native configuration handle.
 *
 * <p>The handle is only used to open a database; tuning setters arrive with
 * the extended binding tier. Always {@link #close()} when done; a cleaner
 * closes handles the caller forgot.
 */
public final class Config implements AutoCloseable {
    private static final Cleaner CLEANER = Cleaner.create();

    private final Object lock = new Object();
    private long handle;
    private boolean closed;
    private final Cleaner.Cleanable cleanable;

    private Config(long handle) {
        if (handle == 0) {
            throw new LinkrsError(Json.text(NativeLib.nLastErrorText()), 7, 0, null);
        }
        this.handle = handle;
        this.cleanable = CLEANER.register(this, new Releaser(handle));
    }

    private static final class Releaser implements Runnable {
        private long handle;

        Releaser(long handle) {
            this.handle = handle;
        }

        @Override
        public void run() {
            if (handle != 0) {
                NativeLib.nConfigFree(handle);
            }
        }
    }

    /** Default configuration. */
    public static Config create() {
        NativeLib.ensureLoaded();
        return new Config(NativeLib.nConfigNew());
    }

    /** Configuration loaded from a file. */
    public static Config forFile(String path) {
        NativeLib.ensureLoaded();
        return new Config(NativeLib.nConfigFile(NativeLib.utf8(path)));
    }

    /** In-memory configuration. */
    public static Config memory() {
        NativeLib.ensureLoaded();
        return new Config(NativeLib.nConfigMemory());
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
                throw new MisuseError("config is closed", 20, 0, null);
            }
            return handle;
        }
    }
}
