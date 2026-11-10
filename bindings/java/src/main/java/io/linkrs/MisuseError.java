package io.linkrs;

/** Primary status code 20 (see {@link LinkrsError}). */
public class MisuseError extends LinkrsError {
    public MisuseError(String message, int code, int extended, Integer offset) {
        super(message, code, extended, offset);
    }
}
