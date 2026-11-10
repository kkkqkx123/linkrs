package io.linkrs;

/** Primary status code 5 (see {@link LinkrsError}). */
public class BusyError extends LinkrsError {
    public BusyError(String message, int code, int extended, Integer offset) {
        super(message, code, extended, offset);
    }
}
