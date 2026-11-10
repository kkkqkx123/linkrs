package io.linkrs;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.List;

/**
 * Thin JNI declarations over the Linkrs C API plus library location.
 *
 * <p>All text crosses as UTF-8 bytes (never modified UTF-8): every string
 * parameter is a {@code byte[]} and every JSON output is a {@code byte[][]}
 * out slot holding standard UTF-8. Null means absent, matching the C rule
 * that null inputs are borrowed and outputs are allocated by the library.
 */
final class NativeLib {
    private NativeLib() {
    }

    static {
        load();
    }

    static void ensureLoaded() {
        // Loading happens in the static initializer; the call keeps the reference.
    }

    private static void load() {
        List<String> tried = new ArrayList<>();
        String explicitBridge = firstNonBlank(
                System.getProperty("linkrs.jni.lib"), System.getenv("LINKRS_JNI_LIB"));
        if (explicitBridge != null) {
            tried.add(explicitBridge);
            preloadRustLib();
            System.load(explicitBridge);
            return;
        }
        Path userDir = Paths.get(System.getProperty("user.dir", ".")).toAbsolutePath();
        Path workspace = userDir.resolve("../../").normalize();
        for (String profile : new String[]{"debug", "release"}) {
            for (String name : bridgeNames()) {
                Path candidate = workspace.resolve("target").resolve(profile).resolve(name);
                tried.add(candidate.toString());
                if (Files.isRegularFile(candidate)) {
                    preloadRustLib();
                    System.load(candidate.toString());
                    return;
                }
            }
        }
        preloadRustLib();
        try {
            System.loadLibrary("linkrs_jni");
            return;
        } catch (UnsatisfiedLinkError first) {
            throw new UnsatisfiedLinkError(
                    "liblinkrs_jni not found; build it with scripts/build-native.sh after "
                            + "`cargo build --features c_api`, then set LINKRS_JNI_LIB, "
                            + "use -Dlinkrs.jni.lib=<path>, or rely on java.library.path. "
                            + "Tried: " + String.join(", ", tried)
                            + ". Original: " + first.getMessage());
        }
    }

    private static void preloadRustLib() {
        String explicit = System.getenv("LINKRS_LIB");
        if (explicit != null && !explicit.isBlank()) {
            System.load(explicit);
        }
    }

    private static String firstNonBlank(String first, String second) {
        if (first != null && !first.isBlank()) {
            return first;
        }
        if (second != null && !second.isBlank()) {
            return second;
        }
        return null;
    }

    private static String[] bridgeNames() {
        String os = System.getProperty("os.name", "").toLowerCase();
        if (os.contains("mac")) {
            return new String[]{"liblinkrs_jni.dylib"};
        }
        if (os.contains("win")) {
            return new String[]{"linkrs_jni.dll"};
        }
        return new String[]{"liblinkrs_jni.so"};
    }

    static byte[] utf8(String text) {
        return text == null ? null : text.getBytes(StandardCharsets.UTF_8);
    }

    static native int nOpen(byte[] pathUtf8, long[] handleOut);

    static native int nOpenMemory(long[] handleOut);

    static native int nOpenWithConfig(long config, long[] handleOut);

    static native int nClose(long db);

    static native int nErrcode(long db);

    static native byte[] nLibversion();

    static native byte[] nLastErrorText();

    static native long nConfigNew();

    static native long nConfigFile(byte[] pathUtf8);

    static native long nConfigMemory();

    static native int nConfigFree(long config);

    static native int nSessionCreate(long db, long[] handleOut);

    static native int nSessionClose(long session);

    static native int nUseSpace(long session, byte[] space);

    static native byte[] nCurrentSpace(long session);

    static native int nSessionStatistics(long session, long[] out4);

    static native int nErrorOffset(long session);

    static native int nExtendedErrcode(long session);

    static native int nExecuteHandle(long session, byte[] query, long[] handleOut);

    static native int nExecuteParamsHandle(long session, byte[] query, int[] types, long[] longs,
            double[] doubles, byte[][] strings, long[] handleOut);

    static native int nValidateQuery(long session, byte[] query);

    static native int nExplainQueryHandle(long session, byte[] query, long[] handleOut);

    static native int nResultFree(long result);

    static native int nColumnCount(long result);

    static native int nRowCount(long result);

    static native byte[] nColumnName(long result, int index);

    static native int nRowToJson(long result, int row, byte[][] out);

    static native int nResultToJson(long result, byte[][] out);

    static native int nTxnBegin(long session, long[] handleOut);

    static native int nTxnBeginReadonly(long session, long[] handleOut);

    static native int nTxnExecuteHandle(long txn, byte[] query, long[] handleOut);

    static native int nTxnExecuteParamsHandle(long txn, byte[] query, int[] types, long[] longs,
            double[] doubles, byte[][] strings, long[] handleOut);

    static native int nTxnCommit(long txn);

    static native int nTxnRollback(long txn);

    static native void nTxnFree(long txn);

    static native long nTxnSavepoint(long txn, byte[] name);

    static native int nTxnReleaseSavepoint(long txn, long id);

    static native int nTxnRollbackToSavepoint(long txn, long id);

    static native int nCreateSpace(long db, byte[] name, int partitions, int replicas,
            byte[] vidType, byte[] comment);

    static native int nDropSpace(long db, byte[] name);

    static native byte[] nListSpaces(long db);

    static native byte[] nGetSpace(long db, byte[] name);

    static native int nCreateTag(long db, byte[] space, byte[] name, byte[] propertiesJson);

    static native int nDropTag(long db, byte[] space, byte[] name);

    static native byte[] nListTags(long db, byte[] space);

    static native byte[] nGetTag(long db, byte[] space, byte[] name);

    static native int nCreateEdgeType(long db, byte[] space, byte[] name, byte[] propertiesJson);

    static native int nDropEdgeType(long db, byte[] space, byte[] name);

    static native byte[] nListEdgeTypes(long db, byte[] space);

    static native byte[] nGetEdgeType(long db, byte[] space, byte[] name);
}
