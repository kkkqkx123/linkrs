package io.linkrs;

import java.lang.ref.Cleaner;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;

/**
 * Owner of one native transaction handle; keeps its session alive.
 *
 * <p>Commit and rollback do not free the handle: always {@link #close()}
 * afterwards. Repeating close is safe; a cleaner closes handles the caller
 * forgot.
 */
public final class Transaction implements AutoCloseable {
    private static final Cleaner CLEANER = Cleaner.create();

    private final Object lock = new Object();
    private long handle;
    private final Session session;
    private boolean closed;
    private final Cleaner.Cleanable cleanable;

    Transaction(long handle, Session session) {
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
            NativeLib.nTxnFree(handle);
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

    private long requireOpen() {
        synchronized (lock) {
            if (closed || handle == 0) {
                throw new MisuseError("transaction is closed", 20, 0, null);
            }
            return handle;
        }
    }

    /** Execute a query inside the transaction. */
    public Result execute(String query) {
        synchronized (lock) {
            long[] out = new long[1];
            Json.check(NativeLib.nTxnExecuteHandle(requireOpen(), NativeLib.utf8(query), out),
                    session.nativeHandle());
            return new Result(out[0], session);
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
        ParamArrays packed = ParamArrays.pack(params);
        synchronized (lock) {
            long[] out = new long[1];
            Json.check(NativeLib.nTxnExecuteParamsHandle(requireOpen(), NativeLib.utf8(query),
                    packed.types, packed.longs, packed.doubles, packed.strings, out),
                    session.nativeHandle());
            return new Result(out[0], session);
        }
    }

    /** Commit; the handle still needs {@link #close()} afterwards. */
    public void commit() {
        synchronized (lock) {
            Json.check(NativeLib.nTxnCommit(requireOpen()), session.nativeHandle());
        }
    }

    /** Roll back; the handle still needs {@link #close()} afterwards. */
    public void rollback() {
        synchronized (lock) {
            Json.check(NativeLib.nTxnRollback(requireOpen()), session.nativeHandle());
        }
    }

    /**
     * Create a savepoint and return its id. No status code travels with the
     * id, so failures raise with the thread-local detail.
     */
    public long savepoint(String name) {
        synchronized (lock) {
            long id = NativeLib.nTxnSavepoint(requireOpen(), NativeLib.utf8(name));
            if (id < 0) {
                throw Json.failure(1, session.nativeHandle());
            }
            return id;
        }
    }

    /** Release a savepoint by id. */
    public void releaseSavepoint(long id) {
        synchronized (lock) {
            Json.check(NativeLib.nTxnReleaseSavepoint(requireOpen(), id), session.nativeHandle());
        }
    }

    /** Roll back to a savepoint by id. */
    public void rollbackToSavepoint(long id) {
        synchronized (lock) {
            Json.check(NativeLib.nTxnRollbackToSavepoint(requireOpen(), id),
                    session.nativeHandle());
        }
    }

    /** Parallel arrays crossing into the C parameter builder. */
    static final class ParamArrays {
        final int[] types;
        final long[] longs;
        final double[] doubles;
        final byte[][] strings;

        ParamArrays(int[] types, long[] longs, double[] doubles, byte[][] strings) {
            this.types = types;
            this.longs = longs;
            this.doubles = doubles;
            this.strings = strings;
        }

        static ParamArrays pack(List<Object> params) {
            int count = params.size();
            int[] types = new int[count];
            long[] longs = new long[count];
            double[] doubles = new double[count];
            byte[][] strings = new byte[count][];
            for (int i = 0; i < count; i++) {
                Object item = params.get(i);
                if (item == null) {
                    types[i] = 0;
                } else if (item instanceof Boolean b) {
                    types[i] = 1;
                    longs[i] = b ? 1 : 0;
                } else if (item instanceof Number n) {
                    if (item instanceof Double || item instanceof Float) {
                        types[i] = 3;
                        doubles[i] = n.doubleValue();
                    } else {
                        types[i] = 2;
                        longs[i] = n.longValue();
                    }
                } else if (item instanceof String s) {
                    types[i] = 4;
                    strings[i] = s.getBytes(StandardCharsets.UTF_8);
                } else if (item instanceof byte[] raw) {
                    types[i] = 4;
                    strings[i] = raw;
                } else {
                    throw new MisuseError(
                            "unsupported query parameter type: " + item.getClass().getName(),
                            20, 0, null);
                }
            }
            return new ParamArrays(types, longs, doubles, strings);
        }
    }
}
