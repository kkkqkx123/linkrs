package io.linkrs;

import java.lang.ref.Cleaner;
import java.nio.charset.StandardCharsets;
import java.util.List;

/**
 * Owner of one native session handle; keeps its database alive.
 *
 * <p>Close sessions before their database. Repeating close is safe; a
 * cleaner closes handles the caller forgot. A session must not be shared
 * across threads concurrently.
 */
public final class Session implements AutoCloseable {
    private static final Cleaner CLEANER = Cleaner.create();

    private final Object lock = new Object();
    private long handle;
    private final Database database;
    private boolean closed;
    private final Cleaner.Cleanable cleanable;

    Session(long handle, Database database) {
        this.handle = handle;
        this.database = database;
        this.cleanable = CLEANER.register(this, new Releaser(handle));
    }

    private static final class Releaser implements Runnable {
        private final long handle;

        Releaser(long handle) {
            this.handle = handle;
        }

        @Override
        public void run() {
            NativeLib.nSessionClose(handle);
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
                throw new MisuseError("session is closed", 20, 0, null);
            }
            return handle;
        }
    }

    long nativeHandle() {
        synchronized (lock) {
            return closed ? 0 : handle;
        }
    }

    /** Switch to a graph space. */
    public void useSpace(String name) {
        synchronized (lock) {
            long handle = requireOpen();
            Json.check(NativeLib.nUseSpace(handle, NativeLib.utf8(name)), handle);
        }
    }

    /** Current graph space name, or null when unset. */
    public String currentSpace() {
        synchronized (lock) {
            byte[] name = NativeLib.nCurrentSpace(requireOpen());
            return name == null ? null : new String(name, StandardCharsets.UTF_8);
        }
    }

    /** Execute a query and return its result set. */
    public Result execute(String query) {
        synchronized (lock) {
            long handle = requireOpen();
            long[] out = new long[1];
            Json.check(NativeLib.nExecuteHandle(handle, NativeLib.utf8(query), out), handle);
            return new Result(out[0], this);
        }
    }

    /**
     * Execute a parameterized query. Params accept null, Boolean, integer
     * numbers, floating numbers, String and raw UTF-8 byte arrays.
     */
    public Result executeParams(String query, List<Object> params) {
        if (params == null || params.isEmpty()) {
            return execute(query);
        }
        Transaction.ParamArrays packed = Transaction.ParamArrays.pack(params);
        synchronized (lock) {
            long handle = requireOpen();
            long[] out = new long[1];
            Json.check(NativeLib.nExecuteParamsHandle(handle, NativeLib.utf8(query),
                    packed.types, packed.longs, packed.doubles, packed.strings, out), handle);
            return new Result(out[0], this);
        }
    }

    /** Validate a query without running it. */
    public void validateQuery(String query) {
        synchronized (lock) {
            long handle = requireOpen();
            Json.check(NativeLib.nValidateQuery(handle, NativeLib.utf8(query)), handle);
        }
    }

    /** Explain a query without running it. */
    public Result explainQuery(String query) {
        synchronized (lock) {
            long handle = requireOpen();
            long[] out = new long[1];
            Json.check(NativeLib.nExplainQueryHandle(handle, NativeLib.utf8(query), out), handle);
            return new Result(out[0], this);
        }
    }

    /** Session statistics snapshot. */
    public Statistics statistics() {
        synchronized (lock) {
            long handle = requireOpen();
            long[] out = new long[4];
            Json.check(NativeLib.nSessionStatistics(handle, out), handle);
            return new Statistics(out[0], out[1], out[2], out[3]);
        }
    }

    /** Begin a transaction on this session. */
    public Transaction begin() {
        synchronized (lock) {
            long handle = requireOpen();
            long[] out = new long[1];
            Json.check(NativeLib.nTxnBegin(handle, out), handle);
            return new Transaction(out[0], this);
        }
    }

    /** Begin a read-only transaction on this session. */
    public Transaction beginReadonly() {
        synchronized (lock) {
            long handle = requireOpen();
            long[] out = new long[1];
            Json.check(NativeLib.nTxnBeginReadonly(handle, out), handle);
            return new Transaction(out[0], this);
        }
    }
}
