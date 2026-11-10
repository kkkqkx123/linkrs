package io.linkrs;

/** Base error carrying the raw primary and extended C API status codes. */
public class LinkrsError extends RuntimeException {
    private final int code;
    private final int extended;
    private final Integer offset;

    public LinkrsError(String message, int code, int extended, Integer offset) {
        super("[linkrs:" + code + "/" + extended + "] "
                + (message == null || message.isEmpty() ? "unknown error" : message));
        this.code = code;
        this.extended = extended;
        this.offset = offset;
    }

    /** Primary status code ({@code linkrs_error_code_t}). */
    public int getCode() {
        return code;
    }

    /** Extended status code ({@code linkrs_extended_error_code_t}), 0 when unset. */
    public int getExtended() {
        return extended;
    }

    /** Query error character offset, or null when not applicable. */
    public Integer getOffset() {
        return offset;
    }

    /** Unwrap any Linkrs failure to its primary status code, or -1. */
    public static int codeOf(Throwable failure) {
        return failure instanceof LinkrsError error ? error.getCode() : -1;
    }

    public static boolean isBusy(Throwable failure) {
        int code = codeOf(failure);
        return code == 5 || code == 6;
    }

    public static boolean isReadonly(Throwable failure) {
        return codeOf(failure) == 8;
    }

    public static boolean isNotFound(Throwable failure) {
        return codeOf(failure) == 12;
    }

    public static boolean isSchema(Throwable failure) {
        return codeOf(failure) == 16;
    }

    public static boolean isConstraint(Throwable failure) {
        return codeOf(failure) == 18;
    }

    public static boolean isMismatch(Throwable failure) {
        return codeOf(failure) == 19;
    }

    public static boolean isMisuse(Throwable failure) {
        return codeOf(failure) == 20;
    }
}
