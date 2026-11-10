package io.linkrs;

/** Primary status code 8 (see {@link LinkrsError}). */
public class ReadonlyError extends LinkrsError {
    public ReadonlyError(String message, int code, int extended, Integer offset) {
        super(message, code, extended, offset);
    }
}
